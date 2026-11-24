use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use termax_interop::{HostSpec, KnownHosts, Marker, fingerprint_sha256};

const KEY: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIHQmsf/AJE2WtRdgTKgOjNIJfobAOOvAZW2XgzcVFIZ3";
/// From `ssh-keygen -lf` on the public key above.
const KEY_FINGERPRINT: &str = "SHA256:1b4O6bdYSPJuv0bwUQdTYb56UZsa4h6VPn9iodBS9XU";

fn fixture() -> KnownHosts {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/known_hosts");
    KnownHosts::parse_file(&path).expect("fixture readable")
}

#[test]
fn fingerprint_matches_ssh_keygen() {
    let blob = STANDARD.decode(KEY).unwrap();
    assert_eq!(fingerprint_sha256(&blob), KEY_FINGERPRINT);
    let kh = fixture();
    assert!(
        kh.entries
            .iter()
            .all(|e| e.fingerprint_sha256() == KEY_FINGERPRINT)
    );
}

#[test]
fn plain_and_bracketed_entries() {
    let kh = fixture();
    let first = &kh.entries[0];
    assert_eq!(first.line, 3);
    assert_eq!(first.key_type, "ssh-ed25519");
    assert_eq!(first.comment.as_deref(), Some("test key"));
    assert_eq!(
        first.hosts,
        HostSpec::Patterns(vec!["web.example.com".into(), "10.0.0.7".into()])
    );
    assert!(first.matches("web.example.com", 22));
    assert!(first.matches("WEB.example.com", 22));
    assert!(first.matches("10.0.0.7", 22));
    assert!(!first.matches("web.example.com", 2222));

    let bracketed = &kh.entries[1];
    assert!(bracketed.matches("git.example.com", 2222));
    assert!(!bracketed.matches("git.example.com", 22));

    let wildcard = &kh.entries[2];
    assert!(wildcard.matches("nas.lan", 22));
    assert!(!wildcard.matches("printer.lan", 22));
}

#[test]
fn hashed_entries_from_ssh_keygen_match() {
    let kh = fixture();
    let hashed: Vec<_> = kh.entries.iter().filter(|e| e.is_hashed()).collect();
    assert_eq!(hashed.len(), 2);
    assert!(hashed[0].matches("git.example.com", 2222));
    assert!(!hashed[0].matches("git.example.com", 22));
    assert!(hashed[1].matches("web.example.com", 22));
    assert!(!hashed[1].matches("www.example.com", 22));
}

#[test]
fn hashed_entry_generated_with_hmac() {
    let salt = [7u8; 20];
    let mut mac = Hmac::<Sha1>::new_from_slice(&salt).unwrap();
    mac.update(b"[db.internal]:2200");
    let hash = mac.finalize().into_bytes();
    let line = format!(
        "|1|{}|{} ssh-ed25519 {KEY}\n",
        STANDARD.encode(salt),
        STANDARD.encode(hash)
    );
    let kh = KnownHosts::parse_str(&line);
    assert!(kh.warnings.is_empty(), "{:?}", kh.warnings);
    let entry = &kh.entries[0];
    assert!(entry.matches("db.internal", 2200));
    assert!(!entry.matches("db.internal", 22));
    assert!(!entry.matches("db.internal2", 2200));
    assert_eq!(kh.lookup("db.internal", 2200).count(), 1);
}

#[test]
fn markers() {
    let kh = fixture();
    let ca: Vec<_> = kh
        .entries
        .iter()
        .filter(|e| e.marker == Some(Marker::CertAuthority))
        .collect();
    assert_eq!(ca.len(), 1);
    assert!(ca[0].matches("api.example.com", 22));
    assert_eq!(ca[0].comment.as_deref(), Some("ca"));
    let revoked: Vec<_> = kh
        .entries
        .iter()
        .filter(|e| e.marker == Some(Marker::Revoked))
        .collect();
    assert_eq!(revoked.len(), 1);
    assert!(revoked[0].matches("anything", 2022));

    // web.example.com: plain, hashed, cert-authority and revoked entries.
    assert_eq!(kh.lookup("web.example.com", 22).count(), 4);
}

#[test]
fn malformed_lines_become_warnings() {
    let kh = fixture();
    assert_eq!(kh.entries.len(), 7);
    let lines: Vec<_> = kh.warnings.iter().map(|w| w.line.unwrap()).collect();
    assert_eq!(lines, [11, 12, 13, 14, 15], "{:?}", kh.warnings);
    assert!(kh.warnings[0].message.contains("missing key data"));
    assert!(kh.warnings[1].message.contains("base64"));
    assert!(kh.warnings[2].message.contains("does not match"));
    assert!(kh.warnings[3].message.contains("unknown marker"));
    assert!(kh.warnings[4].message.contains("20 bytes"));
    assert!(kh.warnings.iter().all(|w| w.file.is_some()));
}
