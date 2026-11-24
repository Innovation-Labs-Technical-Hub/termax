//! Layered connection settings.
//!
//! Each layer (App, Vault, Group chain, Host, Session) holds only the values
//! it overrides. Resolution walks from the most specific layer to the least
//! and records which layer supplied each value so the UI can show
//! inheritance badges.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Sparse map of setting key → value (TOML-like scalar strings).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Settings(pub BTreeMap<String, String>);

impl Settings {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) -> Option<String> {
        self.0.insert(key.into(), value.into())
    }

    pub fn unset(&mut self, key: &str) -> Option<String> {
        self.0.remove(key)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for Settings {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self(
            iter.into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }
}

/// Which layer a resolved value came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "layer", content = "name", rename_all = "lowercase")]
pub enum SettingSource {
    App,
    Vault,
    /// Group name; nested groups resolve innermost first.
    Group(String),
    Host,
    Session,
}

/// One layer of the inheritance chain.
#[derive(Debug, Clone)]
pub struct Layer<'a> {
    pub source: SettingSource,
    pub settings: &'a Settings,
}

impl<'a> Layer<'a> {
    pub fn new(source: SettingSource, settings: &'a Settings) -> Self {
        Self { source, settings }
    }
}

/// Fully resolved settings with per-key provenance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolved {
    values: BTreeMap<String, (String, SettingSource)>,
}

impl Resolved {
    /// Resolve `layers`, ordered least specific first (App … Session).
    /// Later layers override earlier ones.
    pub fn from_layers<'a>(layers: impl IntoIterator<Item = Layer<'a>>) -> Self {
        let mut values = BTreeMap::new();
        for layer in layers {
            for (k, v) in &layer.settings.0 {
                values.insert(k.clone(), (v.clone(), layer.source.clone()));
            }
        }
        Self { values }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(|(v, _)| v.as_str())
    }

    pub fn source(&self, key: &str) -> Option<&SettingSource> {
        self.values.get(key).map(|(_, s)| s)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str, &SettingSource)> {
        self.values
            .iter()
            .map(|(k, (v, s))| (k.as_str(), v.as_str(), s))
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_specific_layer_wins_and_provenance_is_kept() {
        let app: Settings = [("font_size", "13"), ("keepalive", "30"), ("user", "root")]
            .into_iter()
            .collect();
        let vault: Settings = [("user", "deploy")].into_iter().collect();
        let outer: Settings = [("keepalive", "10")].into_iter().collect();
        let inner: Settings = [("keepalive", "5")].into_iter().collect();
        let host: Settings = [("user", "admin")].into_iter().collect();
        let session = Settings::default();

        let r = Resolved::from_layers([
            Layer::new(SettingSource::App, &app),
            Layer::new(SettingSource::Vault, &vault),
            Layer::new(SettingSource::Group("infra".into()), &outer),
            Layer::new(SettingSource::Group("db".into()), &inner),
            Layer::new(SettingSource::Host, &host),
            Layer::new(SettingSource::Session, &session),
        ]);

        assert_eq!(r.get("user"), Some("admin"));
        assert_eq!(r.source("user"), Some(&SettingSource::Host));
        assert_eq!(r.get("keepalive"), Some("5"));
        assert_eq!(
            r.source("keepalive"),
            Some(&SettingSource::Group("db".into()))
        );
        assert_eq!(r.get("font_size"), Some("13"));
        assert_eq!(r.source("font_size"), Some(&SettingSource::App));
        assert_eq!(r.len(), 3);
        assert_eq!(r.get("missing"), None);
    }

    #[test]
    fn session_override_beats_host() {
        let host: Settings = [("term", "xterm-256color")].into_iter().collect();
        let session: Settings = [("term", "vt100")].into_iter().collect();
        let r = Resolved::from_layers([
            Layer::new(SettingSource::Host, &host),
            Layer::new(SettingSource::Session, &session),
        ]);
        assert_eq!(r.get("term"), Some("vt100"));
        assert_eq!(r.source("term"), Some(&SettingSource::Session));
    }
}
