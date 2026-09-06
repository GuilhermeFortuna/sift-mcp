use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

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
pub struct Registry {
    conn: Connection,
}
impl Registry {
    pub fn open_in_memory() -> Result<Self, RegistryError> {
        Self::open(Connection::open_in_memory().map_err(db)?)
    }
    pub fn open_path(path: &Path) -> Result<Self, RegistryError> {
        Self::open(Connection::open(path).map_err(db)?)
    }
    fn open(conn: Connection) -> Result<Self, RegistryError> {
        conn.execute_batch("PRAGMA foreign_keys = ON; CREATE TABLE IF NOT EXISTS registrations (id TEXT PRIMARY KEY, name TEXT NOT NULL, repo_path TEXT NOT NULL, store_path TEXT NOT NULL UNIQUE, model_path TEXT NOT NULL, daemon_path TEXT NOT NULL);").map_err(db)?;
        Ok(Self { conn })
    }
    pub fn register(&self, input: RegistrationInput) -> Result<Registration, RegistryError> {
        let config = validate(input)?;
        let id = Uuid::new_v4().to_string();
        self.conn.execute("INSERT INTO registrations (id, name, repo_path, store_path, model_path, daemon_path) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![id, config.name, config.repo_path.to_string_lossy(), config.store_path.to_string_lossy(), config.model_path.to_string_lossy(), config.daemon_path.to_string_lossy()]).map_err(|e| match e { rusqlite::Error::SqliteFailure(_, Some(ref msg)) if msg.contains("registrations.store_path") || msg.contains("UNIQUE constraint failed") => RegistryError::DuplicateStore(config.store_path.clone()), other => db(other) })?;
        Ok(Registration { id, config })
    }
    pub fn list(&self) -> Result<Vec<Registration>, RegistryError> {
        let mut s = self.conn.prepare("SELECT id, name, repo_path, store_path, model_path, daemon_path FROM registrations ORDER BY name, id").map_err(db)?;
        s.query_map([], row)
            .map_err(db)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db)
    }
    pub fn get(&self, id: &str) -> Result<Registration, RegistryError> {
        self.conn.query_row("SELECT id, name, repo_path, store_path, model_path, daemon_path FROM registrations WHERE id = ?1", [id], row).map_err(|e| match e { rusqlite::Error::QueryReturnedNoRows => RegistryError::Unknown(id.into()), other => db(other) })
    }
    pub fn remove(&self, id: &str) -> Result<(), RegistryError> {
        if self
            .conn
            .execute("DELETE FROM registrations WHERE id = ?1", [id])
            .map_err(db)?
            == 0
        {
            return Err(RegistryError::Unknown(id.into()));
        }
        Ok(())
    }
}
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Registration> {
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
fn validate(input: RegistrationInput) -> Result<RegistrationInput, RegistryError> {
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
fn db(error: rusqlite::Error) -> RegistryError {
    RegistryError::Database(error.to_string())
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
    #[test]
    fn rejects_store_aliases_that_resolve_to_the_same_location() {
        let temp = tempfile::tempdir().unwrap();
        let stores = temp.path().join("stores");
        fs::create_dir(&stores).unwrap();
        let registry = Registry::open_in_memory().unwrap();
        registry
            .register(input(temp.path(), stores.join("one")))
            .unwrap();
        #[cfg(unix)]
        {
            symlink(&stores, temp.path().join("alias")).unwrap();
            assert!(matches!(
                registry.register(input(temp.path(), temp.path().join("alias/one"))),
                Err(RegistryError::DuplicateStore(_))
            ));
        }
    }
    #[test]
    fn accepts_missing_store_leaf_and_removal_preserves_it() {
        let temp = tempfile::tempdir().unwrap();
        let stores = temp.path().join("stores");
        fs::create_dir(&stores).unwrap();
        let store = stores.join("future");
        let registry = Registry::open_in_memory().unwrap();
        let registration = registry
            .register(input(temp.path(), store.clone()))
            .unwrap();
        assert!(!store.exists());
        registry.remove(&registration.id).unwrap();
        assert!(!store.exists());
        assert!(registry.list().unwrap().is_empty());
    }
}
