//! Hosts, listing/filtering and settings resolution.
//!
//! Storage layout: `id`, `vault_id`, `group_id`, `label`, `protocol` and `env`
//! are plaintext so hosts can be listed and coarsely filtered in SQL. Every
//! sensitive field (address, port, tags, settings) is serialized together as
//! one JSON blob and encrypted into `settings_ct` with the vault key. One blob
//! per host keeps a single nonce/AAD per record and lets the sensitive shape
//! evolve without schema changes. The plaintext columns are bound into the AAD,
//! so tampering with e.g. `env` (which drives prod safety rules) or swapping
//! ciphertexts between rows fails authentication.
//!
//! Tags live inside the encrypted blob; tag and text filters decrypt the
//! candidate rows of one vault in memory. That is fine at desktop scale and
//! avoids leaking tag equality through deterministic tokens.

use oxt_core::{Env, GroupId, Host, HostId, Layer, Resolved, SettingSource, Settings, VaultId};
use rusqlite::{OptionalExtension, params_from_iter, types::Value};
use serde::{Deserialize, Serialize};

use crate::crypto::{Aad, open_json, seal_json};
use crate::error::{Error, Result};
use crate::groups::subtree_ids;
use crate::store::{VaultStore, enum_str, not_found, parse_enum, parse_id};

/// Criteria for [`VaultStore::list_hosts`]. All set criteria must match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostFilter {
    /// Case-insensitive substring of the label or address.
    pub text: Option<String>,
    /// Exact tag, case-insensitive.
    pub tag: Option<String>,
    /// Environment.
    pub env: Option<Env>,
    /// Group membership.
    pub group: Option<GroupId>,
    /// With `group`, also match hosts in nested sub-groups.
    pub include_subgroups: bool,
}

#[derive(Serialize)]
struct SecretPartRef<'a> {
    address: &'a str,
    port: Option<u16>,
    tags: &'a [String],
    settings: &'a Settings,
}

#[derive(Deserialize)]
struct SecretPart {
    address: String,
    port: Option<u16>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    settings: Settings,
}

struct HostRow {
    id: String,
    vault_id: String,
    group_id: Option<String>,
    label: String,
    protocol: String,
    env: String,
    ct: Vec<u8>,
}

const SELECT_HOST: &str =
    "SELECT id, vault_id, group_id, label, protocol, env, settings_ct FROM hosts";

fn host_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<HostRow> {
    Ok(HostRow {
        id: r.get(0)?,
        vault_id: r.get(1)?,
        group_id: r.get(2)?,
        label: r.get(3)?,
        protocol: r.get(4)?,
        env: r.get(5)?,
        ct: r.get(6)?,
    })
}

fn settings_aad(
    id: &str,
    vault_id: &str,
    group_id: Option<&str>,
    label: &str,
    protocol: &str,
    env: &str,
) -> Aad {
    Aad::new("hosts", "settings_ct")
        .field(id)
        .field(vault_id)
        .field(group_id.unwrap_or(""))
        .field(label)
        .field(protocol)
        .field(env)
}

impl VaultStore {
    fn decode_host(&self, row: HostRow) -> Result<Host> {
        let vault_id: VaultId = parse_id(&row.vault_id)?;
        let key = self.vault_key(vault_id)?;
        let aad = settings_aad(
            &row.id,
            &row.vault_id,
            row.group_id.as_deref(),
            &row.label,
            &row.protocol,
            &row.env,
        );
        let secret: SecretPart = open_json(&key, &row.ct, &aad, &format!("host {}", row.id))?;
        Ok(Host {
            id: parse_id(&row.id)?,
            vault_id,
            group_id: row.group_id.as_deref().map(parse_id).transpose()?,
            label: row.label,
            address: secret.address,
            port: secret.port,
            protocol: parse_enum(&row.protocol)?,
            env: parse_enum(&row.env)?,
            tags: secret.tags,
            settings: secret.settings,
        })
    }

    /// Validate references and write the host (insert or replace).
    fn write_host(&mut self, host: &Host, insert: bool) -> Result<()> {
        let key = self.vault_key(host.vault_id)?;
        if let Some(g) = host.group_id {
            self.check_group_in_vault(g, host.vault_id)?;
        }
        let id = host.id.to_string();
        let vault_id = host.vault_id.to_string();
        let group_id = host.group_id.map(|g| g.to_string());
        let protocol = enum_str(&host.protocol)?;
        let env = enum_str(&host.env)?;
        let aad = settings_aad(
            &id,
            &vault_id,
            group_id.as_deref(),
            &host.label,
            &protocol,
            &env,
        );
        let ct = seal_json(
            &key,
            &SecretPartRef {
                address: &host.address,
                port: host.port,
                tags: &host.tags,
                settings: &host.settings,
            },
            &aad,
        )?;
        let sql = if insert {
            "INSERT INTO hosts (vault_id, group_id, label, protocol, env, settings_ct, id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        } else {
            "UPDATE hosts SET vault_id = ?1, group_id = ?2, label = ?3, protocol = ?4, \
             env = ?5, settings_ct = ?6 WHERE id = ?7"
        };
        let n = self.conn.execute(
            sql,
            rusqlite::params![vault_id, group_id, host.label, protocol, env, ct, id],
        )?;
        if n == 0 {
            return Err(not_found("host", host.id));
        }
        Ok(())
    }

    fn host_exists(&self, id: HostId) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM hosts WHERE id = ?1)",
            [id.to_string()],
            |r| r.get(0),
        )?)
    }

    /// Create a host. Its group, if any, must belong to the same vault.
    pub fn create_host(&mut self, host: &Host) -> Result<()> {
        if self.host_exists(host.id)? {
            return Err(Error::AlreadyExists {
                kind: "host",
                id: host.id.to_string(),
            });
        }
        self.write_host(host, true)
    }

    /// Replace an existing host (may move it to another vault or group).
    pub fn update_host(&mut self, host: &Host) -> Result<()> {
        self.write_host(host, false)
    }

    /// Fetch and decrypt one host.
    pub fn get_host(&self, id: HostId) -> Result<Host> {
        let row = self
            .conn
            .query_row(
                &format!("{SELECT_HOST} WHERE id = ?1"),
                [id.to_string()],
                host_row,
            )
            .optional()?
            .ok_or_else(|| not_found("host", id))?;
        self.decode_host(row)
    }

    /// Delete a host.
    pub fn delete_host(&mut self, id: HostId) -> Result<()> {
        let n = self
            .conn
            .execute("DELETE FROM hosts WHERE id = ?1", [id.to_string()])?;
        if n == 0 {
            return Err(not_found("host", id));
        }
        Ok(())
    }

    /// Hosts of `vault_id` matching `filter`, ordered by label. Vault, env and
    /// group are filtered in SQL; text and tag after decryption.
    pub fn list_hosts(&self, vault_id: VaultId, filter: &HostFilter) -> Result<Vec<Host>> {
        self.vault_key(vault_id)?;
        let mut sql = format!("{SELECT_HOST} WHERE vault_id = ?");
        let mut args: Vec<Value> = vec![Value::Text(vault_id.to_string())];
        if let Some(env) = filter.env {
            sql.push_str(" AND env = ?");
            args.push(Value::Text(enum_str(&env)?));
        }
        if let Some(group) = filter.group {
            let ids = if filter.include_subgroups {
                subtree_ids(&self.conn, &group.to_string())?
            } else {
                vec![group.to_string()]
            };
            sql.push_str(" AND group_id IN (");
            sql.push_str(&vec!["?"; ids.len()].join(", "));
            sql.push(')');
            args.extend(ids.into_iter().map(Value::Text));
        }
        sql.push_str(" ORDER BY label COLLATE NOCASE, id");

        let rows: Vec<HostRow> = {
            let mut stmt = self.conn.prepare(&sql)?;
            stmt.query_map(params_from_iter(args), host_row)?
                .collect::<rusqlite::Result<_>>()?
        };
        let text = filter.text.as_deref().map(str::to_lowercase);
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let host = self.decode_host(row)?;
            if let Some(t) = &text
                && !host.label.to_lowercase().contains(t.as_str())
                && !host.address.to_lowercase().contains(t.as_str())
            {
                continue;
            }
            if let Some(tag) = &filter.tag
                && !host.has_tag(tag)
            {
                continue;
            }
            out.push(host);
        }
        Ok(out)
    }

    /// Resolve effective settings for a host through
    /// App → Vault → groups (outermost first) → Host → Session.
    pub fn resolved_settings(
        &self,
        host_id: HostId,
        app_settings: &Settings,
        session_settings: &Settings,
    ) -> Result<Resolved> {
        let host = self.get_host(host_id)?;
        let vault = self.get_vault(host.vault_id)?;
        let groups = match host.group_id {
            Some(g) => self.group_chain(g)?,
            None => Vec::new(),
        };
        let mut layers = Vec::with_capacity(groups.len() + 4);
        layers.push(Layer::new(SettingSource::App, app_settings));
        layers.push(Layer::new(SettingSource::Vault, &vault.settings));
        for g in &groups {
            layers.push(Layer::new(
                SettingSource::Group(g.name.clone()),
                &g.settings,
            ));
        }
        layers.push(Layer::new(SettingSource::Host, &host.settings));
        layers.push(Layer::new(SettingSource::Session, session_settings));
        Ok(Resolved::from_layers(layers))
    }
}
