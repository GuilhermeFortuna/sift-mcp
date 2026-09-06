use std::cell::Cell;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const RETENTION_MILLIS: u64 = 7 * 24 * 60 * 60 * 1_000;
pub const EVENT_CAP: usize = 100_000;
pub const METRIC_CAP: usize = 100_000;
pub const REPORT_CAP_PER_REPOSITORY: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestEvent {
    pub repository_id: String,
    pub instance_id: String,
    pub sequence: u64,
    pub completed_at_unix_ms: u64,
    pub operation: String,
    pub elapsed_micros: u64,
    pub outcome: String,
    pub error_code: Option<String>,
    pub result_count: Option<u64>,
}

#[derive(Debug, Error)]
pub enum HistoryError {
    #[error("history database error: {0}")]
    Database(String),
}
pub struct History {
    conn: Connection,
    event_count: Cell<usize>,
    metric_count: Cell<usize>,
}

impl History {
    pub fn open_in_memory() -> Result<Self, HistoryError> {
        Self::open(Connection::open_in_memory().map_err(db)?)
    }
    pub fn open_path(path: &std::path::Path) -> Result<Self, HistoryError> {
        Self::open(Connection::open(path).map_err(db)?)
    }
    fn open(conn: Connection) -> Result<Self, HistoryError> {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS request_events (id INTEGER PRIMARY KEY, repository_id TEXT NOT NULL, instance_id TEXT NOT NULL, sequence INTEGER NOT NULL, completed_at_unix_ms INTEGER NOT NULL, operation TEXT NOT NULL, elapsed_micros INTEGER NOT NULL, outcome TEXT NOT NULL, error_code TEXT, result_count INTEGER, UNIQUE(repository_id, instance_id, sequence));
            CREATE INDEX IF NOT EXISTS request_events_completed_at ON request_events(completed_at_unix_ms);
            CREATE TABLE IF NOT EXISTS metric_samples (id INTEGER PRIMARY KEY, repository_id TEXT NOT NULL, sampled_at_unix_ms INTEGER NOT NULL, payload TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS metric_samples_sampled_at ON metric_samples(sampled_at_unix_ms);
            CREATE TABLE IF NOT EXISTS index_reports (id INTEGER PRIMARY KEY, repository_id TEXT NOT NULL, completed_at_unix_ms INTEGER NOT NULL, payload TEXT NOT NULL);
        ").map_err(db)?;
        let event_count = conn
            .query_row("SELECT COUNT(*) FROM request_events", [], |row| {
                row.get::<_, usize>(0)
            })
            .map_err(db)?;
        let metric_count = conn
            .query_row("SELECT COUNT(*) FROM metric_samples", [], |row| {
                row.get::<_, usize>(0)
            })
            .map_err(db)?;
        Ok(Self {
            conn,
            event_count: Cell::new(event_count),
            metric_count: Cell::new(metric_count),
        })
    }
    pub fn record_event(
        &self,
        event: &RequestEvent,
        now_unix_ms: u64,
    ) -> Result<bool, HistoryError> {
        let inserted = self.conn.execute("INSERT OR IGNORE INTO request_events (repository_id, instance_id, sequence, completed_at_unix_ms, operation, elapsed_micros, outcome, error_code, result_count) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)", params![event.repository_id, event.instance_id, event.sequence, event.completed_at_unix_ms, event.operation, event.elapsed_micros, event.outcome, event.error_code, event.result_count]).map_err(db)? == 1;
        if inserted {
            self.event_count.set(self.event_count.get() + 1);
        }
        let cutoff = now_unix_ms.saturating_sub(RETENTION_MILLIS);
        let expired = self
            .conn
            .execute(
                "DELETE FROM request_events WHERE completed_at_unix_ms < ?1",
                [cutoff],
            )
            .map_err(db)?;
        self.event_count
            .set(self.event_count.get().saturating_sub(expired));
        if self.event_count.get() > EVENT_CAP {
            self.conn.execute("DELETE FROM request_events WHERE id IN (SELECT id FROM request_events ORDER BY completed_at_unix_ms, id LIMIT ?1)", [self.event_count.get() as i64 - EVENT_CAP as i64]).map_err(db)?;
            self.event_count.set(EVENT_CAP);
        }
        Ok(inserted)
    }
    pub fn events(&self) -> Result<Vec<RequestEvent>, HistoryError> {
        let mut statement = self.conn.prepare("SELECT repository_id, instance_id, sequence, completed_at_unix_ms, operation, elapsed_micros, outcome, error_code, result_count FROM request_events ORDER BY completed_at_unix_ms, id").map_err(db)?;
        statement
            .query_map([], |r| {
                Ok(RequestEvent {
                    repository_id: r.get(0)?,
                    instance_id: r.get(1)?,
                    sequence: r.get(2)?,
                    completed_at_unix_ms: r.get(3)?,
                    operation: r.get(4)?,
                    elapsed_micros: r.get(5)?,
                    outcome: r.get(6)?,
                    error_code: r.get(7)?,
                    result_count: r.get(8)?,
                })
            })
            .map_err(db)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db)
    }
    pub fn record_metric(
        &self,
        repository_id: &str,
        sampled_at_unix_ms: u64,
        payload: &str,
        now_unix_ms: u64,
    ) -> Result<(), HistoryError> {
        self.conn.execute("INSERT INTO metric_samples (repository_id, sampled_at_unix_ms, payload) VALUES (?1, ?2, ?3)", params![repository_id, sampled_at_unix_ms, payload]).map_err(db)?;
        self.metric_count.set(self.metric_count.get() + 1);
        let cutoff = now_unix_ms.saturating_sub(RETENTION_MILLIS);
        let expired = self
            .conn
            .execute(
                "DELETE FROM metric_samples WHERE sampled_at_unix_ms < ?1",
                [cutoff],
            )
            .map_err(db)?;
        self.metric_count
            .set(self.metric_count.get().saturating_sub(expired));
        if self.metric_count.get() > METRIC_CAP {
            self.conn.execute("DELETE FROM metric_samples WHERE id IN (SELECT id FROM metric_samples ORDER BY sampled_at_unix_ms, id LIMIT ?1)", [self.metric_count.get() as i64 - METRIC_CAP as i64]).map_err(db)?;
            self.metric_count.set(METRIC_CAP);
        }
        Ok(())
    }
    pub fn record_report(
        &self,
        repository_id: &str,
        completed_at_unix_ms: u64,
        payload: &str,
        now_unix_ms: u64,
    ) -> Result<(), HistoryError> {
        self.conn.execute("INSERT INTO index_reports (repository_id, completed_at_unix_ms, payload) VALUES (?1, ?2, ?3)", params![repository_id, completed_at_unix_ms, payload]).map_err(db)?;
        let cutoff = now_unix_ms.saturating_sub(RETENTION_MILLIS);
        self.conn
            .execute(
                "DELETE FROM index_reports WHERE completed_at_unix_ms < ?1",
                [cutoff],
            )
            .map_err(db)?;
        self.conn.execute("DELETE FROM index_reports WHERE id IN (SELECT id FROM (SELECT id, ROW_NUMBER() OVER (PARTITION BY repository_id ORDER BY completed_at_unix_ms DESC, id DESC) AS rank FROM index_reports) WHERE rank > ?1)", [REPORT_CAP_PER_REPOSITORY as i64]).map_err(db)?;
        Ok(())
    }
    pub fn prune(&self, now_unix_ms: u64) -> Result<(), HistoryError> {
        let cutoff = now_unix_ms.saturating_sub(RETENTION_MILLIS);
        let expired = self
            .conn
            .execute(
                "DELETE FROM request_events WHERE completed_at_unix_ms < ?1",
                [cutoff],
            )
            .map_err(db)?;
        self.event_count
            .set(self.event_count.get().saturating_sub(expired));
        self.conn
            .execute(
                "DELETE FROM metric_samples WHERE sampled_at_unix_ms < ?1",
                [cutoff],
            )
            .map_err(db)?;
        self.conn
            .execute(
                "DELETE FROM index_reports WHERE completed_at_unix_ms < ?1",
                [cutoff],
            )
            .map_err(db)?;
        cap(&self.conn, "request_events", EVENT_CAP)?;
        cap(&self.conn, "metric_samples", METRIC_CAP)?;
        self.metric_count.set(
            self.conn
                .query_row("SELECT COUNT(*) FROM metric_samples", [], |row| {
                    row.get::<_, usize>(0)
                })
                .map_err(db)?,
        );
        self.conn.execute("DELETE FROM index_reports WHERE id IN (SELECT id FROM (SELECT id, ROW_NUMBER() OVER (PARTITION BY repository_id ORDER BY completed_at_unix_ms DESC, id DESC) AS rank FROM index_reports) WHERE rank > ?1)", [REPORT_CAP_PER_REPOSITORY as i64]).map_err(db)?;
        Ok(())
    }
}
fn cap(conn: &Connection, table: &str, limit: usize) -> Result<(), HistoryError> {
    conn.execute(&format!("DELETE FROM {table} WHERE id IN (SELECT id FROM {table} ORDER BY id DESC LIMIT -1 OFFSET ?1)"), [limit as i64]).map_err(db)?;
    Ok(())
}
fn db(error: rusqlite::Error) -> HistoryError {
    HistoryError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(sequence: u64, completed_at_unix_ms: u64) -> RequestEvent {
        RequestEvent {
            repository_id: "repo".into(),
            instance_id: "instance".into(),
            sequence,
            completed_at_unix_ms,
            operation: "search".into(),
            elapsed_micros: 12,
            outcome: "ok".into(),
            error_code: None,
            result_count: Some(1),
        }
    }
    #[test]
    fn persists_safe_events_and_deduplicates_a_reconnected_cursor() {
        let h = History::open_in_memory().unwrap();
        assert!(
            h.record_event(&event(1, 100), RETENTION_MILLIS + 100)
                .unwrap()
        );
        assert!(
            !h.record_event(&event(1, 100), RETENTION_MILLIS + 100)
                .unwrap()
        );
        assert_eq!(h.events().unwrap(), vec![event(1, 100)]);
    }
    #[test]
    fn expires_oldest_events_and_enforces_global_cap() {
        let h = History::open_in_memory().unwrap();
        assert!(h.record_event(&event(1, 1), RETENTION_MILLIS + 2).unwrap());
        assert!(h.events().unwrap().is_empty());
        for n in 0..=EVENT_CAP {
            h.record_event(
                &event(n as u64, RETENTION_MILLIS + n as u64),
                RETENTION_MILLIS * 2,
            )
            .unwrap();
        }
        let events = h.events().unwrap();
        assert_eq!(events.len(), EVENT_CAP);
        assert_eq!(events[0].sequence, 1);
    }
    #[test]
    fn retains_metric_and_report_bounds_without_event_payloads() {
        let h = History::open_in_memory().unwrap();
        for n in 0..=METRIC_CAP {
            h.record_metric(
                "repo",
                RETENTION_MILLIS + n as u64,
                "gpu unavailable",
                RETENTION_MILLIS * 2,
            )
            .unwrap();
        }
        for n in 0..=REPORT_CAP_PER_REPOSITORY {
            h.record_report(
                "repo",
                RETENTION_MILLIS + n as u64,
                "safe summary",
                RETENTION_MILLIS * 2,
            )
            .unwrap();
        }
        let metrics: usize = h
            .conn
            .query_row("SELECT COUNT(*) FROM metric_samples", [], |r| r.get(0))
            .unwrap();
        let reports: usize = h
            .conn
            .query_row("SELECT COUNT(*) FROM index_reports", [], |r| r.get(0))
            .unwrap();
        assert_eq!(metrics, METRIC_CAP);
        assert_eq!(reports, REPORT_CAP_PER_REPOSITORY);
    }
}
