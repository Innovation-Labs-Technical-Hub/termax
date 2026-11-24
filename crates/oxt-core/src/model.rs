use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::settings::Settings;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    };
}

id_type!(VaultId);
id_type!(GroupId);
id_type!(HostId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VaultKind {
    Personal,
    Team,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vault {
    pub id: VaultId,
    pub name: String,
    pub kind: VaultKind,
    #[serde(default)]
    pub settings: Settings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub id: GroupId,
    pub vault_id: VaultId,
    pub parent_id: Option<GroupId>,
    pub name: String,
    #[serde(default)]
    pub settings: Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Ssh,
    Telnet,
    Serial,
    Raw,
    Local,
    Mosh,
}

impl Protocol {
    pub fn default_port(self) -> Option<u16> {
        match self {
            Protocol::Ssh | Protocol::Mosh => Some(22),
            Protocol::Telnet => Some(23),
            Protocol::Serial | Protocol::Raw | Protocol::Local => None,
        }
    }
}

/// Environment classification. `Prod` drives safety behaviour (e.g. agent
/// commands on prod hosts always require approval).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Env {
    #[default]
    None,
    Dev,
    Staging,
    Prod,
}

impl FromStr for Env {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "" | "none" => Ok(Env::None),
            "dev" | "development" => Ok(Env::Dev),
            "staging" | "stage" => Ok(Env::Staging),
            "prod" | "production" => Ok(Env::Prod),
            other => Err(format!("unknown environment `{other}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Host {
    pub id: HostId,
    pub vault_id: VaultId,
    pub group_id: Option<GroupId>,
    pub label: String,
    pub address: String,
    pub port: Option<u16>,
    #[serde(default)]
    pub protocol: Protocol,
    #[serde(default)]
    pub env: Env,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub settings: Settings,
}

impl Host {
    pub fn new(vault_id: VaultId, label: impl Into<String>, address: impl Into<String>) -> Self {
        Self {
            id: HostId::new(),
            vault_id,
            group_id: None,
            label: label.into(),
            address: address.into(),
            port: None,
            protocol: Protocol::Ssh,
            env: Env::None,
            tags: Vec::new(),
            settings: Settings::default(),
        }
    }

    /// Port to connect to: explicit port, else the protocol default.
    pub fn effective_port(&self) -> Option<u16> {
        self.port.or_else(|| self.protocol.default_port())
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t.eq_ignore_ascii_case(tag))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_port_falls_back_to_protocol_default() {
        let mut h = Host::new(VaultId::new(), "web", "web.example.com");
        assert_eq!(h.effective_port(), Some(22));
        h.protocol = Protocol::Telnet;
        assert_eq!(h.effective_port(), Some(23));
        h.port = Some(2222);
        assert_eq!(h.effective_port(), Some(2222));
        h.protocol = Protocol::Serial;
        h.port = None;
        assert_eq!(h.effective_port(), None);
    }

    #[test]
    fn env_parses_aliases() {
        assert_eq!("Production".parse::<Env>().unwrap(), Env::Prod);
        assert_eq!("stage".parse::<Env>().unwrap(), Env::Staging);
        assert!("qa".parse::<Env>().is_err());
    }

    #[test]
    fn ids_round_trip_through_strings() {
        let id = HostId::new();
        assert_eq!(id.to_string().parse::<HostId>().unwrap(), id);
    }
}
