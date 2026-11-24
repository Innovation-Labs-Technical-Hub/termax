//! End-to-end tests that run the `oxt` binary against a throwaway HOME.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

const PASSWORD: &str = "correct horse battery";

struct Env {
    home: tempfile::TempDir,
    db: PathBuf,
    secret_key: String,
}

fn oxt(home: &Path, db: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oxt"));
    cmd.env_clear()
        .env("HOME", home)
        .env("USER", "tester")
        .env("OXT_VAULT_PATH", db)
        .env("OXT_VAULT_PASSWORD", PASSWORD);
    cmd
}

fn run(cmd: &mut Command) -> Output {
    let out = cmd.output().expect("run oxt");
    assert!(
        out.status.success(),
        "oxt failed ({:?}):\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).expect("JSON stdout")
}

fn setup() -> Env {
    let home = tempfile::tempdir().unwrap();
    let ssh = home.path().join(".ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    std::fs::write(
        ssh.join("config"),
        "Host *\n  ServerAliveInterval 30\n\n\
         Host web\n  HostName web.example.com\n  User deploy\n\n\
         Host db\n  HostName 10.0.0.5\n  Port 2200\n  ProxyJump web\n",
    )
    .unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../oxt-interop/tests/fixtures/known_hosts"),
        ssh.join("known_hosts"),
    )
    .unwrap();

    let db = home.path().join("data/vault.db");
    let out =
        run(oxt(home.path(), &db).args(["--json", "vault", "init", "--kdf-memory-kib", "64"]));
    let secret_key = json(&out)["secret_key"].as_str().unwrap().to_owned();
    Env {
        home,
        db,
        secret_key,
    }
}

impl Env {
    fn cmd(&self) -> Command {
        let mut cmd = oxt(self.home.path(), &self.db);
        cmd.env("OXT_SECRET_KEY", &self.secret_key);
        cmd
    }
}

#[test]
fn import_list_export_round_trip() {
    let env = setup();

    let dry = json(&run(env.cmd().args([
        "--json",
        "import",
        "ssh-config",
        "--dry-run",
    ])));
    assert_eq!(dry["imported"].as_array().unwrap().len(), 2);
    let listed = json(&run(env.cmd().args(["--json", "hosts", "list"])));
    assert!(
        listed.as_array().unwrap().is_empty(),
        "dry run must not write"
    );

    let first = json(&run(env.cmd().args(["--json", "import", "ssh-config"])));
    assert_eq!(first["imported"].as_array().unwrap().len(), 2);
    assert!(
        first["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s.as_str().unwrap().starts_with('*'))
    );

    let again = json(&run(env.cmd().args(["--json", "import", "ssh-config"])));
    assert!(again["imported"].as_array().unwrap().is_empty());
    assert_eq!(again["duplicates"].as_array().unwrap().len(), 2);

    let hosts = json(&run(env
        .cmd()
        .args(["--json", "hosts", "list", "--search", "10.0.0"])));
    let hosts = hosts.as_array().unwrap();
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0]["label"], "db");
    assert_eq!(hosts[0]["port"], 2200);

    let exported = String::from_utf8(run(env.cmd().args(["hosts", "export"])).stdout).unwrap();
    assert!(exported.contains("Host db"), "{exported}");
    assert!(
        exported.to_lowercase().contains("proxyjump web"),
        "{exported}"
    );
}

#[test]
fn known_hosts_import_keeps_only_concrete_plain_entries() {
    let env = setup();
    let report = json(&run(env.cmd().args(["--json", "import", "known-hosts"])));
    let imported: Vec<&str> = report["imported"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(imported.len(), 3, "{imported:?}");
    assert!(
        imported
            .iter()
            .any(|i| i.starts_with("web.example.com:22 ssh-ed25519 SHA256:"))
    );
    assert!(imported.iter().any(|i| i.starts_with("10.0.0.7:22 ")));
    assert!(
        imported
            .iter()
            .any(|i| i.starts_with("git.example.com:2222 "))
    );

    let skipped = report["skipped"].to_string();
    assert!(skipped.contains("hashed"), "{skipped}");
    assert!(skipped.contains("@cert-authority/@revoked"), "{skipped}");
    assert!(skipped.contains("*.lan"), "{skipped}");

    let again = json(&run(env.cmd().args(["--json", "import", "known-hosts"])));
    assert!(again["imported"].as_array().unwrap().is_empty());
    assert_eq!(again["duplicates"].as_array().unwrap().len(), 3);

    let info = json(&run(env.cmd().args(["--json", "vault", "info"])));
    assert_eq!(info["known_hosts"], 3);
}

#[test]
fn wrong_secret_key_fails_closed() {
    let env = setup();
    let other = json(&run(oxt(
        env.home.path(),
        &env.home.path().join("other.db"),
    )
    .args(["--json", "vault", "init", "--kdf-memory-kib", "64"])))["secret_key"]
        .as_str()
        .unwrap()
        .to_owned();
    let out = oxt(env.home.path(), &env.db)
        .env("OXT_SECRET_KEY", other)
        .args(["hosts", "list"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("wrong password or Secret Key"));
}

#[test]
fn init_refuses_to_overwrite() {
    let env = setup();
    let out = env.cmd().args(["vault", "init"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("refusing to overwrite"));
}

#[test]
fn policy_check_exit_codes() {
    let home = tempfile::tempdir().unwrap();
    let db = home.path().join("unused.db");
    let code = |args: &[&str]| {
        oxt(home.path(), &db)
            .args(args)
            .output()
            .unwrap()
            .status
            .code()
    };

    let rules = home.path().join("rules.toml");
    std::fs::write(
        &rules,
        "[[rule]]\nprogram = \"git\"\nargs = \"status*\"\neffect = \"allow\"\n",
    )
    .unwrap();
    let rules = rules.to_str().unwrap();

    assert_eq!(
        code(&["policy", "check", "git status", "--rules", rules]),
        Some(0)
    );
    assert_eq!(
        code(&[
            "policy",
            "check",
            "git status",
            "--rules",
            rules,
            "--env",
            "prod"
        ]),
        Some(2)
    );
    assert_eq!(code(&["policy", "check", "uptime"]), Some(2));
    assert_eq!(
        code(&["policy", "check", "echo ok && sudo rm -rf /"]),
        Some(3)
    );
    assert_eq!(
        code(&[
            "policy",
            "check",
            "git status",
            "--rules",
            rules,
            "--external",
            "cursor"
        ]),
        Some(3)
    );
    assert_eq!(
        code(&[
            "policy",
            "check",
            "git status",
            "--rules",
            rules,
            "--external",
            "cursor",
            "--operate"
        ]),
        Some(0)
    );
}
