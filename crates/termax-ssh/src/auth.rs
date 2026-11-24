//! User authentication: method sequencing, prompts and the trivial-auth guard.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use russh::client::{AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::ssh_encoding::Encode;
use russh::keys::ssh_key::private::KeypairData;
use russh::keys::{HashAlg, PrivateKey, PublicKey};
use russh::{MethodKind, MethodSet};
use tracing::{debug, info};
use zeroize::Zeroizing;

use crate::deadline::PromptClock;
use crate::error::{AttemptOutcome, AuthAttempt, Error, Result};
use crate::handler::ClientHandler;
use crate::options::AuthMethod;
use crate::quirks::Quirks;
use crate::trust::BoxFuture;

/// How many passphrase prompts are allowed per key file.
const PASSPHRASE_ATTEMPTS: u32 = 3;
/// Upper bound on keyboard-interactive info-request rounds per attempt.
const MAX_KBD_ROUNDS: usize = 16;

/// One keyboard-interactive prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KbdPrompt {
    /// Prompt text from the server.
    pub text: String,
    /// Whether the answer may be echoed.
    pub echo: bool,
}

/// A keyboard-interactive info request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KbdInteractiveRequest {
    /// Host being authenticated to (`host:port`).
    pub host_port: String,
    /// Request name (may be empty).
    pub name: String,
    /// Instructions (may be empty).
    pub instructions: String,
    /// Prompts; never empty when the prompter is called.
    pub prompts: Vec<KbdPrompt>,
}

/// UI callback for secrets needed during authentication. Every method
/// returns `None` when the user cancels.
pub trait Prompter: Send + Sync {
    /// Passphrase for an encrypted private key. `attempt` starts at 1.
    fn passphrase<'a>(
        &'a self,
        key_path: &'a Path,
        attempt: u32,
    ) -> BoxFuture<'a, Option<Zeroizing<String>>>;

    /// Password for `user` on `host_port`.
    fn password<'a>(
        &'a self,
        user: &'a str,
        host_port: &'a str,
    ) -> BoxFuture<'a, Option<Zeroizing<String>>>;

    /// Answers for a keyboard-interactive request, one per prompt.
    fn keyboard_interactive<'a>(
        &'a self,
        request: &'a KbdInteractiveRequest,
    ) -> BoxFuture<'a, Option<Vec<Zeroizing<String>>>>;
}

/// A [`Prompter`] that cancels every prompt (non-interactive use).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPrompter;

impl Prompter for NoPrompter {
    fn passphrase<'a>(&'a self, _: &'a Path, _: u32) -> BoxFuture<'a, Option<Zeroizing<String>>> {
        Box::pin(async { None })
    }
    fn password<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Option<Zeroizing<String>>> {
        Box::pin(async { None })
    }
    fn keyboard_interactive<'a>(
        &'a self,
        _: &'a KbdInteractiveRequest,
    ) -> BoxFuture<'a, Option<Vec<Zeroizing<String>>>> {
        Box::pin(async { None })
    }
}

/// What the server could have verified when it reported success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Proof {
    /// Nothing was sent (`none`, or a public key that was never signed).
    Nothing,
    /// A signature over the session id was produced.
    Signature,
    /// A password was sent.
    Password,
    /// Keyboard-interactive answers; the count of prompts answered.
    KbdAnswers(usize),
}

impl Proof {
    fn verifiable(self) -> bool {
        match self {
            Proof::Nothing => false,
            Proof::Signature | Proof::Password => true,
            Proof::KbdAnswers(n) => n > 0,
        }
    }
}

/// Decide whether a success reply is acceptable. A success is trivial when
/// neither this method nor an earlier partial success carried anything the
/// server could verify (CVE-2021-36367 class: a rogue server skipping auth
/// to make the user believe it is the real host).
pub(crate) fn accept_success(
    method: &'static str,
    proof: Proof,
    verified_before: bool,
    allow_trivial: bool,
) -> Result<()> {
    if verified_before || proof.verifiable() || allow_trivial {
        Ok(())
    } else {
        Err(Error::TrivialAuthRefused { method })
    }
}

pub(crate) struct AuthRequest<'a> {
    pub user: &'a str,
    pub host_port: &'a str,
    pub methods: &'a [AuthMethod],
    pub prompter: &'a dyn Prompter,
    pub clock: &'a PromptClock,
    pub quirks: Quirks,
    pub allow_trivial: bool,
}

struct State {
    attempts: Vec<AuthAttempt>,
    server_methods: Option<MethodSet>,
    verified_before: bool,
}

enum Step {
    Done,
    Continue,
}

impl State {
    fn offered(&self, m: MethodKind) -> bool {
        self.server_methods.as_ref().is_none_or(|s| s.contains(&m))
    }

    fn record(&mut self, method: &'static str, credential: String, outcome: AttemptOutcome) {
        debug!(method, credential = %credential, ?outcome, "auth attempt");
        self.attempts.push(AuthAttempt {
            method,
            credential,
            outcome,
        });
    }

    /// Apply a server reply. `proof` describes what this attempt sent.
    fn reply(
        &mut self,
        r: AuthResult,
        method: &'static str,
        credential: String,
        proof: Proof,
        allow_trivial: bool,
    ) -> Result<Step> {
        match r {
            AuthResult::Success => {
                accept_success(method, proof, self.verified_before, allow_trivial)?;
                info!(method, "authenticated");
                Ok(Step::Done)
            }
            AuthResult::Failure {
                remaining_methods,
                partial_success,
            } => {
                self.server_methods = Some(remaining_methods);
                if partial_success {
                    self.verified_before |= proof.verifiable();
                    self.record(method, credential, AttemptOutcome::PartialSuccess);
                } else {
                    self.record(method, credential, AttemptOutcome::Rejected);
                }
                Ok(Step::Continue)
            }
        }
    }
}

/// Run the authentication sequence on a freshly handshaken connection.
pub(crate) async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    req: AuthRequest<'_>,
) -> Result<()> {
    let mut st = State {
        attempts: Vec::new(),
        server_methods: None,
        verified_before: false,
    };

    // `none` probe: learns the accepted methods; a success here is trivial.
    let r = handle.authenticate_none(req.user).await?;
    if let Step::Done = st.reply(r, "none", String::new(), Proof::Nothing, req.allow_trivial)? {
        return Ok(());
    }
    st.attempts.clear();

    for method in req.methods {
        let step = match method {
            AuthMethod::PublicKeyFile { path, passphrase } => {
                try_key_file(handle, &req, &mut st, path, passphrase.as_ref()).await?
            }
            AuthMethod::Password(pw) => try_password(handle, &req, &mut st, pw.as_ref()).await?,
            AuthMethod::KeyboardInteractive => try_kbd(handle, &req, &mut st).await?,
            AuthMethod::Agent { socket } => {
                try_agent(handle, &req, &mut st, socket.as_deref()).await?
            }
        };
        if let Step::Done = step {
            return Ok(());
        }
    }

    Err(Error::AuthFailed {
        user: req.user.to_owned(),
        server_methods: st
            .server_methods
            .map(|m| m.iter().map(|k| <&str>::from(k).to_owned()).collect())
            .unwrap_or_default(),
        attempts: st.attempts,
    })
}

async fn try_key_file(
    handle: &mut Handle<ClientHandler>,
    req: &AuthRequest<'_>,
    st: &mut State,
    path: &Path,
    passphrase: Option<&Zeroizing<String>>,
) -> Result<Step> {
    let label = path.display().to_string();
    if !st.offered(MethodKind::PublicKey) {
        st.record("publickey", label, AttemptOutcome::NotOffered);
        return Ok(Step::Continue);
    }
    let key = match load_key(path, passphrase, req).await {
        Ok(Some(k)) => k,
        Ok(None) => {
            st.record("publickey", label, AttemptOutcome::Skipped);
            return Ok(Step::Continue);
        }
        Err(Error::KeyLoad { reason, .. }) => {
            st.record("publickey", label, AttemptOutcome::Unavailable(reason));
            return Ok(Step::Continue);
        }
        Err(e) => return Err(e),
    };
    let public = key.public_key().clone();
    let mut signer = TrackingSigner {
        source: KeySource::Local(Arc::new(key)),
        signed: false,
    };
    sign_in(handle, req, st, public, &mut signer, label).await
}

async fn try_agent(
    handle: &mut Handle<ClientHandler>,
    req: &AuthRequest<'_>,
    st: &mut State,
    socket: Option<&Path>,
) -> Result<Step> {
    if !st.offered(MethodKind::PublicKey) {
        st.record("publickey", "agent".into(), AttemptOutcome::NotOffered);
        return Ok(Step::Continue);
    }
    let mut agent = match connect_agent(socket).await {
        Ok(a) => a,
        Err(why) => {
            st.record(
                "publickey",
                "agent".into(),
                AttemptOutcome::Unavailable(why),
            );
            return Ok(Step::Continue);
        }
    };
    if let Some(binding) = handle.agent_session_binding()
        && let Err(e) = agent.bind_session(binding, false).await
    {
        debug!(error = %e, "agent did not accept session binding");
    }
    let identities = match agent.request_identities().await {
        Ok(ids) => ids,
        Err(e) => {
            st.record(
                "publickey",
                "agent".into(),
                AttemptOutcome::Unavailable(e.to_string()),
            );
            return Ok(Step::Continue);
        }
    };
    let keys: Vec<(PublicKey, String)> = identities
        .into_iter()
        .filter_map(|id| match id {
            AgentIdentity::PublicKey { key, comment } => Some((key, comment)),
            AgentIdentity::Certificate { .. } => None,
        })
        .collect();
    if keys.is_empty() {
        st.record(
            "publickey",
            "agent".into(),
            AttemptOutcome::Unavailable("agent holds no keys".into()),
        );
        return Ok(Step::Continue);
    }
    let mut signer = TrackingSigner {
        source: KeySource::Agent(&mut agent),
        signed: false,
    };
    for (key, comment) in keys {
        let label = if comment.is_empty() {
            format!("agent:{}", key.fingerprint(HashAlg::Sha256))
        } else {
            format!("agent:{comment}")
        };
        signer.signed = false;
        if let Step::Done = sign_in(handle, req, st, key, &mut signer, label).await? {
            return Ok(Step::Done);
        }
        if !st.offered(MethodKind::PublicKey) {
            break;
        }
    }
    Ok(Step::Continue)
}

/// Public-key attempt through our own signer so we know whether a signature
/// was actually produced before the server reported success.
async fn sign_in(
    handle: &mut Handle<ClientHandler>,
    req: &AuthRequest<'_>,
    st: &mut State,
    key: PublicKey,
    signer: &mut TrackingSigner<'_>,
    label: String,
) -> Result<Step> {
    let hash = if key.algorithm().is_rsa() {
        match rsa_hash(handle, req.quirks).await {
            Ok(h) => Some(h),
            Err(why) => {
                st.record("publickey", label, AttemptOutcome::Unavailable(why.into()));
                return Ok(Step::Continue);
            }
        }
    } else {
        None
    };
    let r = match handle
        .authenticate_publickey_with(req.user, key, hash, signer)
        .await
    {
        Ok(r) => r,
        Err(SignError::Send(_)) => {
            return Err(Error::Protocol("session closed during auth".into()));
        }
        Err(SignError::Failed(why)) => {
            st.record("publickey", label, AttemptOutcome::Unavailable(why));
            return Ok(Step::Continue);
        }
    };
    let proof = if signer.signed {
        Proof::Signature
    } else {
        Proof::Nothing
    };
    st.reply(r, "publickey", label, proof, req.allow_trivial)
}

/// Pick the RSA signature hash. Never SHA-1.
async fn rsa_hash(
    handle: &Handle<ClientHandler>,
    quirks: Quirks,
) -> std::result::Result<HashAlg, &'static str> {
    if quirks.no_rsa_sha2 {
        return Err("server does not support rsa-sha2 signatures");
    }
    match handle.best_supported_rsa_hash().await {
        Ok(Some(Some(h))) => Ok(h),
        Ok(Some(None)) => Err("server only accepts SHA-1 ssh-rsa signatures"),
        // No server-sig-algs: try the strongest RFC 8332 hash.
        Ok(None) | Err(_) => Ok(HashAlg::Sha512),
    }
}

async fn try_password(
    handle: &mut Handle<ClientHandler>,
    req: &AuthRequest<'_>,
    st: &mut State,
    given: Option<&Zeroizing<String>>,
) -> Result<Step> {
    if !st.offered(MethodKind::Password) {
        st.record("password", String::new(), AttemptOutcome::NotOffered);
        return Ok(Step::Continue);
    }
    let pw = match given {
        Some(p) => p.clone(),
        None => req
            .clock
            .pause(req.prompter.password(req.user, req.host_port))
            .await
            .ok_or(Error::Cancelled)?,
    };
    let r = handle
        .authenticate_password(req.user, pw.as_str().to_owned())
        .await?;
    st.reply(
        r,
        "password",
        String::new(),
        Proof::Password,
        req.allow_trivial,
    )
}

async fn try_kbd(
    handle: &mut Handle<ClientHandler>,
    req: &AuthRequest<'_>,
    st: &mut State,
) -> Result<Step> {
    const M: &str = "keyboard-interactive";
    if !st.offered(MethodKind::KeyboardInteractive) {
        st.record(M, String::new(), AttemptOutcome::NotOffered);
        return Ok(Step::Continue);
    }
    let mut answered = 0usize;
    let mut resp = handle
        .authenticate_keyboard_interactive_start(req.user, None::<String>)
        .await?;
    for _ in 0..MAX_KBD_ROUNDS {
        match resp {
            KeyboardInteractiveAuthResponse::Success => {
                return st.reply(
                    AuthResult::Success,
                    M,
                    String::new(),
                    Proof::KbdAnswers(answered),
                    req.allow_trivial,
                );
            }
            KeyboardInteractiveAuthResponse::Failure {
                remaining_methods,
                partial_success,
            } => {
                return st.reply(
                    AuthResult::Failure {
                        remaining_methods,
                        partial_success,
                    },
                    M,
                    String::new(),
                    Proof::KbdAnswers(answered),
                    req.allow_trivial,
                );
            }
            KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                let answers: Vec<String> = if prompts.is_empty() {
                    Vec::new()
                } else {
                    let request = KbdInteractiveRequest {
                        host_port: req.host_port.to_owned(),
                        name,
                        instructions,
                        prompts: prompts
                            .into_iter()
                            .map(|p| KbdPrompt {
                                text: p.prompt,
                                echo: p.echo,
                            })
                            .collect(),
                    };
                    let a = req
                        .clock
                        .pause(req.prompter.keyboard_interactive(&request))
                        .await
                        .ok_or(Error::Cancelled)?;
                    if a.len() != request.prompts.len() {
                        return Err(Error::InvalidConfig(format!(
                            "prompter returned {} answers for {} prompts",
                            a.len(),
                            request.prompts.len()
                        )));
                    }
                    answered += a.len();
                    a.iter().map(|s| s.as_str().to_owned()).collect()
                };
                resp = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await?;
            }
        }
    }
    Err(Error::Protocol(format!(
        "keyboard-interactive exceeded {MAX_KBD_ROUNDS} rounds"
    )))
}

/// Load a private key, prompting for the passphrase if needed. `Ok(None)`
/// means the user skipped the key. Decryption (bcrypt-pbkdf) runs on the
/// blocking pool.
async fn load_key(
    path: &Path,
    given: Option<&Zeroizing<String>>,
    req: &AuthRequest<'_>,
) -> Result<Option<PrivateKey>> {
    let key_err = |reason: String| Error::KeyLoad {
        path: path.to_owned(),
        reason,
    };
    let text = Zeroizing::new(
        tokio::fs::read_to_string(path)
            .await
            .map_err(|e| key_err(e.to_string()))?,
    );
    let text = Arc::new(text);

    let decode = |pw: Option<Zeroizing<String>>| {
        let text = Arc::clone(&text);
        async move {
            tokio::task::spawn_blocking(move || {
                russh::keys::decode_secret_key(&text, pw.as_ref().map(|p| p.as_str()))
            })
            .await
            .map_err(|e| e.to_string())
        }
    };

    let first = decode(given.cloned()).await.map_err(key_err)?;
    match first {
        Ok(k) => return Ok(Some(k)),
        Err(russh::keys::Error::KeyIsEncrypted) if given.is_none() => {}
        Err(e) => return Err(key_err(e.to_string())),
    }
    for attempt in 1..=PASSPHRASE_ATTEMPTS {
        let Some(pw) = req
            .clock
            .pause(req.prompter.passphrase(path, attempt))
            .await
        else {
            return Ok(None);
        };
        match decode(Some(pw)).await.map_err(key_err)? {
            Ok(k) => return Ok(Some(k)),
            Err(e) => {
                debug!(path = %PathBuf::from(path).display(), error = %e, "passphrase rejected")
            }
        }
    }
    Err(key_err("wrong passphrase".into()))
}

type DynAgent = AgentClient<Box<dyn AgentStream + Send + Unpin>>;

#[cfg(unix)]
async fn connect_agent(socket: Option<&Path>) -> std::result::Result<DynAgent, String> {
    let c = match socket {
        Some(p) => AgentClient::connect_uds(p).await,
        None => AgentClient::connect_env().await,
    };
    c.map(|c| c.dynamic()).map_err(|e| e.to_string())
}

#[cfg(windows)]
async fn connect_agent(socket: Option<&Path>) -> std::result::Result<DynAgent, String> {
    const DEFAULT_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";
    let pipe = socket.map_or_else(|| DEFAULT_PIPE.into(), |p| p.as_os_str().to_owned());
    AgentClient::connect_named_pipe(pipe)
        .await
        .map(|c| c.dynamic())
        .map_err(|e| e.to_string())
}

#[cfg(not(any(unix, windows)))]
async fn connect_agent(_socket: Option<&Path>) -> std::result::Result<DynAgent, String> {
    Err("ssh-agent is not supported on this platform".into())
}

enum KeySource<'a> {
    Local(Arc<PrivateKey>),
    Agent(&'a mut DynAgent),
}

/// Signs auth requests and remembers whether it was asked to.
struct TrackingSigner<'a> {
    source: KeySource<'a>,
    signed: bool,
}

#[derive(Debug, thiserror::Error)]
enum SignError {
    #[error("session closed")]
    Send(#[from] russh::SendError),
    #[error("{0}")]
    Failed(String),
}

impl russh::Signer for TrackingSigner<'_> {
    type Error = SignError;

    fn auth_sign(
        &mut self,
        key: &AgentIdentity,
        hash_alg: Option<HashAlg>,
        to_sign: Vec<u8>,
    ) -> impl Future<Output = std::result::Result<Vec<u8>, SignError>> + Send {
        let key = key.clone();
        async move {
            let out = match &mut self.source {
                KeySource::Local(k) => {
                    let k = Arc::clone(k);
                    tokio::task::spawn_blocking(move || sign_local(&k, hash_alg, to_sign))
                        .await
                        .map_err(|e| SignError::Failed(e.to_string()))??
                }
                KeySource::Agent(a) => a
                    .auth_sign(&key, hash_alg, to_sign)
                    .await
                    .map_err(|e| SignError::Failed(format!("agent: {e}")))?,
            };
            self.signed = true;
            Ok(out)
        }
    }
}

/// Append `string(signature-blob)` to `data`, the format russh expects from a signer.
fn sign_local(
    key: &PrivateKey,
    hash_alg: Option<HashAlg>,
    mut data: Vec<u8>,
) -> std::result::Result<Vec<u8>, SignError> {
    use russh::keys::signature::Signer as _;
    let fail = |e: &dyn std::fmt::Display| SignError::Failed(e.to_string());
    let sig: russh::keys::ssh_key::Signature = match key.key_data() {
        KeypairData::Rsa(kp) => {
            let Some(h) = hash_alg else {
                return Err(SignError::Failed("refusing SHA-1 RSA signature".into()));
            };
            (kp, Some(h)).try_sign(&data).map_err(|e| fail(&e))?
        }
        kd => kd.try_sign(&data).map_err(|e| fail(&e))?,
    };
    let blob = sig.encode_vec().map_err(|e| fail(&e))?;
    blob.encode(&mut data).map_err(|e| fail(&e))?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trivial_success_is_refused() {
        assert!(matches!(
            accept_success("none", Proof::Nothing, false, false),
            Err(Error::TrivialAuthRefused { method: "none" })
        ));
        assert!(matches!(
            accept_success("keyboard-interactive", Proof::KbdAnswers(0), false, false),
            Err(Error::TrivialAuthRefused { .. })
        ));
        // Public key accepted without our signature (probe-only success).
        assert!(accept_success("publickey", Proof::Nothing, false, false).is_err());
    }

    #[test]
    fn verified_success_is_accepted() {
        accept_success("publickey", Proof::Signature, false, false).unwrap();
        accept_success("password", Proof::Password, false, false).unwrap();
        accept_success("keyboard-interactive", Proof::KbdAnswers(1), false, false).unwrap();
    }

    #[test]
    fn earlier_partial_success_or_allow_flag_permits_trivial_step() {
        accept_success("keyboard-interactive", Proof::KbdAnswers(0), true, false).unwrap();
        accept_success("none", Proof::Nothing, false, true).unwrap();
    }

    #[test]
    fn local_signature_matches_wire_format() {
        use russh::keys::ssh_encoding::Decode;
        // Throwaway key generated for this test only.
        const TEST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACAcTBGG4vYlkmFcRI4jIo0fsvvGjjQLGM3r8wGX7ov+KAAAAIglvMzmJbzM
5gAAAAtzc2gtZWQyNTUxOQAAACAcTBGG4vYlkmFcRI4jIo0fsvvGjjQLGM3r8wGX7ov+KA
AAAECzHhCUnVWqjfOECHpqALZ4vwkU+3Q9wfGPCRJ6C/nZ9BxMEYbi9iWSYVxEjiMijR+y
+8aONAsYzevzAZfui/4oAAAABHRlc3QB
-----END OPENSSH PRIVATE KEY-----
";
        let key = russh::keys::decode_secret_key(TEST_KEY, None).unwrap();
        let out = sign_local(&key, None, b"payload".to_vec()).unwrap();
        assert!(out.starts_with(b"payload"));
        let mut rest = &out[b"payload".len()..];
        let blob = Vec::<u8>::decode(&mut rest).unwrap();
        assert!(rest.is_empty());
        let sig = russh::keys::ssh_key::Signature::decode(&mut blob.as_slice()).unwrap();
        assert_eq!(sig.algorithm(), russh::keys::Algorithm::Ed25519);
    }
}
