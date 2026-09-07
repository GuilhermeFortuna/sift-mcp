use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistrationInput {
    pub name: String,
    pub repo_path: PathBuf,
    pub store_path: PathBuf,
    pub model_path: PathBuf,
    pub daemon_path: PathBuf,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Registration {
    pub id: String,
    pub config: RegistrationInput,
}
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("registration name must not be blank")]
    BlankName,
    #[error("{field} must be an absolute path")]
    RelativePath { field: &'static str },
    #[error("{field} is not a valid {expected}: {path}")]
    InvalidPath {
        field: &'static str,
        expected: &'static str,
        path: PathBuf,
    },
    #[error("a registration already uses store {0}")]
    DuplicateStore(PathBuf),
    #[error("unknown repository {0}")]
    Unknown(String),
    #[error("registry database error: {0}")]
    Database(String),
}
pub(crate) fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Registration> {
    Ok(Registration {
        id: r.get(0)?,
        config: RegistrationInput {
            name: r.get(1)?,
            repo_path: PathBuf::from(r.get::<_, String>(2)?),
            store_path: PathBuf::from(r.get::<_, String>(3)?),
            model_path: PathBuf::from(r.get::<_, String>(4)?),
            daemon_path: PathBuf::from(r.get::<_, String>(5)?),
        },
    })
}
pub fn validate(input: RegistrationInput) -> Result<RegistrationInput, RegistryError> {
    if input.name.trim().is_empty() {
        return Err(RegistryError::BlankName);
    }
    Ok(RegistrationInput {
        name: input.name.trim().into(),
        repo_path: directory("repository", &input.repo_path)?,
        store_path: store(&input.store_path)?,
        model_path: directory("model", &input.model_path)?,
        daemon_path: executable(&input.daemon_path)?,
    })
}
fn absolute(field: &'static str, path: &Path) -> Result<(), RegistryError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(RegistryError::RelativePath { field })
    }
}
fn directory(field: &'static str, path: &Path) -> Result<PathBuf, RegistryError> {
    absolute(field, path)?;
    let value = fs::canonicalize(path).map_err(|_| invalid(field, "directory", path))?;
    if value.is_dir() {
        Ok(value)
    } else {
        Err(invalid(field, "directory", path))
    }
}
fn store(path: &Path) -> Result<PathBuf, RegistryError> {
    absolute("store", path)?;
    if path.exists() {
        return directory("store", path);
    }
    let parent = path.parent().ok_or_else(|| {
        invalid(
            "store",
            "directory or missing final leaf below a directory",
            path,
        )
    })?;
    let leaf = path.file_name().ok_or_else(|| {
        invalid(
            "store",
            "directory or missing final leaf below a directory",
            path,
        )
    })?;
    Ok(directory("store", parent)?.join(leaf))
}
fn executable(path: &Path) -> Result<PathBuf, RegistryError> {
    absolute("daemon", path)?;
    let value = fs::canonicalize(path).map_err(|_| invalid("daemon", "executable file", path))?;
    if value.is_file() && is_executable(&value) {
        Ok(value)
    } else {
        Err(invalid("daemon", "executable file", path))
    }
}
fn invalid(field: &'static str, expected: &'static str, path: &Path) -> RegistryError {
    RegistryError::InvalidPath {
        field,
        expected,
        path: path.into(),
    }
}
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}
#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};
    fn input(root: &Path, store_path: PathBuf) -> RegistrationInput {
        let repo = root.join("repo");
        let model = root.join("model");
        let daemon = root.join("daemon");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&model).unwrap();
        fs::write(&daemon, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            let mut p = fs::metadata(&daemon).unwrap().permissions();
            p.set_mode(0o700);
            fs::set_permissions(&daemon, p).unwrap();
        }
        RegistrationInput {
            name: " Example ".into(),
            repo_path: repo,
            store_path,
            model_path: model,
            daemon_path: daemon,
        }
    }
    #[tokio::test]
    async fn rejects_store_aliases_that_resolve_to_the_same_location() {
        let temp = tempfile::tempdir().unwrap();
        let stores = temp.path().join("stores");
        fs::create_dir(&stores).unwrap();
        let registry = crate::db::Database::memory(0).await.unwrap();
        registry
            .register(input(temp.path(), stores.join("one")))
            .await
            .unwrap();
        #[cfg(unix)]
        {
            symlink(&stores, temp.path().join("alias")).unwrap();
            assert!(matches!(
                registry
                    .register(input(temp.path(), temp.path().join("alias/one")))
                    .await,
                Err(crate::db::DbError::Registry(RegistryError::DuplicateStore(
                    _
                )))
            ));
        }
    }
    #[tokio::test]
    async fn accepts_missing_store_leaf_and_removal_preserves_it() {
        let temp = tempfile::tempdir().unwrap();
        let stores = temp.path().join("stores");
        fs::create_dir(&stores).unwrap();
        let store = stores.join("future");
        let registry = crate::db::Database::memory(0).await.unwrap();
        let registration = registry
            .register(input(temp.path(), store.clone()))
            .await
            .unwrap();
        assert!(!store.exists());
        registry.remove(&registration.id).await.unwrap();
        assert!(!store.exists());
        assert!(registry.list().await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn validates_replacements_and_keeps_repositories_isolated() {
        let temp = tempfile::tempdir().unwrap();
        let d = crate::db::Database::memory(0).await.unwrap();
        let one = d
            .register(input(temp.path(), temp.path().join("one")))
            .await
            .unwrap();
        let two = d
            .register(input(temp.path(), temp.path().join("two")))
            .await
            .unwrap();
        assert_eq!(d.list().await.unwrap().len(), 2);
        assert!(d.replace(&two.id, one.config.clone()).await.is_err());
        let mut invalid = two.config.clone();
        invalid.name = "  ".into();
        assert!(d.replace(&two.id, invalid).await.is_err());
        let mut invalid = two.config.clone();
        invalid.model_path = temp.path().join("missing");
        assert!(d.replace(&two.id, invalid).await.is_err());
        let mut invalid = two.config.clone();
        invalid.daemon_path = two.config.model_path.clone();
        assert!(d.replace(&two.id, invalid).await.is_err());
        let mut invalid = two.config.clone();
        invalid.repo_path = "relative".into();
        assert!(d.replace(&two.id, invalid).await.is_err());
        assert!(d.get("missing").await.is_err());
        assert!(d.remove("missing").await.is_err());
        let mut replacement = two.config.clone();
        replacement.name = "renamed".into();
        assert_eq!(
            d.replace(&two.id, replacement).await.unwrap().config.name,
            "renamed"
        );
        fs::create_dir(&one.config.store_path).unwrap();
        fs::write(one.config.store_path.join("index"), "keep").unwrap();
        d.remove(&one.id).await.unwrap();
        assert!(one.config.store_path.join("index").exists());
        assert_eq!(d.get(&two.id).await.unwrap().config.name, "renamed");
    }
}
