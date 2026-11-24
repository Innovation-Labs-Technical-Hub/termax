use std::fs;
use std::path::{Path, PathBuf};

use oxt_core::{Host, VaultId};
use oxt_interop::{MULTI_VALUE_SEPARATOR, ParseOptions, SshConfig, to_ssh_config};

fn fixture_home() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/home")
}

fn fixture_opts() -> ParseOptions {
    ParseOptions::new(fixture_home(), "alice")
}

fn fixture_config() -> SshConfig {
    let opts = fixture_opts();
    SshConfig::parse_file(&opts.ssh_dir.join("config"), &opts).expect("fixture config readable")
}

/// Options for string-only tests; the home does not exist so no Include
/// can reach the real filesystem.
fn str_opts() -> ParseOptions {
    ParseOptions::new("/nonexistent-oxt-home", "alice")
}

fn warning_messages(cfg: &SshConfig) -> Vec<String> {
    cfg.warnings().iter().map(|w| w.message.clone()).collect()
}

fn find<'a>(hosts: &'a [Host], label: &str) -> &'a Host {
    hosts
        .iter()
        .find(|h| h.label == label)
        .unwrap_or_else(|| panic!("host {label} not imported"))
}

#[test]
fn include_glob_skips_hidden_and_non_matching_files() {
    let cfg = fixture_config();
    assert_eq!(cfg.resolve("work").hostname(), "work.corp");
    assert_eq!(cfg.resolve("work").user(), "me");
    // `.hidden.conf` is not matched by `*.conf`, `readme.txt` not at all.
    assert_eq!(cfg.resolve("hidden").hostname(), "hidden");
    let labels: Vec<_> = cfg
        .import(VaultId::new())
        .hosts
        .into_iter()
        .map(|h| h.label)
        .collect();
    assert!(!labels.iter().any(|l| l == "hidden" || l == "not-a-conf"));
}

#[test]
fn include_cycles_are_reported_not_followed() {
    let cfg = fixture_config();
    let cycles: Vec<_> = warning_messages(&cfg)
        .into_iter()
        .filter(|m| m.contains("Include cycle"))
        .collect();
    assert_eq!(cycles.len(), 2, "{cycles:?}");
    assert!(cycles[0].contains("loop.conf"));
    assert!(cycles[1].contains("config"));
}

#[test]
fn include_inside_host_block_is_gated_by_that_block() {
    let cfg = fixture_config();
    let other = cfg.resolve("other");
    assert_eq!(other.port(), Some(2022));
    // `User` after the Include still belongs to `Host other`.
    assert_eq!(other.user(), "other-user");
    // `Host nested` lives in a file included under `Host other`, so it can
    // never match an alias other than `other`.
    let nested = cfg.resolve("nested");
    assert_eq!(nested.hostname(), "nested");
    assert_eq!(nested.port(), Some(22));
}

#[test]
fn include_depth_is_limited() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..20 {
        fs::write(
            dir.path().join(format!("f{i}.conf")),
            format!("Include f{}.conf\nHost h{i}\n    User u{i}\n", i + 1),
        )
        .unwrap();
    }
    let mut opts = ParseOptions::new(dir.path(), "alice");
    opts.ssh_dir = dir.path().to_path_buf();
    let cfg = SshConfig::parse_file(&dir.path().join("f0.conf"), &opts).unwrap();
    let report = cfg.import(VaultId::new());
    // f0 is depth 0; f1..=f16 are within the limit, f17 is refused.
    assert_eq!(report.hosts.len(), 17);
    assert!(
        warning_messages(&cfg)
            .iter()
            .any(|m| m.contains("maximum depth of 16") && m.contains("f17.conf"))
    );
}

#[test]
fn parse_str_resolves_includes_against_injected_ssh_dir() {
    let cfg = SshConfig::parse_str("Include conf.d/10-*.conf\n", &fixture_opts());
    assert!(cfg.warnings().is_empty(), "{:?}", cfg.warnings());
    assert_eq!(cfg.resolve("work").hostname(), "work.corp");
}

#[test]
fn match_host_uses_hostname_and_exec_is_never_run() {
    let cfg = fixture_config();
    assert!(
        warning_messages(&cfg)
            .iter()
            .any(|m| m.contains("`exec` is not supported"))
    );
    assert!(!Path::new("/tmp/oxt-interop-must-not-run").exists());

    let web1 = cfg.resolve("web1");
    assert_eq!(web1.get("serveraliveinterval"), Some("30"));
    assert_ne!(web1.user(), "hacked");
    // web3.internal does not match `*.example.com`.
    assert_eq!(cfg.resolve("web3").get("serveraliveinterval"), None);
    // `user deploy` is not satisfied for legacy.
    assert_eq!(cfg.resolve("legacy").get("serveraliveinterval"), None);
    assert_eq!(cfg.resolve("db").get("compression"), Some("yes"));
}

#[test]
fn resolve_matches_ssh_g_semantics() {
    let cfg = fixture_config();
    let home = fixture_home();
    let home = home.to_str().unwrap();

    let web1 = cfg.resolve("web1");
    assert_eq!(web1.hostname(), "web1.example.com");
    assert_eq!(web1.user(), "deploy");
    assert_eq!(web1.port(), Some(2201));
    assert_eq!(web1.get("forwardagent"), Some("no"));
    assert_eq!(web1.get("DynamicForward"), Some("1080"));

    let db = cfg.resolve("db");
    assert_eq!(db.port(), Some(2222), "first obtained value wins");
    assert_eq!(db.user(), "fallback");
    assert_eq!(db.get("proxyjump"), Some("bastion"));
    assert_eq!(
        db.all("localforward"),
        &[
            vec!["5432".to_string(), "localhost:5432".to_string()],
            vec!["6379".to_string(), "localhost:6379".to_string()],
        ]
    );
    // `~` and `%r` (effective remote user) are expanded, in order obtained.
    assert_eq!(
        db.all("identityfile"),
        &[
            vec![format!("{home}/.ssh/db_fallback")],
            vec![format!("{home}/.ssh/id_ed25519")],
        ]
    );

    let legacy = cfg.resolve("legacy");
    assert_eq!(
        legacy.get("proxycommand"),
        Some("ssh -W legacy.example.com:22 bastion")
    );

    let unknown = cfg.resolve("unlisted.example.org");
    assert_eq!(unknown.hostname(), "unlisted.example.org");
    assert_eq!(unknown.user(), "fallback");
    assert_eq!(unknown.port(), Some(22));
}

#[test]
fn negated_patterns_exclude_hosts() {
    let cfg = fixture_config();
    let web3 = cfg.resolve("web3");
    assert_eq!(web3.hostname(), "web3.internal");
    assert_eq!(web3.user(), "fallback");
    assert_eq!(web3.port(), Some(22));
    assert_eq!(cfg.resolve("web2").hostname(), "web2.example.com");
}

#[test]
fn keyword_case_equals_syntax_and_quotes() {
    let cfg = SshConfig::parse_str(
        concat!(
            "HOST alpha beta\n",
            "  hostname=alpha.example.com\n",
            "  Port = 2200\n",
            "  user =\"bob smith\"\n",
            "  IdentityFile '~/my keys/id' # trailing comment\n",
            "  SendEnv LANG LC_*\n",
        ),
        &str_opts(),
    );
    assert!(cfg.warnings().is_empty(), "{:?}", cfg.warnings());
    let r = cfg.resolve("ALPHA");
    assert_eq!(r.hostname(), "alpha.example.com");
    assert_eq!(r.port(), Some(2200));
    assert_eq!(r.user(), "bob smith");
    assert_eq!(
        r.get("identityfile"),
        Some("/nonexistent-oxt-home/my keys/id")
    );
    assert_eq!(
        r.args("sendenv"),
        Some(&["LANG".to_string(), "LC_*".to_string()][..])
    );
}

#[test]
fn token_expansion() {
    let cfg = SshConfig::parse_str(
        concat!(
            "Host t\n",
            "  HostName %h.lan\n",
            "  User root\n",
            "  Port 2022\n",
            "  IdentityFile %d/.ssh/%u-%r@%h:%p-%n-100%%\n",
            "  ControlPath ~/.ssh/cm-%C\n",
            "  ProxyCommand nc %h %p\n",
        ),
        &str_opts(),
    );
    let r = cfg.resolve("t");
    assert_eq!(r.hostname(), "t.lan");
    assert_eq!(
        r.get("identityfile"),
        Some("/nonexistent-oxt-home/.ssh/alice-root@t.lan:2022-t-100%")
    );
    assert_eq!(r.get("proxycommand"), Some("nc t.lan 2022"));
    // Unsupported tokens are kept verbatim and reported.
    assert_eq!(
        r.get("controlpath"),
        Some("/nonexistent-oxt-home/.ssh/cm-%C")
    );
    assert_eq!(r.warnings.len(), 1);
    assert!(r.warnings[0].message.contains("%C"));
}

#[test]
fn import_maps_hosts_and_reports_skips() {
    let cfg = fixture_config();
    let vault = VaultId::new();
    let report = cfg.import(vault);

    let labels: Vec<_> = report.hosts.iter().map(|h| h.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "work",
            "other",
            "nested",
            "web1",
            "web2",
            "web3",
            "quoted alias",
            "bastion",
            "db",
            "legacy"
        ]
    );
    let skipped: Vec<_> = report
        .skipped
        .iter()
        .map(|s| (s.pattern.as_str(), s.reason.as_str()))
        .collect();
    assert_eq!(
        skipped,
        [("!web3", "negated pattern"), ("*", "wildcard pattern")]
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.message.contains("UnknownThing"))
    );

    let web1 = find(&report.hosts, "web1");
    assert_eq!(web1.vault_id, vault);
    assert_eq!(web1.address, "web1.example.com");
    assert_eq!(web1.port, Some(2201));
    assert_eq!(web1.settings.get("user"), Some("deploy"));
    assert_eq!(web1.settings.get("serveraliveinterval"), Some("30"));
    assert_eq!(web1.settings.get("hostname"), None);
    assert_eq!(web1.settings.get("port"), None);

    let db = find(&report.hosts, "db");
    assert_eq!(db.address, "10.1.2.3");
    assert_eq!(db.port, Some(2222));
    assert_eq!(db.settings.get("proxyjump"), Some("bastion"));
    assert_eq!(db.settings.get("forwardagent"), Some("no"));
    assert_eq!(db.settings.get("dynamicforward"), Some("1080"));
    assert_eq!(db.settings.get("compression"), Some("yes"));
    let sep = MULTI_VALUE_SEPARATOR;
    // Stored unexpanded so the values remain portable.
    assert_eq!(
        db.settings.get("identityfile"),
        Some(format!("~/.ssh/db_%r{sep}~/.ssh/id_ed25519").as_str())
    );
    assert_eq!(
        db.settings.get("localforward"),
        Some(format!("5432 localhost:5432{sep}6379 localhost:6379").as_str())
    );
    assert_eq!(
        oxt_interop::decode_setting("localforward", db.settings.get("localforward").unwrap())
            .unwrap()[1],
        ["6379", "localhost:6379"]
    );

    let bastion = find(&report.hosts, "bastion");
    assert_eq!(bastion.address, "bastion.example.com");
    assert_eq!(bastion.port, None);
    assert_eq!(bastion.settings.get("user"), Some("jump"));
    assert_eq!(bastion.settings.get("forwardagent"), Some("yes"));

    let legacy = find(&report.hosts, "legacy");
    assert_eq!(
        legacy.settings.get("proxycommand"),
        Some("ssh -W %h:%p bastion")
    );

    let quoted = find(&report.hosts, "quoted alias");
    assert_eq!(quoted.address, "10.0.0.5");
    assert_eq!(
        quoted.settings.get("identityfile"),
        Some(format!("\"~/.ssh/key with space\"{sep}~/.ssh/id_ed25519").as_str())
    );
}

#[test]
fn import_reports_invalid_port() {
    let cfg = SshConfig::parse_str("Host p\n  Port http\n", &str_opts());
    let report = cfg.import(VaultId::new());
    assert_eq!(report.hosts[0].port, None);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.message.contains("invalid Port"))
    );
}

/// Label, address, port and settings: everything an export can carry.
type Comparable = (String, String, Option<u16>, Vec<(String, String)>);

fn comparable(hosts: &[Host]) -> Vec<Comparable> {
    hosts
        .iter()
        .map(|h| {
            (
                h.label.clone(),
                h.address.clone(),
                h.port,
                h.settings
                    .0
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn export_round_trips_through_the_parser() {
    let imported = fixture_config().import(VaultId::new()).hosts;
    let text = to_ssh_config(&imported);
    let reparsed = SshConfig::parse_str(&text, &str_opts());
    assert!(
        reparsed.warnings().is_empty(),
        "{:?}\n{text}",
        reparsed.warnings()
    );
    let report = reparsed.import(VaultId::new());
    assert!(report.skipped.is_empty());
    assert_eq!(comparable(&report.hosts), comparable(&imported), "{text}");
}

#[test]
fn export_round_trips_hand_built_hosts() {
    let vault = VaultId::new();
    let mut a = Host::new(vault, "app", "app%1.example.com");
    a.port = Some(2222);
    a.settings.set("user", "ops");
    a.settings
        .set("proxycommand", "ssh -W \"%h:%p\" jump # raw");
    a.settings
        .set("identityfile", "\"C:\\\\keys\\\\id rsa\"\n~/.ssh/id");
    a.settings.set("setenv", "\"FOO=a b\" BAR=1");
    let b = Host::new(vault, "plain", "plain");
    let hosts = [a, b];
    let text = to_ssh_config(&hosts);
    let report = SshConfig::parse_str(&text, &str_opts()).import(VaultId::new());
    assert_eq!(comparable(&report.hosts), comparable(&hosts), "{text}");
    assert!(text.contains("HostName app%%1.example.com"));
    assert!(!text.contains("Host plain\n    HostName"));
}
