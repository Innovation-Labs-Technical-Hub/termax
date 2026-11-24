//! Termax encrypted local store.
//!
//! A single SQLite database (WAL mode, foreign keys on) holding vaults,
//! nested groups, hosts and trusted host keys. No secret is written to disk in
//! plaintext.
//!
//! # Key hierarchy
//!
//! ```text
//! password ──Argon2id(salt, params)──┐
//!                                    ├─HKDF-SHA256─► account key
//! Secret Key (128-bit, never stored) ┘                 │ wraps (XChaCha20-Poly1305)
//!                                          ┌───────────┴──────────┐
//!                                     store key              vault key (per vault)
//!                                  (known-hosts keys)        (encrypts records)
//! ```
//!
//! The Argon2id parameters and salt live in the `meta` header table so they
//! can be raised later. Unwrapping the store key verifies the credentials:
//! failure is reported as [`Error::WrongCredentials`], never as corruption.
//! Changing the password only re-wraps keys; records are not re-encrypted.
//!
//! # Records
//!
//! Each encrypted column holds `version || 24-byte random nonce || ciphertext`.
//! The AAD binds it to its table, column, record id and the row's plaintext
//! index columns, so ciphertexts cannot be swapped between rows and index
//! columns cannot be edited undetected. See the `hosts` module docs for which
//! host fields are plaintext and why tags live in the encrypted blob.
//!
//! # Schema
//!
//! Versioned with `PRAGMA user_version`; migrations are an append-only list
//! applied in one transaction on every open (see [`SCHEMA_VERSION`]).

mod crypto;
mod error;
mod groups;
mod hosts;
mod known_hosts;
mod schema;
mod store;
mod vaults;

pub use crypto::{KdfParams, SecretKey, generate_secret_key};
pub use error::{Error, Result};
pub use hosts::HostFilter;
pub use known_hosts::KnownHost;
pub use schema::SCHEMA_VERSION;
pub use store::VaultStore;
