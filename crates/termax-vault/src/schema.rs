//! Schema versioning.
//!
//! The schema version lives in `PRAGMA user_version`. [`MIGRATIONS`] is an
//! append-only list: entry `i` upgrades version `i` to `i + 1`. To evolve the
//! schema, append a new SQL batch; never edit a released one. All pending
//! migrations and the version bump run in a single transaction.

use rusqlite::{Connection, TransactionBehavior};

use crate::error::{Error, Result};

const V1: &str = r#"
CREATE TABLE meta (
    id                INTEGER PRIMARY KEY CHECK (id = 1),
    kdf_algorithm     TEXT    NOT NULL,
    kdf_m_cost_kib    INTEGER NOT NULL,
    kdf_t_cost        INTEGER NOT NULL,
    kdf_p_cost        INTEGER NOT NULL,
    kdf_salt          BLOB    NOT NULL,
    wrapped_store_key BLOB    NOT NULL
);

CREATE TABLE vaults (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL,
    wrapped_key BLOB NOT NULL,
    settings_ct BLOB NOT NULL
);

CREATE TABLE groups (
    id          TEXT PRIMARY KEY,
    vault_id    TEXT NOT NULL REFERENCES vaults(id),
    parent_id   TEXT REFERENCES groups(id),
    name        TEXT NOT NULL,
    settings_ct BLOB NOT NULL
);
CREATE INDEX groups_vault ON groups(vault_id);
CREATE INDEX groups_parent ON groups(parent_id);

CREATE TABLE hosts (
    id          TEXT PRIMARY KEY,
    vault_id    TEXT NOT NULL REFERENCES vaults(id),
    group_id    TEXT REFERENCES groups(id),
    label       TEXT NOT NULL,
    protocol    TEXT NOT NULL,
    env         TEXT NOT NULL,
    settings_ct BLOB NOT NULL
);
CREATE INDEX hosts_vault ON hosts(vault_id);
CREATE INDEX hosts_group ON hosts(group_id);

CREATE TABLE known_hosts (
    host_port_mac TEXT    NOT NULL,
    key_type      TEXT    NOT NULL,
    first_seen    INTEGER NOT NULL,
    record_ct     BLOB    NOT NULL,
    PRIMARY KEY (host_port_mac, key_type)
) WITHOUT ROWID;
"#;

const MIGRATIONS: &[&str] = &[V1];

/// Schema version written by this build.
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

/// Bring the database up to [`SCHEMA_VERSION`]. A no-op when already current.
pub(crate) fn migrate(conn: &mut Connection) -> Result<u32> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: u32 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if current > SCHEMA_VERSION {
        return Err(Error::SchemaTooNew {
            found: current,
            supported: SCHEMA_VERSION,
        });
    }
    if current == SCHEMA_VERSION {
        return Ok(current);
    }
    for sql in &MIGRATIONS[current as usize..] {
        tx.execute_batch(sql)?;
    }
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(SCHEMA_VERSION)
}

/// Current `user_version` of an open connection.
pub(crate) fn version(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}
