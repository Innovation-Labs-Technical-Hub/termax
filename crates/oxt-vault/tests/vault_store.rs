use std::path::{Path, PathBuf};

use oxt_core::{Env, Group, Host, Protocol, SettingSource, Settings, Vault, VaultId, VaultKind};
use oxt_vault::{
    Error, HostFilter, KdfParams, KnownHost, SCHEMA_VERSION, SecretKey, VaultStore,
    generate_secret_key,
};
use tempfile::TempDir;

const PW: &str = "correct horse battery staple";

fn cheap() -> KdfParams {
    KdfParams::new(64, 1, 1).unwrap()
}

struct Fixture {
    _dir: TempDir,
    path: PathBuf,
    sk: SecretKey,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oxterm.db");
        Self {
            _dir: dir,
            path,
            sk: generate_secret_key().unwrap(),
        }
    }

    fn create(&self) -> VaultStore {
        VaultStore::create_with_params(&self.path, PW, &self.sk, cheap()).unwrap()
    }

    fn open(&self) -> VaultStore {
        VaultStore::open(&self.path, PW, &self.sk).unwrap()
    }

    fn raw(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(&self.path).unwrap()
    }
}

fn vault(name: &str) -> Vault {
    Vault {
        id: VaultId::new(),
        name: name.into(),
        kind: VaultKind::Personal,
        settings: Settings::default(),
    }
}

fn group(v: &Vault, parent: Option<&Group>, name: &str, settings: Settings) -> Group {
    Group {
        id: Default::default(),
        vault_id: v.id,
        parent_id: parent.map(|p| p.id),
        name: name.into(),
        settings,
    }
}

fn settings(pairs: &[(&str, &str)]) -> Settings {
    pairs.iter().copied().collect()
}

fn read_all(path: &Path) -> Vec<u8> {
    let mut bytes = std::fs::read(path).unwrap();
    for suffix in ["-wal", "-shm"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        if let Ok(b) = std::fs::read(PathBuf::from(p)) {
            bytes.extend(b);
        }
    }
    bytes
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

#[test]
fn create_and_reopen_round_trip() {
    let fx = Fixture::new();
    let v = vault("Personal");
    let mut h = Host::new(v.id, "web", "web.example.com");
    h.port = Some(2222);
    h.protocol = Protocol::Mosh;
    h.env = Env::Staging;
    h.tags = vec!["frontend".into()];
    h.settings = settings(&[("user", "deploy")]);
    {
        let mut s = fx.create();
        s.create_vault(&v).unwrap();
        s.create_host(&h).unwrap();
    }
    let s = fx.open();
    assert_eq!(s.list_vaults().unwrap(), vec![v.clone()]);
    assert_eq!(s.get_host(h.id).unwrap(), h);
    assert_eq!(s.kdf_params(), cheap());

    let mut s = s;
    h.address = "web2.example.com".into();
    s.update_host(&h).unwrap();
    assert_eq!(s.get_host(h.id).unwrap().address, "web2.example.com");
    s.delete_host(h.id).unwrap();
    assert!(matches!(s.get_host(h.id), Err(Error::NotFound { .. })));
}

#[test]
fn wrong_password_or_secret_key_fails_closed() {
    let fx = Fixture::new();
    drop(fx.create());
    assert!(matches!(
        VaultStore::open(&fx.path, "nope", &fx.sk),
        Err(Error::WrongCredentials)
    ));
    let other = generate_secret_key().unwrap();
    assert!(matches!(
        VaultStore::open(&fx.path, PW, &other),
        Err(Error::WrongCredentials)
    ));
    assert!(VaultStore::open(&fx.path, PW, &fx.sk).is_ok());
}

#[test]
fn lifecycle_errors_are_distinct_from_bad_credentials() {
    let fx = Fixture::new();
    assert!(matches!(
        VaultStore::open(&fx.path, PW, &fx.sk),
        Err(Error::NotInitialized)
    ));
    drop(fx.create());
    assert!(matches!(
        VaultStore::create_with_params(&fx.path, PW, &fx.sk, cheap()),
        Err(Error::AlreadyInitialized)
    ));

    let junk = fx.path.with_file_name("junk.db");
    std::fs::write(&junk, vec![0x5a; 8192]).unwrap();
    assert!(matches!(
        VaultStore::open(&junk, PW, &fx.sk),
        Err(Error::Corrupt(_))
    ));
}

#[test]
fn no_plaintext_secrets_on_disk() {
    let fx = Fixture::new();
    let mut s = fx.create();
    let mut v = vault("Work");
    v.settings = settings(&[("vault_secret", "hunter2")]);
    s.create_vault(&v).unwrap();
    let g = group(&v, None, "db", settings(&[("group_secret", "hunter2")]));
    s.create_group(&g).unwrap();
    let mut h = Host::new(v.id, "primary", "secret-db.internal");
    h.group_id = Some(g.id);
    h.tags = vec!["tag-classified".into()];
    h.settings = settings(&[("password", "hunter2")]);
    s.create_host(&h).unwrap();
    s.upsert_known_host(&KnownHost::new(
        "secret-db.internal:22",
        "ssh-ed25519",
        "SHA256:fingerprint-classified",
        "tofu",
    ))
    .unwrap();

    let needles = [
        "secret-db.internal",
        "hunter2",
        "tag-classified",
        "fingerprint-classified",
    ];
    // While open, data sits in the WAL.
    let bytes = read_all(&fx.path);
    assert!(contains(&bytes, "primary"), "sanity: label is plaintext");
    for n in needles {
        assert!(!contains(&bytes, n), "`{n}` found on disk (open)");
    }
    // After close, the WAL is checkpointed into the main file.
    drop(s);
    let bytes = read_all(&fx.path);
    for n in needles {
        assert!(!contains(&bytes, n), "`{n}` found on disk (closed)");
    }
    assert_eq!(fx.open().get_host(h.id).unwrap(), h);
}

#[test]
fn swapping_ciphertexts_between_rows_is_detected() {
    let fx = Fixture::new();
    let v = vault("V");
    let a = Host::new(v.id, "a", "a.internal");
    let b = Host::new(v.id, "b", "b.internal");
    {
        let mut s = fx.create();
        s.create_vault(&v).unwrap();
        s.create_host(&a).unwrap();
        s.create_host(&b).unwrap();
    }
    {
        let raw = fx.raw();
        let ct = |id: String| -> Vec<u8> {
            raw.query_row("SELECT settings_ct FROM hosts WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
        };
        let (ca, cb) = (ct(a.id.to_string()), ct(b.id.to_string()));
        raw.execute(
            "UPDATE hosts SET settings_ct = ?1 WHERE id = ?2",
            rusqlite::params![cb, a.id.to_string()],
        )
        .unwrap();
        raw.execute(
            "UPDATE hosts SET settings_ct = ?1 WHERE id = ?2",
            rusqlite::params![ca, b.id.to_string()],
        )
        .unwrap();
    }
    let s = fx.open();
    assert!(matches!(s.get_host(a.id), Err(Error::Corrupt(_))));
    assert!(matches!(s.get_host(b.id), Err(Error::Corrupt(_))));
}

#[test]
fn tampering_with_plaintext_index_columns_is_detected() {
    let fx = Fixture::new();
    let v = vault("V");
    let mut h = Host::new(v.id, "prod-db", "db.internal");
    h.env = Env::Prod;
    {
        let mut s = fx.create();
        s.create_vault(&v).unwrap();
        s.create_host(&h).unwrap();
    }
    fx.raw()
        .execute(
            "UPDATE hosts SET env = 'dev' WHERE id = ?1",
            [h.id.to_string()],
        )
        .unwrap();
    assert!(matches!(fx.open().get_host(h.id), Err(Error::Corrupt(_))));
}

#[test]
fn migrations_are_idempotent_on_reopen() {
    let fx = Fixture::new();
    let v = vault("V");
    {
        let mut s = fx.create();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
        s.create_vault(&v).unwrap();
    }
    for _ in 0..3 {
        let s = fx.open();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(s.list_vaults().unwrap().len(), 1);
    }
    let journal: String = fx
        .raw()
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(journal, "wal");
}

#[test]
fn newer_schema_is_rejected() {
    let fx = Fixture::new();
    drop(fx.create());
    fx.raw()
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .unwrap();
    assert!(matches!(
        VaultStore::open(&fx.path, PW, &fx.sk),
        Err(Error::SchemaTooNew { .. })
    ));
}

#[test]
fn change_password_rewraps_keys() {
    let fx = Fixture::new();
    let v = vault("V");
    let h = Host::new(v.id, "h", "h.internal");
    let kh = KnownHost::new("h.internal:22", "ssh-ed25519", "SHA256:x", "tofu");
    {
        let mut s = fx.create();
        s.create_vault(&v).unwrap();
        s.create_host(&h).unwrap();
        s.upsert_known_host(&kh).unwrap();
        assert!(matches!(
            s.change_password("wrong", "new-pass"),
            Err(Error::WrongCredentials)
        ));
        let stronger = KdfParams::new(128, 2, 1).unwrap();
        s.change_password_with_params(PW, "new-pass", stronger)
            .unwrap();
        // Still usable in the same session, including new vaults.
        assert_eq!(s.get_host(h.id).unwrap(), h);
        s.create_vault(&vault("After")).unwrap();
    }
    assert!(matches!(
        VaultStore::open(&fx.path, PW, &fx.sk),
        Err(Error::WrongCredentials)
    ));
    let s = VaultStore::open(&fx.path, "new-pass", &fx.sk).unwrap();
    assert_eq!(s.kdf_params(), KdfParams::new(128, 2, 1).unwrap());
    assert_eq!(s.get_host(h.id).unwrap(), h);
    assert_eq!(s.list_vaults().unwrap().len(), 2);
    assert_eq!(
        s.get_known_host("h.internal:22", "ssh-ed25519").unwrap(),
        Some(kh)
    );
}

#[test]
fn resolved_settings_walk_nested_groups_outermost_first() {
    let fx = Fixture::new();
    let mut s = fx.create();
    let mut v = vault("V");
    v.settings = settings(&[("user", "vault-user"), ("keepalive", "60")]);
    s.create_vault(&v).unwrap();
    let outer = group(
        &v,
        None,
        "infra",
        settings(&[("keepalive", "10"), ("term", "xterm")]),
    );
    let inner = group(&v, Some(&outer), "db", settings(&[("keepalive", "5")]));
    s.create_group(&outer).unwrap();
    s.create_group(&inner).unwrap();
    let mut h = Host::new(v.id, "pg", "pg.internal");
    h.group_id = Some(inner.id);
    h.settings = settings(&[("user", "postgres")]);
    s.create_host(&h).unwrap();

    let app = settings(&[("font_size", "13"), ("user", "root"), ("term", "vt100")]);
    let session = settings(&[("font_size", "16")]);
    let r = s.resolved_settings(h.id, &app, &session).unwrap();

    assert_eq!(r.get("keepalive"), Some("5"));
    assert_eq!(
        r.source("keepalive"),
        Some(&SettingSource::Group("db".into()))
    );
    assert_eq!(r.get("term"), Some("xterm"));
    assert_eq!(
        r.source("term"),
        Some(&SettingSource::Group("infra".into()))
    );
    assert_eq!(r.get("user"), Some("postgres"));
    assert_eq!(r.source("user"), Some(&SettingSource::Host));
    assert_eq!(r.get("font_size"), Some("16"));
    assert_eq!(r.source("font_size"), Some(&SettingSource::Session));
    assert_eq!(r.len(), 4);
}

#[test]
fn list_hosts_filters() {
    let fx = Fixture::new();
    let mut s = fx.create();
    let v = vault("V");
    let other = vault("Other");
    s.create_vault(&v).unwrap();
    s.create_vault(&other).unwrap();
    let g = group(&v, None, "infra", Settings::default());
    let sub = group(&v, Some(&g), "db", Settings::default());
    s.create_group(&g).unwrap();
    s.create_group(&sub).unwrap();

    let mut web = Host::new(v.id, "Web-1", "web1.example.com");
    web.env = Env::Prod;
    web.tags = vec!["Frontend".into()];
    web.group_id = Some(g.id);
    let mut db = Host::new(v.id, "db-1", "pg.internal");
    db.env = Env::Prod;
    db.tags = vec!["backend".into()];
    db.group_id = Some(sub.id);
    let mut dev = Host::new(v.id, "dev-box", "dev.example.com");
    dev.env = Env::Dev;
    let elsewhere = Host::new(other.id, "web-other", "web.example.com");
    for h in [&web, &db, &dev, &elsewhere] {
        s.create_host(h).unwrap();
    }

    let labels = |f: HostFilter| -> Vec<String> {
        s.list_hosts(v.id, &f)
            .unwrap()
            .into_iter()
            .map(|h| h.label)
            .collect()
    };
    assert_eq!(labels(HostFilter::default()), ["db-1", "dev-box", "Web-1"]);
    assert_eq!(
        labels(HostFilter {
            text: Some("EXAMPLE".into()),
            ..Default::default()
        }),
        ["dev-box", "Web-1"]
    );
    assert_eq!(
        labels(HostFilter {
            text: Some("db".into()),
            ..Default::default()
        }),
        ["db-1"]
    );
    assert_eq!(
        labels(HostFilter {
            tag: Some("frontend".into()),
            ..Default::default()
        }),
        ["Web-1"]
    );
    assert_eq!(
        labels(HostFilter {
            env: Some(Env::Prod),
            ..Default::default()
        }),
        ["db-1", "Web-1"]
    );
    assert_eq!(
        labels(HostFilter {
            group: Some(g.id),
            ..Default::default()
        }),
        ["Web-1"]
    );
    assert_eq!(
        labels(HostFilter {
            group: Some(g.id),
            include_subgroups: true,
            ..Default::default()
        }),
        ["db-1", "Web-1"]
    );
    assert_eq!(
        labels(HostFilter {
            env: Some(Env::Prod),
            tag: Some("backend".into()),
            text: Some("pg".into()),
            ..Default::default()
        }),
        ["db-1"]
    );
}

#[test]
fn group_delete_requires_cascade_when_not_empty() {
    let fx = Fixture::new();
    let mut s = fx.create();
    let v = vault("V");
    s.create_vault(&v).unwrap();
    let parent = group(&v, None, "parent", Settings::default());
    let child = group(&v, Some(&parent), "child", Settings::default());
    let empty = group(&v, None, "empty", Settings::default());
    for g in [&parent, &child, &empty] {
        s.create_group(g).unwrap();
    }
    let mut h = Host::new(v.id, "h", "h.internal");
    h.group_id = Some(child.id);
    s.create_host(&h).unwrap();

    assert!(matches!(
        s.delete_group(child.id, false),
        Err(Error::NotEmpty { .. })
    ));
    assert!(matches!(
        s.delete_group(parent.id, false),
        Err(Error::NotEmpty { .. })
    ));
    assert!(s.get_host(h.id).is_ok());

    s.delete_group(empty.id, false).unwrap();
    s.delete_group(parent.id, true).unwrap();
    assert!(s.list_groups(v.id).unwrap().is_empty());
    assert!(matches!(s.get_host(h.id), Err(Error::NotFound { .. })));

    let g = group(&v, None, "g", Settings::default());
    s.create_group(&g).unwrap();
    assert!(matches!(
        s.delete_vault(v.id, false),
        Err(Error::NotEmpty { .. })
    ));
    s.delete_vault(v.id, true).unwrap();
    assert!(s.list_vaults().unwrap().is_empty());
}

#[test]
fn group_and_host_references_are_validated() {
    let fx = Fixture::new();
    let mut s = fx.create();
    let v = vault("V");
    let w = vault("W");
    s.create_vault(&v).unwrap();
    s.create_vault(&w).unwrap();
    let a = group(&v, None, "a", Settings::default());
    let b = group(&v, Some(&a), "b", Settings::default());
    s.create_group(&a).unwrap();
    s.create_group(&b).unwrap();

    // Cycle: a under its own child.
    let mut moved = s.get_group(a.id).unwrap();
    moved.parent_id = Some(b.id);
    assert!(matches!(
        s.update_group(&moved),
        Err(Error::InvalidInput(_))
    ));
    // Cross-vault parent and cross-vault host group.
    let foreign = group(&w, Some(&a), "x", Settings::default());
    assert!(matches!(
        s.create_group(&foreign),
        Err(Error::InvalidInput(_))
    ));
    let mut h = Host::new(w.id, "h", "h.internal");
    h.group_id = Some(a.id);
    assert!(matches!(s.create_host(&h), Err(Error::InvalidInput(_))));

    // Rename + re-parent to root works and survives reopen.
    let mut renamed = s.get_group(b.id).unwrap();
    renamed.name = "b2".into();
    renamed.parent_id = None;
    renamed.settings = settings(&[("k", "v")]);
    s.update_group(&renamed).unwrap();
    drop(s);
    assert_eq!(fx.open().get_group(b.id).unwrap(), renamed);
}

#[test]
fn known_host_upsert_keeps_first_seen() {
    let fx = Fixture::new();
    let mut s = fx.create();
    let mut kh = KnownHost::new("db.internal:22", "ssh-ed25519", "SHA256:old", "tofu");
    kh.first_seen = 1_000;
    assert_eq!(s.upsert_known_host(&kh).unwrap().first_seen, 1_000);

    let mut rotated = kh.clone();
    rotated.fp_sha256 = "SHA256:new".into();
    rotated.source = "manual".into();
    rotated.first_seen = 2_000;
    let stored = s.upsert_known_host(&rotated).unwrap();
    assert_eq!(stored.first_seen, 1_000);
    assert_eq!(stored.fp_sha256, "SHA256:new");

    let rsa = KnownHost {
        key_type: "rsa-sha2-512".into(),
        first_seen: 3_000,
        ..kh.clone()
    };
    s.upsert_known_host(&rsa).unwrap();
    drop(s);

    let s = fx.open();
    let got = s
        .get_known_host("db.internal:22", "ssh-ed25519")
        .unwrap()
        .unwrap();
    assert_eq!(got, stored);
    assert_eq!(
        s.get_known_host("db.internal:2222", "ssh-ed25519").unwrap(),
        None
    );
    let all = s.list_known_hosts().unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].key_type, "rsa-sha2-512");
    assert_eq!(all[0].first_seen, 3_000);
}
