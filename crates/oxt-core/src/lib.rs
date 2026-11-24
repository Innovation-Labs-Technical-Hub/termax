//! OxTerm domain model: vaults, groups, hosts and the layered settings
//! resolver (App → Vault → Group → Host → Session).

pub mod model;
pub mod settings;

pub use model::{Env, Group, GroupId, Host, HostId, Protocol, Vault, VaultId, VaultKind};
pub use settings::{Layer, Resolved, SettingSource, Settings};
