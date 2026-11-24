//! Nested host groups. `name` and the tree structure are plaintext; group
//! settings are encrypted with the owning vault's key, with the plaintext
//! columns bound into the AAD so they cannot be altered undetected.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use termax_core::{Group, GroupId, Settings, VaultId};

use crate::crypto::{Aad, open_json, seal_json};
use crate::error::{Error, Result};
use crate::store::{VaultStore, not_found, parse_id};

fn settings_aad(id: &str, vault_id: &str, parent_id: Option<&str>, name: &str) -> Aad {
    Aad::new("groups", "settings_ct")
        .field(id)
        .field(vault_id)
        .field(parent_id.unwrap_or(""))
        .field(name)
}

const SELECT_GROUP: &str = "SELECT id, vault_id, parent_id, name, settings_ct FROM groups";

type GroupRow = (String, String, Option<String>, String, Vec<u8>);

fn group_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<GroupRow> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
}

/// Ids of `root` and all of its descendants.
pub(crate) fn subtree_ids(conn: &Connection, root: &str) -> Result<Vec<String>> {
    // UNION (not UNION ALL) also terminates on a corrupted, cyclic tree.
    let mut stmt = conn.prepare(
        "WITH RECURSIVE sub(id) AS ( \
             SELECT ?1 \
             UNION SELECT g.id FROM groups g JOIN sub ON g.parent_id = sub.id \
         ) SELECT id FROM sub",
    )?;
    let ids = stmt
        .query_map([root], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

impl VaultStore {
    fn decode_group(&self, (id, vault_id, parent_id, name, ct): GroupRow) -> Result<Group> {
        let vid: VaultId = parse_id(&vault_id)?;
        let key = self.vault_key(vid)?;
        let settings: Settings = open_json(
            &key,
            &ct,
            &settings_aad(&id, &vault_id, parent_id.as_deref(), &name),
            &format!("group {id}"),
        )?;
        Ok(Group {
            id: parse_id(&id)?,
            vault_id: vid,
            parent_id: parent_id.as_deref().map(parse_id).transpose()?,
            name,
            settings,
        })
    }

    fn encode_group_settings(&self, group: &Group) -> Result<Vec<u8>> {
        let key = self.vault_key(group.vault_id)?;
        let parent = group.parent_id.map(|p| p.to_string());
        let aad = settings_aad(
            &group.id.to_string(),
            &group.vault_id.to_string(),
            parent.as_deref(),
            &group.name,
        );
        seal_json(&key, &group.settings, &aad)
    }

    /// Vault id of a group, or `None` if it does not exist.
    pub(crate) fn group_vault(&self, id: GroupId) -> Result<Option<VaultId>> {
        let v: Option<String> = self
            .conn
            .query_row(
                "SELECT vault_id FROM groups WHERE id = ?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        v.as_deref().map(parse_id).transpose()
    }

    /// Ensure `group_id` exists and belongs to `vault_id`.
    pub(crate) fn check_group_in_vault(&self, group_id: GroupId, vault_id: VaultId) -> Result<()> {
        match self.group_vault(group_id)? {
            None => Err(not_found("group", group_id)),
            Some(v) if v != vault_id => Err(Error::InvalidInput(format!(
                "group {group_id} belongs to a different vault"
            ))),
            Some(_) => Ok(()),
        }
    }

    /// Create a group, optionally nested under `group.parent_id` (which must
    /// be in the same vault).
    pub fn create_group(&mut self, group: &Group) -> Result<()> {
        self.vault_key(group.vault_id)?;
        if self.group_vault(group.id)?.is_some() {
            return Err(Error::AlreadyExists {
                kind: "group",
                id: group.id.to_string(),
            });
        }
        if let Some(parent) = group.parent_id {
            self.check_group_in_vault(parent, group.vault_id)?;
        }
        let ct = self.encode_group_settings(group)?;
        self.conn.execute(
            "INSERT INTO groups (id, vault_id, parent_id, name, settings_ct) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                group.id.to_string(),
                group.vault_id.to_string(),
                group.parent_id.map(|p| p.to_string()),
                group.name,
                ct
            ],
        )?;
        Ok(())
    }

    /// Fetch one group.
    pub fn get_group(&self, id: GroupId) -> Result<Group> {
        let row = self
            .conn
            .query_row(
                &format!("{SELECT_GROUP} WHERE id = ?1"),
                [id.to_string()],
                group_row,
            )
            .optional()?
            .ok_or_else(|| not_found("group", id))?;
        self.decode_group(row)
    }

    /// All groups of a vault (flat), ordered by name.
    pub fn list_groups(&self, vault_id: VaultId) -> Result<Vec<Group>> {
        let rows: Vec<GroupRow> = {
            let mut stmt = self.conn.prepare(&format!(
                "{SELECT_GROUP} WHERE vault_id = ?1 ORDER BY name COLLATE NOCASE, id"
            ))?;
            stmt.query_map([vault_id.to_string()], group_row)?
                .collect::<rusqlite::Result<_>>()?
        };
        rows.into_iter().map(|r| self.decode_group(r)).collect()
    }

    /// Rename, re-parent or change settings of a group. Moving a group to a
    /// different vault or under its own descendant is rejected.
    pub fn update_group(&mut self, group: &Group) -> Result<()> {
        match self.group_vault(group.id)? {
            None => return Err(not_found("group", group.id)),
            Some(v) if v != group.vault_id => {
                return Err(Error::InvalidInput(
                    "moving a group to another vault is not supported".into(),
                ));
            }
            Some(_) => {}
        }
        if let Some(parent) = group.parent_id {
            self.check_group_in_vault(parent, group.vault_id)?;
            let descendants = subtree_ids(&self.conn, &group.id.to_string())?;
            if descendants.contains(&parent.to_string()) {
                return Err(Error::InvalidInput(
                    "a group cannot be nested under itself or its descendant".into(),
                ));
            }
        }
        let ct = self.encode_group_settings(group)?;
        self.conn.execute(
            "UPDATE groups SET parent_id = ?1, name = ?2, settings_ct = ?3 WHERE id = ?4",
            params![
                group.parent_id.map(|p| p.to_string()),
                group.name,
                ct,
                group.id.to_string()
            ],
        )?;
        Ok(())
    }

    /// Delete a group. If it has sub-groups or hosts, fails with
    /// [`Error::NotEmpty`] unless `cascade` is set, in which case the whole
    /// subtree and every host in it are deleted.
    pub fn delete_group(&mut self, id: GroupId, cascade: bool) -> Result<()> {
        let sid = id.to_string();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM groups WHERE id = ?1)",
            [&sid],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(not_found("group", id));
        }
        let subtree = subtree_ids(&tx, &sid)?;
        let has_hosts: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM hosts WHERE group_id = ?1)",
            [&sid],
            |r| r.get(0),
        )?;
        if (subtree.len() > 1 || has_hosts) && !cascade {
            return Err(Error::NotEmpty {
                kind: "group",
                id: sid,
            });
        }
        tx.pragma_update(None, "defer_foreign_keys", true)?;
        for gid in &subtree {
            tx.execute("DELETE FROM hosts WHERE group_id = ?1", [gid])?;
            tx.execute("DELETE FROM groups WHERE id = ?1", [gid])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Groups from the outermost ancestor down to `group_id` itself.
    pub(crate) fn group_chain(&self, group_id: GroupId) -> Result<Vec<Group>> {
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut next = Some(group_id);
        while let Some(id) = next {
            if !seen.insert(id) {
                return Err(Error::Corrupt(format!("group cycle at {id}")));
            }
            let g = self.get_group(id)?;
            next = g.parent_id;
            chain.push(g);
        }
        chain.reverse();
        Ok(chain)
    }
}
