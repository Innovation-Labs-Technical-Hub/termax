//! Error type for the vault store.

/// Errors returned by [`crate::VaultStore`] and the key-handling helpers.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The password or Secret Key does not unlock this store. Returned instead
    /// of a decryption error so callers can prompt again; it never reveals
    /// which of the two secrets was wrong.
    #[error("wrong password or secret key")]
    WrongCredentials,

    /// A Secret Key string could not be parsed.
    #[error("invalid secret key format")]
    InvalidSecretKey,

    /// Argon2 parameters are out of range.
    #[error("invalid KDF parameters: {0}")]
    InvalidKdfParams(String),

    /// The path holds no initialized vault store (missing file or header).
    #[error("no initialized vault store at this path")]
    NotInitialized,

    /// `create` was called on a path that already holds a vault store.
    #[error("vault store already initialized at this path")]
    AlreadyInitialized,

    /// The database was written by a newer schema than this build supports.
    #[error("schema version {found} is newer than the supported version {supported}")]
    SchemaTooNew { found: u32, supported: u32 },

    /// Stored data failed authentication or could not be decoded after the
    /// credentials were verified: corruption or tampering.
    #[error("vault store is corrupt or was tampered with: {0}")]
    Corrupt(String),

    /// The referenced record does not exist.
    #[error("{kind} {id} not found")]
    NotFound { kind: &'static str, id: String },

    /// A record with this id already exists.
    #[error("{kind} {id} already exists")]
    AlreadyExists { kind: &'static str, id: String },

    /// Deleting a vault or group that still has contents without `cascade`.
    #[error("{kind} {id} is not empty; pass cascade = true to delete its contents")]
    NotEmpty { kind: &'static str, id: String },

    /// The request is structurally invalid (cross-vault parent, cycle, ...).
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// The operating system random number generator failed.
    #[error("random number generator failure: {0}")]
    Rng(String),

    /// Key derivation or encryption failed for a reason other than bad input.
    #[error("cryptographic operation failed: {0}")]
    Crypto(String),

    /// Underlying SQLite error.
    #[error(transparent)]
    Storage(#[from] rusqlite::Error),
}

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
