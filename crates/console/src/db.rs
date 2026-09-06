//! One blocking owner for all console SQLite access.
use crate::registry::{Registration, RegistrationInput, RegistryError};
use fs4::fs_std::FileExt;
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
    sync::mpsc,
};
use thiserror::Error;
#[derive(Debug, Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("database IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error("database worker unavailable")]
    Closed,
    #[error("database is already in use")]
    Locked,
    #[error("unsupported database schema version")]
    Version,
    #[error("invalid persisted metadata")]
    Metadata,
}
type Work = Box<dyn FnOnce(&mut Connection) + Send>;
#[derive(Clone)]
pub struct Database {
    sender: mpsc::Sender<Work>,
}
impl Database {
    pub async fn open(path: &Path, now: u64) -> Result<Self, DbError> {
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            if !parent.exists() {
                fs::create_dir_all(parent)?;
                private(parent, 0o700)?;
            }
            let lock = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path.with_extension("lock"))?;
            private(&path.with_extension("lock"), 0o600)?;
            if !lock.try_lock_exclusive()? {
                return Err(DbError::Locked);
            }
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)?;
            private(&path, 0o600)?;
            drop(file);
            let conn = Connection::open(path)?;
            Self::worker(conn, Some(lock), now)
        })
        .await
        .map_err(|_| DbError::Closed)?
    }
    pub async fn memory(now: u64) -> Result<Self, DbError> {
        tokio::task::spawn_blocking(move || Self::worker(Connection::open_in_memory()?, None, now))
            .await
            .map_err(|_| DbError::Closed)?
    }
    fn worker(mut conn: Connection, lock: Option<File>, now: u64) -> Result<Self, DbError> {
        migrate(&mut conn)?;
        conn.execute("UPDATE collection_gaps SET to_unix_ms=?1 WHERE reason='schema_migration' AND to_unix_ms=0",[now])?;
        conn.execute("INSERT INTO collection_gaps(repository_id,from_unix_ms,to_unix_ms,reason) SELECT repository_id,MIN(observed_at_unix_ms,?1),?1,'console_restart' FROM collector_cursors",[now])?;
        crate::history::prune(&conn, now)?;
        conn.execute(
            "UPDATE jobs SET state='interrupted' WHERE state='running'",
            [],
        )?;
        let (sender, receiver) = mpsc::channel::<Work>();
        std::thread::Builder::new()
            .name("console-sqlite".into())
            .spawn(move || {
                let _lock = lock;
                while let Ok(work) = receiver.recv() {
                    work(&mut conn);
                }
            })?;
        Ok(Self { sender })
    }
    pub async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
    ) -> Result<T, DbError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sender
            .send(Box::new(move |conn| {
                let _ = tx.send(f(conn));
            }))
            .map_err(|_| DbError::Closed)?;
        rx.await.map_err(|_| DbError::Closed)?
    }
    pub async fn list(&self) -> Result<Vec<Registration>, DbError> {
        self.call(|c| { let mut s=c.prepare("SELECT id,name,repo_path,store_path,model_path,daemon_path FROM registrations ORDER BY name,id")?; Ok(s.query_map([], crate::registry::row)?.collect::<Result<_,_>>()?) }).await
    }
    pub async fn get(&self, id: &str) -> Result<Registration, DbError> {
        let id = id.to_owned();
        self.call(move |c| get(c, &id)).await
    }
    pub async fn register(&self, input: RegistrationInput) -> Result<Registration, DbError> {
        self.save(None, input).await
    }
    pub async fn replace(
        &self,
        id: &str,
        input: RegistrationInput,
    ) -> Result<Registration, DbError> {
        self.save(Some(id.to_owned()), input).await
    }
    async fn save(
        &self,
        id: Option<String>,
        input: RegistrationInput,
    ) -> Result<Registration, DbError> {
        self.call(move |c| {
            let config=crate::registry::validate(input)?;
            let tx=c.transaction()?;
            if let Some(id)=&id { get(&tx,id)?; }
            let id=id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let duplicate: Option<String>=tx.query_row("SELECT id FROM registrations WHERE store_path=?1 AND id<>?2",params![config.store_path.to_string_lossy(),id],|r|r.get(0)).optional()?;
            if duplicate.is_some() { return Err(RegistryError::DuplicateStore(config.store_path).into()); }
            tx.execute("INSERT INTO registrations VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET name=excluded.name,repo_path=excluded.repo_path,store_path=excluded.store_path,model_path=excluded.model_path,daemon_path=excluded.daemon_path",params![id,config.name,config.repo_path.to_string_lossy(),config.store_path.to_string_lossy(),config.model_path.to_string_lossy(),config.daemon_path.to_string_lossy()])?;
            tx.execute("DELETE FROM collector_cursors WHERE repository_id=?1",[&id])?;
            tx.commit()?;
            Ok(Registration{id,config})
        }).await
    }
    pub async fn remove(&self, id: &str) -> Result<(), DbError> {
        let id = id.to_owned();
        self.call(move |c| {
            if c.execute("DELETE FROM registrations WHERE id=?1", [&id])? == 0 {
                return Err(RegistryError::Unknown(id).into());
            }
            Ok(())
        })
        .await
    }
}
fn get(c: &Connection, id: &str) -> Result<Registration, DbError> {
    c.query_row(
        "SELECT id,name,repo_path,store_path,model_path,daemon_path FROM registrations WHERE id=?1",
        [id],
        crate::registry::row,
    )
    .optional()?
    .ok_or_else(|| RegistryError::Unknown(id.into()).into())
}
#[cfg(unix)]
fn private(path: &Path, mode: u32) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}
#[cfg(not(unix))]
fn private(_: &Path, _: u32) -> Result<(), std::io::Error> {
    Ok(())
}
fn migrate(c: &mut Connection) -> Result<(), DbError> {
    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;
    let version: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > 1 {
        return Err(DbError::Version);
    }
    if version == 1 {
        return Ok(());
    }
    let tx = c.transaction()?;
    let legacy: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='registrations')",
        [],
        |r| r.get(0),
    )?;
    if legacy {
        tx.execute_batch("ALTER TABLE registrations RENAME TO legacy_registrations; DROP TABLE IF EXISTS request_events; DROP TABLE IF EXISTS metric_samples; DROP TABLE IF EXISTS index_reports;")?;
    }
    tx.execute_batch(SCHEMA)?;
    if legacy {
        tx.execute_batch("INSERT INTO registrations SELECT * FROM legacy_registrations; INSERT INTO collection_gaps(repository_id,from_unix_ms,to_unix_ms,reason) SELECT id,0,0,'schema_migration' FROM registrations; DROP TABLE legacy_registrations;")?;
    }
    tx.pragma_update(None, "user_version", 1)?;
    tx.commit()?;
    Ok(())
}
const SCHEMA:&str="
CREATE TABLE registrations(id TEXT PRIMARY KEY,name TEXT NOT NULL,repo_path TEXT NOT NULL,store_path TEXT NOT NULL UNIQUE,model_path TEXT NOT NULL,daemon_path TEXT NOT NULL);
CREATE TABLE request_events(id INTEGER PRIMARY KEY,repository_id TEXT NOT NULL REFERENCES registrations(id) ON DELETE CASCADE,instance_id TEXT NOT NULL,sequence INTEGER NOT NULL,completed_at_unix_ms INTEGER NOT NULL,payload TEXT NOT NULL,UNIQUE(repository_id,instance_id,sequence));
CREATE INDEX event_time ON request_events(completed_at_unix_ms,id);
CREATE TABLE metric_samples(id INTEGER PRIMARY KEY,repository_id TEXT NOT NULL REFERENCES registrations(id) ON DELETE CASCADE,sampled_at_unix_ms INTEGER NOT NULL,payload TEXT NOT NULL);
CREATE INDEX metric_time ON metric_samples(sampled_at_unix_ms,id);
CREATE TABLE index_reports(id INTEGER PRIMARY KEY,repository_id TEXT NOT NULL REFERENCES registrations(id) ON DELETE CASCADE,instance_id TEXT NOT NULL,completed_at_unix_ms INTEGER NOT NULL,payload TEXT NOT NULL,UNIQUE(repository_id,instance_id,completed_at_unix_ms));
CREATE TABLE collection_gaps(id INTEGER PRIMARY KEY,repository_id TEXT NOT NULL REFERENCES registrations(id) ON DELETE CASCADE,from_unix_ms INTEGER NOT NULL,to_unix_ms INTEGER NOT NULL,reason TEXT NOT NULL);
CREATE TABLE collector_cursors(repository_id TEXT PRIMARY KEY REFERENCES registrations(id) ON DELETE CASCADE,instance_id TEXT NOT NULL,sequence INTEGER NOT NULL,observed_at_unix_ms INTEGER NOT NULL);
CREATE TABLE jobs(id TEXT PRIMARY KEY,repository_id TEXT NOT NULL REFERENCES registrations(id) ON DELETE CASCADE,state TEXT NOT NULL,payload TEXT NOT NULL,updated_at_unix_ms INTEGER NOT NULL);
";
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_rolls_back_and_rejects_future_version() {
        let mut c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE metric_samples(id INTEGER)")
            .unwrap();
        assert!(migrate(&mut c).is_err());
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            0
        );
        assert!(c.prepare("SELECT * FROM registrations").is_err());
        c.pragma_update(None, "user_version", 2).unwrap();
        assert!(matches!(migrate(&mut c), Err(DbError::Version)));
    }
    #[test]
    fn legacy_migration_preserves_registration_and_marks_lost_history() {
        let mut c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE registrations(id TEXT PRIMARY KEY,name TEXT,repo_path TEXT,store_path TEXT,model_path TEXT,daemon_path TEXT); INSERT INTO registrations VALUES('r','r','/r','/s','/m','/d'); CREATE TABLE metric_samples(id INTEGER);").unwrap();
        migrate(&mut c).unwrap();
        assert_eq!(get(&c, "r").unwrap().id, "r");
        assert_eq!(
            c.query_row(
                "SELECT COUNT(*) FROM collection_gaps WHERE reason='schema_migration'",
                [],
                |r| r.get::<_, usize>(0)
            )
            .unwrap(),
            1
        );
    }
    #[tokio::test]
    async fn private_files_singleton_and_restart() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("state/db.sqlite");
        let d = Database::open(&path, 0).await.unwrap();
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(matches!(
            Database::open(&path, 0).await,
            Err(DbError::Locked)
        ));
        d.call(|c| {
            c.execute(
                "INSERT INTO registrations VALUES('r','r','/r','/s','/m','/d')",
                [],
            )?;
            c.execute("INSERT INTO collector_cursors VALUES('r','i',2,10)", [])?;
            Ok(())
        })
        .await
        .unwrap();
        drop(d);
        let d = loop {
            match Database::open(&path, 20).await {
                Ok(d) => break d,
                Err(DbError::Locked) => tokio::task::yield_now().await,
                Err(e) => panic!("{e}"),
            }
        };
        assert_eq!(d.list().await.unwrap().len(), 1);
        assert_eq!(d.cursor("r").await.unwrap().unwrap().sequence, 2);
        assert_eq!(
            d.gaps(0, 30, None).await.unwrap()[0].reason,
            "console_restart"
        );
    }
    #[tokio::test]
    async fn worker_opens_and_prunes() {
        let d = Database::memory(0).await.unwrap();
        assert!(d.list().await.unwrap().is_empty());
    }
}
