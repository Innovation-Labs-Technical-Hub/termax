//! JSON-RPC 2.0 messages and the endpoint's method parameters and results.
//!
//! | Method | Scope | Result |
//! |---|---|---|
//! | `control.hello {api_version, token}` | — | [`HelloResult`] |
//! | `control.pair {api_version, name, kind, scopes}` | — (user approval) | [`PairResult`] |
//! | `session.list` | observe | [`ListResult`] |
//! | `session.open {host, cols?, rows?}` | operate | [`OpenResult`] |
//! | `session.attach {session, mode}` | observe / operate | [`SubscribeResult`], then `session.output` |
//! | `session.subscribe {session?}` | observe | [`SubscribeResult`], then `session.event` |
//! | `session.send_input {session, data}` | operate + policy | `{}` |
//! | `session.special {session, special, signal?, millis?}` | operate | `{}` |
//!
//! Errors carry a JSON-RPC code and, for endpoint refusals, `data.code`
//! with a stable `TMX-CTL-NNN` code ([`CtlCode`]).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::clients::{ClientKind, Scope};
use crate::error::CtlCode;
use crate::token::Token;

/// Protocol version spoken by this build.
pub const API_VERSION: u32 = 1;

/// JSON-RPC: invalid JSON.
pub const PARSE_ERROR: i64 = -32700;
/// JSON-RPC: not a valid request object.
pub const INVALID_REQUEST: i64 = -32600;
/// JSON-RPC: unknown method.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC: invalid parameters.
pub const INVALID_PARAMS: i64 = -32602;
/// Endpoint refusal or failure; `data.code` holds the `TMX-CTL-NNN` code.
pub const SERVER_ERROR: i64 = -32000;

/// An incoming request (or notification, without `id`).
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    /// Must be `"2.0"`.
    pub jsonrpc: String,
    /// Request id; absent for notifications.
    #[serde(default)]
    pub id: Option<Value>,
    /// Method name.
    pub method: String,
    /// Parameters.
    #[serde(default)]
    pub params: Value,
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    /// JSON-RPC error code.
    pub code: i64,
    /// Human-readable message (never contains secrets).
    pub message: String,
    /// `{"code": "TMX-CTL-NNN", ...}` for endpoint errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// A plain JSON-RPC error.
    pub fn rpc(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// An endpoint error with a stable code.
    pub fn ctl(code: CtlCode, message: impl Into<String>) -> Self {
        Self {
            code: SERVER_ERROR,
            message: message.into(),
            data: Some(serde_json::json!({ "code": code.as_str() })),
        }
    }

    /// Add a field to `data`.
    pub fn with(mut self, key: &str, value: impl Serialize) -> Self {
        let v = serde_json::to_value(value).unwrap_or(Value::Null);
        match &mut self.data {
            Some(Value::Object(m)) => {
                m.insert(key.to_owned(), v);
            }
            _ => self.data = Some(serde_json::json!({ key: v })),
        }
        self
    }

    /// The stable code in `data.code`, if any.
    pub fn ctl_code(&self) -> Option<CtlCode> {
        self.data
            .as_ref()
            .and_then(|d| d.get("code"))
            .and_then(Value::as_str)
            .and_then(CtlCode::parse)
    }
}

/// A response to one request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// `"2.0"`.
    pub jsonrpc: String,
    /// The request's id (`null` when it could not be read).
    pub id: Value,
    /// Success value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    /// A success.
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// A failure.
    pub fn err(id: Value, error: RpcError) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// A server-to-client notification (stream data).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    /// `"2.0"`.
    pub jsonrpc: String,
    /// `session.output`, `session.event` or `session.end`.
    pub method: String,
    /// Payload; always has `subscription`.
    pub params: Value,
}

impl Notification {
    /// A notification.
    pub fn new(method: &str, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params,
        }
    }
}

/// `control.hello` parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloParams {
    /// Client protocol version.
    pub api_version: u32,
    /// Token from pairing, if the client has one.
    #[serde(default)]
    pub token: Option<Token>,
}

/// `control.hello` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResult {
    /// Server protocol version.
    pub api_version: u32,
    /// The authenticated client.
    pub client_id: String,
    /// Its scopes.
    pub scopes: Vec<Scope>,
}

/// `control.pair` parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairParams {
    /// Client protocol version.
    pub api_version: u32,
    /// Display name shown in the approval prompt (1–64 characters).
    pub name: String,
    /// Client kind.
    pub kind: ClientKind,
    /// Requested scopes.
    pub scopes: Vec<Scope>,
}

/// `control.pair` result. The token is returned exactly once.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairResult {
    /// Client id.
    pub client_id: String,
    /// Bearer token for `control.hello`; store it in the OS keychain.
    pub token: Token,
    /// Granted scopes (may be fewer than requested).
    pub scopes: Vec<Scope>,
}

/// One attached client in a [`SessionSummary`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientSummary {
    /// `ui`, `cli`, `mcp` or `headless`.
    pub kind: String,
    /// Client id.
    pub id: String,
    /// `observe` or `operate`.
    pub grant: String,
}

/// A session as listed by `session.list` (hosts by id, never secrets).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Session id (UUIDv7).
    pub id: String,
    /// Saved host id, or `None` for ad-hoc targets.
    pub host_id: Option<String>,
    /// `user@host:port`.
    pub address: String,
    /// Protocol (`ssh`).
    pub protocol: String,
    /// Connection carrying the session, while connected.
    pub connection_id: Option<String>,
    /// Lifecycle state, e.g. `open`, `disconnected`.
    pub state: String,
    /// The `TMX-*` code of a `disconnected` state.
    pub code: Option<String>,
    /// The client that opened it.
    pub owner: ClientSummary,
    /// Attached clients.
    pub clients: Vec<ClientSummary>,
}

/// `session.list` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListResult {
    /// Live sessions, oldest first.
    pub sessions: Vec<SessionSummary>,
}

/// `session.open` parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenParams {
    /// Saved host label.
    pub host: String,
    /// Terminal columns (default 80).
    #[serde(default)]
    pub cols: Option<u16>,
    /// Terminal rows (default 24).
    #[serde(default)]
    pub rows: Option<u16>,
}

/// `session.open` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenResult {
    /// The new session's id; progress arrives through `session.subscribe`.
    pub session: String,
}

/// A parameter object naming one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionParam {
    /// Session id.
    pub session: String,
}

/// How a client attaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachMode {
    /// Read-only mirror (observe scope).
    #[default]
    Observe,
    /// Interactive: input goes through `session.send_input` (operate scope).
    Operate,
}

/// `session.attach` parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachParams {
    /// Session id.
    pub session: String,
    /// Mode.
    #[serde(default)]
    pub mode: AttachMode,
}

/// `session.subscribe` parameters.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SubscribeParams {
    /// Only this session's events; all sessions when absent.
    #[serde(default)]
    pub session: Option<String>,
}

/// Result of a call that starts a stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeResult {
    /// Id carried by every notification of this stream.
    pub subscription: u64,
}

/// `session.send_input` parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendInputParams {
    /// Session id.
    pub session: String,
    /// Text to type (UTF-8), checked by the command policy.
    pub data: String,
}

/// `session.special` parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecialParams {
    /// Session id.
    pub session: String,
    /// `signal` or `break`.
    pub special: String,
    /// Signal name without `SIG` (`INT`, `TERM`, …) for `signal`.
    #[serde(default)]
    pub signal: Option<String>,
    /// Break length for `break` (default 500 ms).
    #[serde(default)]
    pub millis: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctl_errors_carry_a_stable_code_and_extra_data() {
        let e =
            RpcError::ctl(CtlCode::Forbidden, "operate scope required").with("approval_id", "a1");
        assert_eq!(e.code, SERVER_ERROR);
        assert_eq!(e.ctl_code(), Some(CtlCode::Forbidden));
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["data"]["code"], "TMX-CTL-403");
        assert_eq!(v["data"]["approval_id"], "a1");
        assert_eq!(RpcError::rpc(METHOD_NOT_FOUND, "x").ctl_code(), None);
    }

    #[test]
    fn requests_parse_with_and_without_params_or_id() {
        let r: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":7,"method":"session.list"}"#).unwrap();
        assert_eq!(r.id, Some(Value::from(7)));
        assert!(r.params.is_null());
        let n: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"x","params":{"a":1}}"#).unwrap();
        assert!(n.id.is_none());
    }

    #[test]
    fn responses_have_either_result_or_error() {
        let ok = serde_json::to_value(Response::ok(Value::from(1), serde_json::json!({}))).unwrap();
        assert!(ok.get("error").is_none());
        let err = serde_json::to_value(Response::err(
            Value::Null,
            RpcError::rpc(PARSE_ERROR, "bad json"),
        ))
        .unwrap();
        assert!(err.get("result").is_none());
        assert_eq!(err["error"]["code"], PARSE_ERROR);
    }
}
