//! Termax local control endpoint.
//!
//! Other processes of the same OS user (the `tmx` CLI, the `tmx mcp` shim,
//! a second app launch) reach the session runtime through this endpoint:
//!
//! - **Transport** ([`transport`], [`paths`]): a Unix domain socket in a
//!   0700 per-user directory (socket 0600, peer UID checked), or a Windows
//!   named pipe whose DACL admits only its owner and that rejects remote
//!   clients. One runtime per user holds the [`InstanceLock`].
//! - **Protocol** ([`frame`], [`protocol`]): length-framed JSON-RPC 2.0 with
//!   `api_version`, stable `TMX-CTL-NNN` error codes ([`CtlCode`]).
//! - **Clients** ([`clients`], [`token`], [`token_store`]): each client pairs
//!   once (`control.pair`) after the user approves it through the
//!   [`Approvals`] hook, and receives a 256-bit token that it keeps in the OS
//!   keychain; the server stores only its SHA-256. Scopes are observe and
//!   operate.
//! - **Server** ([`server::start`]): rate limits, scope gates, the command
//!   policy for input, approval requests for refused operate calls, and an
//!   audit chain of every call. Output to MCP clients is sanitised.
//! - **Headless** ([`headless`]): the runtime and endpoint without a UI,
//!   exiting when idle; sessions come from the vault.

pub mod approvals;
pub mod client;
pub mod clients;
pub mod error;
pub mod frame;
pub mod headless;
mod methods;
pub mod paths;
pub mod protocol;
pub mod server;
pub mod token;
pub mod token_store;
pub mod transport;

pub use approvals::{ApprovalId, ApprovalKind, ApprovalRequest, Approvals, Decision};
pub use client::ControlClient;
pub use clients::{ClientKind, ClientRecord, ClientRegistry, Scope};
pub use error::{ControlError, CtlCode};
pub use headless::{DEFAULT_IDLE_TIMEOUT, Headless, VaultOpener};
pub use methods::{DEFAULT_BREAK_MILLIS, parse_signal};
pub use paths::{Endpoint, InstanceLock};
pub use protocol::API_VERSION;
pub use server::{ControlHandle, RateLimit, ServerConfig, SessionOpener};
pub use token::Token;
pub use token_store::{KeychainStore, MemoryStore, TokenStore};
