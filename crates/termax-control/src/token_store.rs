//! Where clients keep their tokens: the OS keychain.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::ControlError;
use crate::token::Token;

/// Keychain service name for control-endpoint client tokens.
pub const KEYCHAIN_SERVICE: &str = "dev.termax.control";

/// Storage for a client's token, keyed by an account name (the client name
/// plus the endpoint).
pub trait TokenStore: Send + Sync {
    /// The stored token, if any.
    fn get(&self, account: &str) -> Result<Option<Token>, ControlError>;
    /// Store or replace the token.
    fn set(&self, account: &str, token: &Token) -> Result<(), ControlError>;
    /// Forget the token (no error if absent).
    fn delete(&self, account: &str) -> Result<(), ControlError>;
}

/// The OS keychain: macOS Keychain, Windows Credential Manager, Linux
/// kernel keyutils (kept across logouts for a few days, not across reboots).
#[derive(Debug, Default, Clone, Copy)]
pub struct KeychainStore;

fn keychain_err(e: keyring::Error) -> ControlError {
    ControlError::Keychain(e.to_string())
}

impl TokenStore for KeychainStore {
    fn get(&self, account: &str) -> Result<Option<Token>, ControlError> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account).map_err(keychain_err)?;
        match entry.get_password() {
            Ok(s) => Ok(Some(Token::from_secret(s))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keychain_err(e)),
        }
    }

    fn set(&self, account: &str, token: &Token) -> Result<(), ControlError> {
        keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .and_then(|e| e.set_password(token.expose()))
            .map_err(keychain_err)
    }

    fn delete(&self, account: &str) -> Result<(), ControlError> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account).map_err(keychain_err)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keychain_err(e)),
        }
    }
}

/// In-process storage (tests and embedders that keep tokens elsewhere).
#[derive(Debug, Default)]
pub struct MemoryStore(Mutex<HashMap<String, Token>>);

impl TokenStore for MemoryStore {
    fn get(&self, account: &str) -> Result<Option<Token>, ControlError> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(account)
            .cloned())
    }

    fn set(&self, account: &str, token: &Token) -> Result<(), ControlError> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(account.to_owned(), token.clone());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), ControlError> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(store: &dyn TokenStore, account: &str) -> Result<(), ControlError> {
        let t = Token::generate()?;
        store.set(account, &t)?;
        assert_eq!(store.get(account)?, Some(t));
        store.delete(account)?;
        assert_eq!(store.get(account)?, None);
        store.delete(account)?;
        Ok(())
    }

    #[test]
    fn memory_store_round_trips() {
        round_trip(&MemoryStore::default(), "tmx").unwrap();
    }

    #[test]
    fn keychain_store_round_trips_where_a_keychain_exists() {
        let account = format!("termax-test-{}", uuid::Uuid::new_v4());
        match round_trip(&KeychainStore, &account) {
            Ok(()) => {}
            Err(ControlError::Keychain(e)) => {
                eprintln!("SKIP: no usable OS keychain here: {e}");
            }
            Err(e) => panic!("{e}"),
        }
    }
}
