//! Vault location, credentials, and `oxt vault …`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use oxt_core::{Vault, VaultId, VaultKind};
use oxt_vault::{Error as VaultError, KdfParams, SecretKey, VaultStore};

use crate::{Ctx, print_json};

const PASSWORD_ENV: &str = "OXT_VAULT_PASSWORD";
const SECRET_KEY_ENV: &str = "OXT_SECRET_KEY";
const DEFAULT_VAULT_NAME: &str = "Personal";
const MIN_PASSWORD_CHARS: usize = 8;

#[derive(Subcommand)]
pub enum VaultCmd {
    /// Create a new encrypted vault and print its Secret Key.
    Init {
        /// Argon2id memory cost in KiB (default 65536).
        #[arg(long, hide = true)]
        kdf_memory_kib: Option<u32>,
    },
    /// Show vault location, schema version and a summary of its contents.
    Info,
}

pub fn run(ctx: &Ctx, cmd: VaultCmd) -> Result<ExitCode> {
    match cmd {
        VaultCmd::Init { kdf_memory_kib } => init(ctx, kdf_memory_kib),
        VaultCmd::Info => info(ctx),
    }
}

/// Resolved database path.
pub fn db_path(ctx: &Ctx) -> Result<PathBuf> {
    if let Some(p) = &ctx.db {
        return Ok(p.clone());
    }
    let dirs = directories::ProjectDirs::from("dev", "OxTerm", "oxterm")
        .context("cannot determine the per-user data directory; pass --db")?;
    Ok(dirs.data_dir().join("vault.db"))
}

fn password_from_env() -> Option<String> {
    std::env::var(PASSWORD_ENV).ok()
}

fn prompt_secret(prompt: &str) -> Result<String> {
    rpassword::prompt_password(prompt)
        .with_context(|| format!("reading {}", prompt.trim_end_matches([':', ' '])))
}

fn read_secret_key() -> Result<SecretKey> {
    let raw = match std::env::var(SECRET_KEY_ENV) {
        Ok(v) => v,
        Err(_) => prompt_secret("Secret Key: ")?,
    };
    SecretKey::parse(&raw).map_err(|e| anyhow::anyhow!("invalid Secret Key: {e}"))
}

/// Open the vault at the configured path, prompting for credentials.
pub fn open_store(ctx: &Ctx) -> Result<VaultStore> {
    let path = db_path(ctx)?;
    if !path.exists() {
        bail!(
            "no vault at {}; create one with `oxt vault init`",
            path.display()
        );
    }
    let password = match password_from_env() {
        Some(p) => p,
        None => prompt_secret("Vault password: ")?,
    };
    let secret_key = read_secret_key()?;
    match VaultStore::open(&path, &password, &secret_key) {
        Ok(store) => Ok(store),
        Err(VaultError::WrongCredentials) => bail!("wrong password or Secret Key"),
        Err(e) => Err(e).with_context(|| format!("opening {}", path.display())),
    }
}

/// The vault that imports and listings act on: the first personal vault.
pub fn default_vault(store: &VaultStore) -> Result<Vault> {
    let vaults = store.list_vaults()?;
    vaults
        .iter()
        .find(|v| v.kind == VaultKind::Personal)
        .or_else(|| vaults.first())
        .cloned()
        .context("the store has no vault; recreate it with `oxt vault init`")
}

fn init(ctx: &Ctx, kdf_memory_kib: Option<u32>) -> Result<ExitCode> {
    let path = db_path(ctx)?;
    if path.exists() {
        bail!(
            "{} already exists; refusing to overwrite it",
            path.display()
        );
    }
    let password = match password_from_env() {
        Some(p) => p,
        None => {
            let first = prompt_secret("New vault password: ")?;
            if prompt_secret("Repeat password: ")? != first {
                bail!("passwords do not match");
            }
            first
        }
    };
    if password.chars().count() < MIN_PASSWORD_CHARS {
        bail!("password must be at least {MIN_PASSWORD_CHARS} characters");
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }

    let secret_key = oxt_vault::generate_secret_key()?;
    let mut store = match kdf_memory_kib {
        Some(m) => {
            VaultStore::create_with_params(&path, &password, &secret_key, KdfParams::new(m, 1, 1)?)?
        }
        None => VaultStore::create(&path, &password, &secret_key)?,
    };
    store.create_vault(&Vault {
        id: VaultId::new(),
        name: DEFAULT_VAULT_NAME.into(),
        kind: VaultKind::Personal,
        settings: Default::default(),
    })?;

    let formatted = secret_key.to_formatted();
    if ctx.json {
        print_json(&serde_json::json!({ "path": path, "secret_key": formatted.as_str() }))?;
    } else {
        println!("Created vault at {}", path.display());
        println!();
        println!("Secret Key: {}", formatted.as_str());
        println!();
        println!("Store this Secret Key somewhere safe. You need it together with your");
        println!("password to open the vault, and it cannot be recovered if lost.");
    }
    Ok(ExitCode::SUCCESS)
}

fn info(ctx: &Ctx) -> Result<ExitCode> {
    let store = open_store(ctx)?;
    let path = db_path(ctx)?;
    let schema_version = store.schema_version()?;
    let mut vaults = Vec::new();
    for v in store.list_vaults()? {
        let hosts = store.list_hosts(v.id, &Default::default())?.len();
        vaults.push(
            serde_json::json!({ "id": v.id, "name": v.name, "kind": v.kind, "hosts": hosts }),
        );
    }
    let known_hosts = store.list_known_hosts()?.len();
    if ctx.json {
        print_json(&serde_json::json!({
            "path": path,
            "schema_version": schema_version,
            "vaults": vaults,
            "known_hosts": known_hosts,
        }))?;
    } else {
        println!("Path:           {}", path.display());
        println!("Schema version: {schema_version}");
        for v in &vaults {
            println!(
                "Vault:          {} ({}, {} hosts)",
                v["name"].as_str().unwrap_or_default(),
                v["kind"].as_str().unwrap_or_default(),
                v["hosts"]
            );
        }
        println!("Known hosts:    {known_hosts}");
    }
    Ok(ExitCode::SUCCESS)
}
