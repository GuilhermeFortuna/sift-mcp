//! Socket and lock-file path derivation from a store directory.

use std::fs;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::protocol::DaemonError;

/// Derive a deterministic runtime directory for a store's socket and lock.
pub fn runtime_dir_for_store(store_dir: &Path) -> Result<PathBuf, DaemonError> {
    let canonical = stable_store_path(store_dir)?;
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

/// Return a stable absolute path even before the store has been created.
/// Existing paths are canonicalized so symlink aliases share one daemon; a
/// missing store uses its canonicalized parent and the requested leaf.
fn stable_store_path(store_dir: &Path) -> Result<PathBuf, DaemonError> {
    if store_dir.exists() {
        return store_dir.canonicalize().map_err(|e| DaemonError::Internal {
            detail: format!("canonicalize store {}: {e}", store_dir.display()),
        });
    }

    let absolute = if store_dir.is_absolute() {
        store_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| DaemonError::Internal {
                detail: format!("resolve current directory: {e}"),
            })?
            .join(store_dir)
    };
    let leaf = absolute.file_name().ok_or_else(|| DaemonError::Internal {
        detail: format!("store path has no leaf: {}", store_dir.display()),
    })?;
    let parent = absolute.parent().unwrap_or_else(|| Path::new("."));
    let parent = if parent.exists() {
        parent.canonicalize().map_err(|e| DaemonError::Internal {
            detail: format!("canonicalize store parent {}: {e}", parent.display()),
        })?
    } else {
        parent.to_path_buf()
    };
    Ok(parent.join(leaf))
}

pub fn socket_path_for_store(store_dir: &Path) -> Result<PathBuf, DaemonError> {
    Ok(runtime_dir_for_store(store_dir)?.join("daemon.sock"))
}

pub fn lock_path_for_store(store_dir: &Path) -> Result<PathBuf, DaemonError> {
    Ok(runtime_dir_for_store(store_dir)?.join("daemon.lock"))
}

/// Ensure the runtime directory exists with owner-only permissions. The directory
/// that will contain the socket must not be world-writable.
pub fn ensure_runtime_dir(dir: &Path) -> Result<(), DaemonError> {
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
    if let Some(parent) = dir.parent()
        && parent.exists()
    {
        // Creating a new subdirectory under a world-writable parent is unsafe:
        // another user can rename our directory out of the way.
        check_not_world_writable(parent)?;
    }
    fs::DirBuilder::new().mode(0o700).create(dir).or_else(|e| {
        // Parent may be missing — create with recursive only when parent is safe.
        if e.kind() == std::io::ErrorKind::NotFound {
            if let Some(parent) = dir.parent() {
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)
                    .map_err(|e| DaemonError::Internal {
                        detail: format!("create parent {}: {e}", parent.display()),
                    })?;
                // Re-check parent after create.
                check_not_world_writable(parent)?;
            }
            fs::DirBuilder::new()
                .mode(0o700)
                .create(dir)
                .map_err(|e| DaemonError::Internal {
                    detail: format!("create runtime dir {}: {e}", dir.display()),
                })
        } else {
            Err(DaemonError::Internal {
                detail: format!("create runtime dir {}: {e}", dir.display()),
            })
        }
    })?;
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
    fn socket_path_can_be_derived_before_store_exists() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("new-store");
        let path = socket_path_for_store(&missing).unwrap();
        assert!(path.ends_with("daemon.sock"));
        assert_eq!(path, socket_path_for_store(&missing).unwrap());
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
                assert!(detail.contains("world-writable"), "detail={detail}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
