//! Where the endpoint lives, and the single-runtime lock.
//!
//! Unix: `<dir>/control.sock` in a directory private to the user (0700,
//! owned by the effective UID); the default directory is
//! `$XDG_RUNTIME_DIR/termax`, `~/Library/Application Support/dev.Termax.termax`
//! on macOS, or `$TMPDIR/termax-<uid>` when there is no runtime directory.
//! Windows: the named pipe `\\.\pipe\termax-control-<hash>`, with the lock
//! file in the per-user local data directory.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::ControlError;

/// File name of the socket inside the endpoint directory.
pub const SOCKET_NAME: &str = "control.sock";
/// File name of the single-runtime lock inside the endpoint directory.
pub const LOCK_NAME: &str = "control.lock";
/// File name of the paired-client registry in the config directory.
pub const REGISTRY_NAME: &str = "control-clients.json";

/// Longest Unix socket path (`sun_path` minus the terminating NUL).
#[cfg(target_os = "macos")]
pub const MAX_SOCKET_PATH: usize = 103;
/// Longest Unix socket path (`sun_path` minus the terminating NUL).
#[cfg(not(target_os = "macos"))]
pub const MAX_SOCKET_PATH: usize = 107;

/// The location of a control endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    dir: PathBuf,
}

impl Endpoint {
    /// The endpoint in `dir` (created private on first use).
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The current user's default endpoint.
    pub fn for_current_user() -> Result<Self, ControlError> {
        Ok(Self::in_dir(default_dir()?))
    }

    /// The endpoint directory (holds the socket on Unix and the lock).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Socket path (Unix).
    pub fn socket_path(&self) -> PathBuf {
        self.dir.join(SOCKET_NAME)
    }

    /// Lock file path.
    pub fn lock_path(&self) -> PathBuf {
        self.dir.join(LOCK_NAME)
    }

    /// Named pipe path (Windows): `\\.\pipe\termax-control-<hash>`, where
    /// the hash covers the user and the endpoint directory, so test
    /// endpoints in other directories do not collide.
    pub fn pipe_name(&self) -> String {
        let user = format!(
            "{}\\{}",
            std::env::var("USERDOMAIN").unwrap_or_default(),
            std::env::var("USERNAME").unwrap_or_default()
        );
        let mut h = Sha256::new();
        h.update(user.as_bytes());
        h.update([0]);
        h.update(self.dir.to_string_lossy().as_bytes());
        let digest = h.finalize();
        format!(r"\\.\pipe\termax-control-{}", hex::encode(&digest[..8]))
    }

    /// What to show users: the socket path or pipe name.
    pub fn display(&self) -> String {
        if cfg!(windows) {
            self.pipe_name()
        } else {
            self.socket_path().display().to_string()
        }
    }
}

/// The per-user config directory (client registry).
pub fn config_dir() -> Result<PathBuf, ControlError> {
    directories::ProjectDirs::from("dev", "Termax", "termax")
        .map(|d| d.config_dir().to_path_buf())
        .ok_or(ControlError::NoRuntimeDir)
}

#[cfg(target_os = "macos")]
fn default_dir() -> Result<PathBuf, ControlError> {
    directories::ProjectDirs::from("dev", "Termax", "termax")
        .map(|d| d.data_dir().to_path_buf())
        .ok_or(ControlError::NoRuntimeDir)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn default_dir() -> Result<PathBuf, ControlError> {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(d) => Ok(PathBuf::from(d).join("termax")),
        None => Ok(std::env::temp_dir().join(format!("termax-{}", euid()))),
    }
}

#[cfg(windows)]
fn default_dir() -> Result<PathBuf, ControlError> {
    directories::ProjectDirs::from("dev", "Termax", "termax")
        .map(|d| d.data_local_dir().to_path_buf())
        .ok_or(ControlError::NoRuntimeDir)
}

/// The effective user id of this process.
#[cfg(unix)]
pub fn euid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// Create `dir` if needed and make sure only this user can use it: it must
/// be a real directory (not a symlink) owned by the effective UID. Group
/// and other permission bits are removed; a directory owned by someone else
/// is refused.
pub fn prepare_private_dir(dir: &Path) -> Result<(), ControlError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if !dir.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        check_private_dir(dir)?;
        let mode = std::fs::symlink_metadata(dir)?.permissions().mode();
        if mode & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)?;
        Ok(())
    }
}

/// Check that `dir` is a directory owned by this user and, for the client
/// side, that nobody else can write into it.
#[cfg(unix)]
pub fn check_private_dir(dir: &Path) -> Result<(), ControlError> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.file_type().is_dir() {
        return Err(ControlError::Insecure(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    if meta.uid() != euid() {
        return Err(ControlError::Insecure(format!(
            "{} is owned by uid {}, not by this user (uid {})",
            dir.display(),
            meta.uid(),
            euid()
        )));
    }
    Ok(())
}

/// Check that the socket file and its directory belong to this user and are
/// not reachable by others, before a client sends a token to it.
#[cfg(unix)]
pub fn check_private_socket(path: &Path) -> Result<(), ControlError> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    if let Some(dir) = path.parent() {
        check_private_dir(dir)?;
        let mode = std::fs::symlink_metadata(dir)?.mode();
        if mode & 0o077 != 0 {
            return Err(ControlError::Insecure(format!(
                "{} is accessible to other users (mode {:o})",
                dir.display(),
                mode & 0o777
            )));
        }
    }
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.file_type().is_socket() || meta.uid() != euid() {
        return Err(ControlError::Insecure(format!(
            "{} is not a socket owned by this user",
            path.display()
        )));
    }
    Ok(())
}

/// Held while a runtime serves an endpoint: one runtime per user and
/// directory. Released on drop (the lock file itself stays).
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    /// Take the lock, creating the private directory if needed.
    pub fn acquire(endpoint: &Endpoint) -> Result<Self, ControlError> {
        prepare_private_dir(endpoint.dir())?;
        let path = endpoint.lock_path();
        let mut opts = OpenOptions::new();
        opts.create(true).truncate(false).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file, path }),
            Err(std::fs::TryLockError::WouldBlock) => {
                Err(ControlError::AlreadyRunning(endpoint.display()))
            }
            Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }

    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[allow(clippy::unwrap_used, reason = "test helper; a panic fails the test")]
    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn private_dir_is_created_0700_and_tightened() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a/b");
        prepare_private_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        prepare_private_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn symlinks_and_files_are_refused_as_endpoint_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(matches!(
            prepare_private_dir(&link),
            Err(ControlError::Insecure(_))
        ));
        let file = tmp.path().join("file");
        std::fs::write(&file, b"").unwrap();
        assert!(matches!(
            prepare_private_dir(&file),
            Err(ControlError::Insecure(_))
        ));
    }

    #[test]
    fn a_second_runtime_cannot_take_the_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let ep = Endpoint::in_dir(tmp.path().join("ep"));
        let first = InstanceLock::acquire(&ep).unwrap();
        assert!(matches!(
            InstanceLock::acquire(&ep),
            Err(ControlError::AlreadyRunning(_))
        ));
        assert_eq!(mode(first.path()), 0o600);
        drop(first);
        InstanceLock::acquire(&ep).unwrap();
    }

    #[test]
    fn a_world_writable_socket_dir_is_refused_by_clients() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ep");
        prepare_private_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        let err = check_private_socket(&dir.join(SOCKET_NAME)).unwrap_err();
        assert!(matches!(err, ControlError::Insecure(_)), "{err}");
    }

    #[test]
    fn pipe_names_differ_per_directory() {
        assert_ne!(
            Endpoint::in_dir("/a").pipe_name(),
            Endpoint::in_dir("/b").pipe_name()
        );
        assert!(
            Endpoint::in_dir("/a")
                .pipe_name()
                .starts_with(r"\\.\pipe\termax-control-")
        );
    }
}
