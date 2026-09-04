//! Socket and lock-file path derivation from a store directory.

use std::fs;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::protocol::DaemonError;

/// Derive a deterministic runtime directory for a store's socket and lock.
pub fn runtime_dir_for_store(store_dir: &Path) -> Result<PathBuf, DaemonError> {
    let canonical = store_dir
        .canonicalize()
        .map_err(|e| DaemonError::Internal {
            detail: format!("canonicalize store {}: {e}", store_dir.display()),
        })?;
    let hash = blake3::hash(canonical.to_string_lossy().as_bytes());
    let hex = hash.to_hex();
    let short = &hex.as_str()[..16];

    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
            PathBuf::from(format!("/tmp/sift-{user}"))
        });

    Ok(base.join("sift").join(short))
}

pub fn socket_path_for_store(store_dir: &Path) -> Result<PathBuf, DaemonError> {
    Ok(runtime_dir_for_store(store_dir)?.join("daemon.sock"))
}

pub fn lock_path_for_store(store_dir: &Path) -> Result<PathBuf, DaemonError> {
    Ok(runtime_dir_for_store(store_dir)?.join("daemon.lock"))
}

/// Ensure the runtime directory exists with owner-only permissions, refusing a
/// world-writable parent.
pub fn ensure_runtime_dir(dir: &Path) -> Result<(), DaemonError> {
    if let Some(parent) = dir.parent()
        && parent.exists()
    {
        check_not_world_writable(parent)?;
    }
    if dir.exists() {
        check_not_world_writable(dir)?;
        let meta = fs::metadata(dir).map_err(|e| DaemonError::Internal {
            detail: format!("stat {}: {e}", dir.display()),
        })?;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(DaemonError::Internal {
                detail: format!(
                    "runtime dir {} permissions {:o} allow group/other",
                    dir.display(),
                    mode
                ),
            });
        }
        return Ok(());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| DaemonError::Internal {
            detail: format!("create runtime dir {}: {e}", dir.display()),
        })?;
    // Recheck parent after create — recursive create may have made parents.
    if let Some(parent) = dir.parent() {
        check_not_world_writable(parent)?;
    }
    Ok(())
}

fn check_not_world_writable(path: &Path) -> Result<(), DaemonError> {
    let meta = fs::metadata(path).map_err(|e| DaemonError::Internal {
        detail: format!("stat {}: {e}", path.display()),
    })?;
    let mode = meta.permissions().mode();
    if mode & 0o002 != 0 {
        return Err(DaemonError::Internal {
            detail: format!(
                "refusing world-writable directory {} (mode {:o})",
                path.display(),
                mode & 0o777
            ),
        });
    }
    Ok(())
}

/// Create a unix socket listener path: ensure dir, unlink stale sock only when
/// caller holds the lock, then return the path ready for bind.
pub fn prepare_socket_path(socket: &Path) -> Result<(), DaemonError> {
    let dir = socket.parent().ok_or_else(|| DaemonError::Internal {
        detail: "socket path has no parent".into(),
    })?;
    ensure_runtime_dir(dir)?;
    if socket.exists() {
        fs::remove_file(socket).map_err(|e| DaemonError::Internal {
            detail: format!("unlink stale socket {}: {e}", socket.display()),
        })?;
    }
    Ok(())
}

/// Assert socket file mode denies group and other.
pub fn assert_socket_permissions(socket: &Path) -> Result<(), DaemonError> {
    let meta = fs::metadata(socket).map_err(|e| DaemonError::Internal {
        detail: format!("stat socket {}: {e}", socket.display()),
    })?;
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(DaemonError::Internal {
            detail: format!(
                "socket {} mode {:o} allows group/other",
                socket.display(),
                mode
            ),
        });
    }
    Ok(())
}

/// Set mode on a newly created socket to 0o600.
pub fn tighten_socket_permissions(socket: &Path) -> Result<(), DaemonError> {
    let mut perms = fs::metadata(socket)
        .map_err(|e| DaemonError::Internal {
            detail: format!("stat socket {}: {e}", socket.display()),
        })?
        .permissions();
    perms.set_mode(0o600);
    fs::set_permissions(socket, perms).map_err(|e| DaemonError::Internal {
        detail: format!("chmod socket {}: {e}", socket.display()),
    })?;
    Ok(())
}

/// Open/create the lock file with owner-only mode.
pub fn open_lock_file(path: &Path) -> Result<std::fs::File, DaemonError> {
    if let Some(parent) = path.parent() {
        ensure_runtime_dir(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| DaemonError::Internal {
            detail: format!("open lock {}: {e}", path.display()),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    #[test]
    fn socket_path_deterministic_for_same_store() {
        let dir = tempdir().unwrap();
        let store = dir.path().join("store");
        fs::create_dir_all(&store).unwrap();
        let a = socket_path_for_store(&store).unwrap();
        let b = socket_path_for_store(&store).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn socket_path_differs_for_different_stores() {
        let dir = tempdir().unwrap();
        let s1 = dir.path().join("a");
        let s2 = dir.path().join("b");
        fs::create_dir_all(&s1).unwrap();
        fs::create_dir_all(&s2).unwrap();
        let p1 = socket_path_for_store(&s1).unwrap();
        let p2 = socket_path_for_store(&s2).unwrap();
        assert_ne!(p1, p2);
    }

    #[test]
    fn ensure_runtime_dir_is_owner_only() {
        let dir = tempdir().unwrap();
        // Use a nested path under the tempdir (not world-writable).
        let rt = dir.path().join("rt").join("hash");
        ensure_runtime_dir(&rt).unwrap();
        let mode = fs::metadata(&rt).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode & 0o077, 0, "mode={mode:o}");
    }

    #[test]
    fn refuses_world_writable_directory() {
        let dir = tempdir().unwrap();
        let ww = dir.path().join("ww");
        fs::create_dir(&ww).unwrap();
        let mut perms = fs::metadata(&ww).unwrap().permissions();
        perms.set_mode(0o777);
        fs::set_permissions(&ww, perms).unwrap();
        let nested = ww.join("child");
        let err = ensure_runtime_dir(&nested).unwrap_err();
        match err {
            DaemonError::Internal { detail } => {
                assert!(
                    detail.contains("world-writable"),
                    "detail={detail}"
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
