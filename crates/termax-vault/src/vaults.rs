//! Vault CRUD. Each vault has its own random key, stored only wrapped by the
//! account key. `name` and `kind` are plaintext (listing); vault-level
//! settings are encrypted with the vault key.

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use termax_core::{Settings, Vault, VaultId, VaultKind};

use crate::crypto::{Aad, Key, open_json, random_key, seal_json, wrap_key};
use crate::error::{Error, Result};
use crate::store::{VaultStore, enum_str, not_found, parse_enum, parse_id, vault_key_aad};

fn settings_aad(id: &str, name: &str, kind: &str) -> Aad {
    Aad::new("vaults", "settings_ct")
        .field(id)
        .field(name)
        .field(kind)
}

const SELECT_VAULT: &str = "SELECT id, name, kind, settings_ct FROM vaults";

type VaultRow = (String, String, String, Vec<u8>);

fn vault_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<VaultRow> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
}

impl VaultStore {
    fn decode_vault(&self, (id, name, kind, ct): VaultRow) -> Result<Vault> {
        let vault_id: VaultId = parse_id(&id)?;
        let key = self.vault_key(vault_id)?;
        let settings: Settings = open_json(
            &key,
            &ct,
            &settings_aad(&id, &name, &kind),
            &format!("vault {id}"),
        )?;
        Ok(Vault {
            id: vault_id,
            name,
            kind: parse_enum::<VaultKind>(&kind)?,
            settings,
        })
    }

    fn encode_vault_settings(&self, key: &Key, vault: &Vault) -> Result<Vec<u8>> {
        let aad = settings_aad(&vault.id.to_string(), &vault.name, &enum_str(&vault.kind)?);
        seal_json(key, &vault.settings, &aad)
    }

    /// Create a vault and generate its key.
    pub fn create_vault(&mut self, vault: &Vault) -> Result<()> {
        let id = vault.id.to_string();
        if self.vault_exists(&id)? {
            return Err(Error::AlreadyExists { kind: "vault", id });
        }
        let key = random_key()?;
        let wrapped = wrap_key(&self.account_key, &key, &vault_key_aad(&id))?;
        let ct = self.encode_vault_settings(&key, vault)?;
        self.conn.execute(
            "INSERT INTO vaults (id, name, kind, wrapped_key, settings_ct) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, vault.name, enum_str(&vault.kind)?, wrapped, ct],
        )?;
        self.vault_keys.insert(vault.id, key);
        Ok(())
    }

    /// Fetch one vault.
    pub fn get_vault(&self, id: VaultId) -> Result<Vault> {
        let row = self
            .conn
            .query_row(
                &format!("{SELECT_VAULT} WHERE id = ?1"),
                [id.to_string()],
                vault_row,
            )
            .optional()?
            .ok_or_else(|| not_found("vault", id))?;
        self.decode_vault(row)
    }

    /// All vaults, ordered by name.
    pub fn list_vaults(&self) -> Result<Vec<Vault>> {
        let rows: Vec<VaultRow> = {
            let mut stmt = self
                .conn
                .prepare(&format!("{SELECT_VAULT} ORDER BY name COLLATE NOCASE, id"))?;
            stmt.query_map([], vault_row)?
                .collect::<rusqlite::Result<_>>()?
        };
        rows.into_iter().map(|r| self.decode_vault(r)).collect()
    }

    /// Update name, kind and settings of an existing vault.
    pub fn update_vault(&mut self, vault: &Vault) -> Result<()> {
        let key = self.vault_key(vault.id)?;
        let ct = self.encode_vault_settings(&key, vault)?;
        let n = self.conn.execute(
            "UPDATE vaults SET name = ?1, kind = ?2, settings_ct = ?3 WHERE id = ?4",
            params![vault.name, enum_str(&vault.kind)?, ct, vault.id.to_string()],
        )?;
        if n == 0 {
            return Err(not_found("vault", vault.id));
        }
        Ok(())
    }

    /// Delete a vault. If it still holds groups or hosts, fails with
    /// [`Error::NotEmpty`] unless `cascade` is set, in which case its groups
    /// and hosts are deleted too.
    pub fn delete_vault(&mut self, id: VaultId, cascade: bool) -> Result<()> {
        let sid = id.to_string();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM vaults WHERE id = ?1)",
            [&sid],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(not_found("vault", id));
        }
        let non_empty: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM groups WHERE vault_id = ?1) \
             OR EXISTS(SELECT 1 FROM hosts WHERE vault_id = ?1)",
            [&sid],
            |r| r.get(0),
        )?;
        if non_empty && !cascade {
            return Err(Error::NotEmpty {
                kind: "vault",
                id: sid,
            });
        }
        // Groups reference each other; check foreign keys at commit instead
        // of per row so deletion order within the vault does not matter.
        tx.pragma_update(None, "defer_foreign_keys", true)?;
        tx.execute("DELETE FROM hosts WHERE vault_id = ?1", [&sid])?;
        tx.execute("DELETE FROM groups WHERE vault_id = ?1", [&sid])?;
        tx.execute("DELETE FROM vaults WHERE id = ?1", [&sid])?;
        tx.commit()?;
        self.vault_keys.remove(&id);
        Ok(())
    }

    fn vault_exists(&self, id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM vaults WHERE id = ?1)",
            [id],
            |r| r.get(0),
        )?)
    }
}
