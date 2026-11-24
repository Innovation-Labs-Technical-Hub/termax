//! Store lifecycle: create, unlock, password change and shared helpers.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use oxt_core::VaultId;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::crypto::{
    self, Aad, KDF_ARGON2ID, KdfParams, Key, SALT_LEN, SecretKey, derive_account_key,
    derive_subkey, random_key, unwrap_key, wrap_key,
};
use crate::error::{Error, Result};
use crate::schema;

const KNOWN_HOSTS_ENC_INFO: &[u8] = b"oxterm-vault known-hosts enc v1";
const KNOWN_HOSTS_MAC_INFO: &[u8] = b"oxterm-vault known-hosts mac v1";

/// An unlocked OxTerm local store.
///
/// Holds the decrypted account key, vault keys and Secret Key in memory
/// (zeroized on drop). Mutating methods take `&mut self`; each runs in its own
/// SQLite transaction.
pub struct VaultStore {
    pub(crate) conn: Connection,
    secret_key: SecretKey,
    pub(crate) account_key: Key,
    kdf: KdfParams,
    pub(crate) kh_enc_key: Key,
    pub(crate) kh_mac_key: Key,
    pub(crate) vault_keys: HashMap<VaultId, Key>,
}

impl fmt::Debug for VaultStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VaultStore")
            .field("vaults", &self.vault_keys.len())
            .field("kdf", &self.kdf)
            .finish_non_exhaustive()
    }
}

struct Header {
    params: KdfParams,
    salt: Vec<u8>,
    wrapped_store_key: Vec<u8>,
}

fn store_key_aad() -> Aad {
    Aad::new("meta", "wrapped_store_key")
}

pub(crate) fn vault_key_aad(vault_id: &str) -> Aad {
    Aad::new("vaults", "wrapped_key").field(vault_id)
}

impl VaultStore {
    /// Create a new store at `path` with production Argon2id parameters.
    /// Fails with [`Error::AlreadyInitialized`] if the path already holds one.
    pub fn create(path: impl AsRef<Path>, password: &str, secret_key: &SecretKey) -> Result<Self> {
        Self::create_with_params(path, password, secret_key, KdfParams::default())
    }

    /// Like [`VaultStore::create`] with explicit KDF parameters.
    pub fn create_with_params(
        path: impl AsRef<Path>,
        password: &str,
        secret_key: &SecretKey,
        params: KdfParams,
    ) -> Result<Self> {
        let mut conn = open_connection(path.as_ref(), true)?;
        schema::migrate(&mut conn)?;
        if meta_exists(&conn)? {
            return Err(Error::AlreadyInitialized);
        }

        let mut salt = [0u8; SALT_LEN];
        crypto::fill_random(&mut salt)?;
        let account_key = derive_account_key(password, secret_key, &salt, params)?;
        let store_key = random_key()?;
        let wrapped = wrap_key(&account_key, &store_key, &store_key_aad())?;

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Re-check inside the write lock in case another process raced us.
        if meta_exists(&tx)? {
            return Err(Error::AlreadyInitialized);
        }
        tx.execute(
            "INSERT INTO meta (id, kdf_algorithm, kdf_m_cost_kib, kdf_t_cost, kdf_p_cost, \
             kdf_salt, wrapped_store_key) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                KDF_ARGON2ID,
                params.m_cost_kib(),
                params.t_cost(),
                params.p_cost(),
                &salt[..],
                wrapped
            ],
        )?;
        tx.commit()?;

        Self::assemble(conn, secret_key, account_key, params, &store_key)
    }

    /// Open and unlock an existing store. Wrong password or Secret Key yields
    /// [`Error::WrongCredentials`]; damaged data yields [`Error::Corrupt`].
    pub fn open(path: impl AsRef<Path>, password: &str, secret_key: &SecretKey) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(Error::NotInitialized);
        }
        let mut conn = open_connection(path, false)?;
        schema::migrate(&mut conn)?;
        let header = read_header(&conn)?;
        let account_key = derive_account_key(password, secret_key, &header.salt, header.params)?;
        let store_key = unwrap_key(&account_key, &header.wrapped_store_key, &store_key_aad())
            .map_err(|_| Error::WrongCredentials)?;
        Self::assemble(conn, secret_key, account_key, header.params, &store_key)
    }

    fn assemble(
        conn: Connection,
        secret_key: &SecretKey,
        account_key: Key,
        kdf: KdfParams,
        store_key: &Key,
    ) -> Result<Self> {
        let mut store = Self {
            conn,
            secret_key: secret_key.clone(),
            account_key,
            kdf,
            kh_enc_key: derive_subkey(store_key, KNOWN_HOSTS_ENC_INFO)?,
            kh_mac_key: derive_subkey(store_key, KNOWN_HOSTS_MAC_INFO)?,
            vault_keys: HashMap::new(),
        };
        store.load_vault_keys()?;
        Ok(store)
    }

    fn load_vault_keys(&mut self) -> Result<()> {
        let rows: Vec<(String, Vec<u8>)> = {
            let mut stmt = self.conn.prepare("SELECT id, wrapped_key FROM vaults")?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (id, wrapped) in rows {
            let key = unwrap_key(&self.account_key, &wrapped, &vault_key_aad(&id))
                .map_err(|_| Error::Corrupt(format!("vault {id}: key unwrap failed")))?;
            self.vault_keys.insert(parse_id(&id)?, key);
        }
        Ok(())
    }

    /// Key for `vault_id`, unwrapping it on demand if another connection
    /// created the vault after this store was opened.
    pub(crate) fn vault_key(&self, vault_id: VaultId) -> Result<Key> {
        if let Some(k) = self.vault_keys.get(&vault_id) {
            return Ok(k.clone());
        }
        let id = vault_id.to_string();
        let wrapped: Vec<u8> = self
            .conn
            .query_row("SELECT wrapped_key FROM vaults WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or_else(|| not_found("vault", &id))?;
        unwrap_key(&self.account_key, &wrapped, &vault_key_aad(&id))
            .map_err(|_| Error::Corrupt(format!("vault {id}: key unwrap failed")))
    }

    /// Change the password, keeping the current KDF parameters.
    pub fn change_password(&mut self, current_password: &str, new_password: &str) -> Result<()> {
        self.change_password_with_params(current_password, new_password, self.kdf)
    }

    /// Change the password and (optionally stronger) KDF parameters.
    ///
    /// The current password is re-verified first. A new salt is generated and
    /// the store key and every vault key are re-wrapped in one transaction;
    /// record ciphertexts are untouched.
    pub fn change_password_with_params(
        &mut self,
        current_password: &str,
        new_password: &str,
        params: KdfParams,
    ) -> Result<()> {
        let header = read_header(&self.conn)?;
        let current = derive_account_key(
            current_password,
            &self.secret_key,
            &header.salt,
            header.params,
        )?;
        let store_key = unwrap_key(&current, &header.wrapped_store_key, &store_key_aad())
            .map_err(|_| Error::WrongCredentials)?;

        let mut salt = [0u8; SALT_LEN];
        crypto::fill_random(&mut salt)?;
        let new_key = derive_account_key(new_password, &self.secret_key, &salt, params)?;
        let wrapped_store = wrap_key(&new_key, &store_key, &store_key_aad())?;

        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let vaults: Vec<(String, Vec<u8>)> = {
            let mut stmt = tx.prepare("SELECT id, wrapped_key FROM vaults")?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (id, wrapped) in vaults {
            let aad = vault_key_aad(&id);
            let vk = unwrap_key(&current, &wrapped, &aad)
                .map_err(|_| Error::Corrupt(format!("vault {id}: key unwrap failed")))?;
            tx.execute(
                "UPDATE vaults SET wrapped_key = ?1 WHERE id = ?2",
                params![wrap_key(&new_key, &vk, &aad)?, id],
            )?;
        }
        tx.execute(
            "UPDATE meta SET kdf_algorithm = ?1, kdf_m_cost_kib = ?2, kdf_t_cost = ?3, \
             kdf_p_cost = ?4, kdf_salt = ?5, wrapped_store_key = ?6 WHERE id = 1",
            params![
                KDF_ARGON2ID,
                params.m_cost_kib(),
                params.t_cost(),
                params.p_cost(),
                &salt[..],
                wrapped_store
            ],
        )?;
        tx.commit()?;

        self.account_key = new_key;
        self.kdf = params;
        Ok(())
    }

    /// KDF parameters currently protecting the store.
    pub fn kdf_params(&self) -> KdfParams {
        self.kdf
    }

    /// Schema version of the open database.
    pub fn schema_version(&self) -> Result<u32> {
        schema::version(&self.conn)
    }
}

fn open_connection(path: &Path, create: bool) -> Result<Connection> {
    let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_URI;
    if create {
        flags |= OpenFlags::SQLITE_OPEN_CREATE;
    }
    let conn = Connection::open_with_flags(path, flags).map_err(map_open_error)?;
    let mode: String = conn
        .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
        .map_err(map_open_error)?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(Error::Corrupt(format!(
            "could not enable WAL journal mode (got `{mode}`)"
        )));
    }
    // secure_delete overwrites freed pages so superseded ciphertexts and
    // index values do not linger; temp_store keeps sort/temp data off disk.
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA synchronous = NORMAL;
         PRAGMA secure_delete = ON;
         PRAGMA temp_store = MEMORY;",
    )
    .map_err(map_open_error)?;
    Ok(conn)
}

fn map_open_error(e: rusqlite::Error) -> Error {
    match e.sqlite_error_code() {
        Some(ErrorCode::NotADatabase) => Error::Corrupt("file is not a vault database".into()),
        _ => Error::Storage(e),
    }
}

fn meta_exists(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM meta)", [], |r| r.get(0))?)
}

/// `(algorithm, m_cost_kib, t_cost, p_cost, salt, wrapped_store_key)`.
type HeaderRow = (String, u32, u32, u32, Vec<u8>, Vec<u8>);

fn read_header(conn: &Connection) -> Result<Header> {
    let row: Option<HeaderRow> = conn
        .query_row(
            "SELECT kdf_algorithm, kdf_m_cost_kib, kdf_t_cost, kdf_p_cost, kdf_salt, \
             wrapped_store_key FROM meta WHERE id = 1",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()?;
    let (algorithm, m, t, p, salt, wrapped_store_key) = row.ok_or(Error::NotInitialized)?;
    if algorithm != KDF_ARGON2ID {
        return Err(Error::Corrupt(format!("unsupported KDF `{algorithm}`")));
    }
    let params = KdfParams::new(m, t, p)
        .map_err(|e| Error::Corrupt(format!("stored KDF parameters: {e}")))?;
    Ok(Header {
        params,
        salt,
        wrapped_store_key,
    })
}

pub(crate) fn not_found(kind: &'static str, id: impl fmt::Display) -> Error {
    Error::NotFound {
        kind,
        id: id.to_string(),
    }
}

/// Parse an id column, reporting failures as corruption.
pub(crate) fn parse_id<T: FromStr>(s: &str) -> Result<T> {
    s.parse()
        .map_err(|_| Error::Corrupt(format!("malformed id `{s}`")))
}

/// Text form of a unit enum as serde writes it (e.g. `Env::Prod` → `prod`).
pub(crate) fn enum_str<T: Serialize>(value: &T) -> Result<String> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => Ok(s),
        _ => Err(Error::Crypto("enum did not serialize to a string".into())),
    }
}

/// Inverse of [`enum_str`].
pub(crate) fn parse_enum<T: DeserializeOwned>(s: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(s.to_owned()))
        .map_err(|_| Error::Corrupt(format!("unknown enum value `{s}`")))
}
