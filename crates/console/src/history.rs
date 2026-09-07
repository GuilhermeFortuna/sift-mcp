use crate::db::{Database, DbError};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
pub const RETENTION_MILLIS: u64 = 7 * 24 * 60 * 60 * 1000;
pub const EVENT_CAP: usize = 100_000;
pub const METRIC_CAP: usize = 100_000;
pub const REPORT_CAP_PER_REPOSITORY: usize = 100;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestEvent {
    pub repository_id: String,
    #[serde(flatten)]
    pub event: daemon::RequestEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceSample {
    pub repository_id: String,
    pub resources: daemon::ResourceSnapshot,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Gap {
    pub repository_id: String,
    pub from_unix_ms: u64,
    pub to_unix_ms: u64,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricsResponse {
    pub buckets: Vec<MetricBucket>,
    pub coverage_seconds: f64,
    pub gap_markers: Vec<Gap>,
    pub sample_count: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricBucket {
    pub from_unix_ms: u64,
    pub to_unix_ms: u64,
    pub request_count: usize,
    pub sample_count: usize,
    pub coverage_seconds: f64,
    pub rate_per_second: Option<f64>,
    pub p50_micros: Option<u64>,
    pub p95_micros: Option<u64>,
    pub resources: Vec<ResourceSample>,
}

pub async fn metrics(
    db: &Database,
    from: u64,
    to: u64,
    id: Option<String>,
) -> Result<MetricsResponse, DbError> {
    let events = db.events(from, to, id.clone()).await?;
    let samples = db.samples(from, to, id.clone()).await?;
    let gaps = db.gaps(from, to, id).await?;
    let mut buckets = Vec::new();
    let mut start = from - from % 60_000;
    while start <= to {
        let end = start.saturating_add(60_000).min(to.saturating_add(1));
        let in_bucket: Vec<_> = events
            .iter()
            .filter(|e| e.event.completed_at_unix_ms >= start && e.event.completed_at_unix_ms < end)
            .collect();
        let resources: Vec<_> = samples
            .iter()
            .filter(|s| {
                s.resources.sampled_at_unix_ms >= start && s.resources.sampled_at_unix_ms < end
            })
            .cloned()
            .collect();
        let mut durations: Vec<_> = in_bucket.iter().map(|e| e.event.elapsed_micros).collect();
        durations.sort_unstable();
        let coverage_ms = resources
            .windows(2)
            .map(|w| {
                w[1].resources
                    .sampled_at_unix_ms
                    .saturating_sub(w[0].resources.sampled_at_unix_ms)
                    .min(60_000)
            })
            .sum::<u64>();
        let coverage_seconds = coverage_ms as f64 / 1000.0;
        buckets.push(MetricBucket {
            from_unix_ms: start,
            to_unix_ms: end,
            request_count: in_bucket.len(),
            sample_count: resources.len(),
            coverage_seconds,
            rate_per_second: (coverage_seconds > 0.0)
                .then(|| in_bucket.len() as f64 / coverage_seconds),
            p50_micros: nearest(&durations, 50),
            p95_micros: nearest(&durations, 95),
            resources,
        });
        if end > to || end == start {
            break;
        }
        start = end;
    }
    Ok(MetricsResponse {
        coverage_seconds: buckets.iter().map(|b| b.coverage_seconds).sum(),
        sample_count: samples.len(),
        buckets,
        gap_markers: gaps,
    })
}
fn nearest(values: &[u64], percentile: usize) -> Option<u64> {
    if values.is_empty() {
        None
    } else {
        Some(values[((values.len() * percentile).div_ceil(100)).saturating_sub(1)])
    }
}
impl Database {
    pub async fn cursor(&self, id: &str) -> Result<Option<daemon::EventCursor>, DbError> {
        let id = id.to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                "SELECT instance_id,sequence FROM collector_cursors WHERE repository_id=?1",
                [id],
                |r| {
                    Ok(daemon::EventCursor {
                        instance_id: r.get(0)?,
                        sequence: r.get(1)?,
                    })
                },
            )
            .optional()?)
        })
        .await
    }
    pub async fn ingest(&self, id: &str, o: daemon::Observation, now: u64) -> Result<(), DbError> {
        let id = id.to_owned();
        self.call(move|c|{
  let tx=c.transaction()?;
  let previous:Option<(String,u64)>=tx.query_row("SELECT instance_id,observed_at_unix_ms FROM collector_cursors WHERE repository_id=?1",[&id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
  if o.gap || previous.as_ref().is_some_and(|(instance,_)| instance!=&o.next_cursor.instance_id){
   let reason=if previous.as_ref().is_some_and(|(instance,_)|instance!=&o.next_cursor.instance_id){"daemon_restart"}else{"buffer_loss"};
   insert_gap(&tx,&id,previous.as_ref().map_or(now,|(_,t)|*t),now,reason)?;
  }
  for event in o.events {
   let mut event=event;
   event.operation=safe_operation(&event.operation).into();
   event.outcome=safe_outcome(&event.outcome).into();
   event.error_code=event.error_code.as_deref().map(safe_error).map(str::to_owned);
   let safe=RequestEvent{repository_id:id.clone(),event};
   tx.execute("INSERT OR IGNORE INTO request_events(repository_id,instance_id,sequence,completed_at_unix_ms,payload) VALUES(?1,?2,?3,?4,?5)",params![id,safe.event.cursor.instance_id,safe.event.cursor.sequence,safe.event.completed_at_unix_ms,json(&safe)?])?;
  }
  let mut resources=o.status.resources;
  // A device identifier is metadata, but arbitrary daemon strings are not retained.
  resources.device_id=resources.device_id.filter(|s|s.len()<=80 && s.chars().all(|c|c.is_ascii_alphanumeric() || "-_: .".contains(c)));
  tx.execute("INSERT INTO metric_samples(repository_id,sampled_at_unix_ms,payload) VALUES(?1,?2,?3)",params![id,resources.sampled_at_unix_ms,json(&resources)?])?;
  if let Some(mut last)=o.status.last_index {
   last.outcome=safe_outcome(&last.outcome).into();last.error_code=last.error_code.as_deref().map(safe_error).map(str::to_owned);
   if let Some(report)=&mut last.report && (report.commit.len()>64 || !report.commit.chars().all(|c|c.is_ascii_hexdigit())){report.commit.clear();}
   tx.execute("INSERT OR IGNORE INTO index_reports(repository_id,instance_id,completed_at_unix_ms,payload) VALUES(?1,?2,?3,?4)",params![id,o.next_cursor.instance_id,last.completed_at_unix_ms,json(&last)?])?;
  }
  tx.execute("INSERT INTO collector_cursors VALUES(?1,?2,?3,?4) ON CONFLICT(repository_id) DO UPDATE SET instance_id=excluded.instance_id,sequence=excluded.sequence,observed_at_unix_ms=excluded.observed_at_unix_ms",params![id,o.next_cursor.instance_id,o.next_cursor.sequence,now])?;
  prune(&tx,now)?;tx.commit()?;Ok(())
 }).await
    }
    pub async fn gap(&self, id: &str, from: u64, to: u64, reason: &str) -> Result<(), DbError> {
        let id = id.to_owned();
        let reason = reason.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            insert_gap(&tx, &id, from, to, &reason)?;
            prune(&tx, to)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
    pub async fn events(
        &self,
        from: u64,
        to: u64,
        id: Option<String>,
    ) -> Result<Vec<RequestEvent>, DbError> {
        self.call(move|c|{let mut s=c.prepare("SELECT payload FROM request_events WHERE completed_at_unix_ms>=?1 AND completed_at_unix_ms<=?2 AND (?3 IS NULL OR repository_id=?3) ORDER BY completed_at_unix_ms,id")?; let rows=s.query_map(params![from,to,id],|r|r.get::<_,String>(0))?;rows.map(|r|parse(&r?)).collect()}).await
    }
    pub async fn samples(
        &self,
        from: u64,
        to: u64,
        id: Option<String>,
    ) -> Result<Vec<ResourceSample>, DbError> {
        self.call(move|c|{let mut s=c.prepare("SELECT repository_id,payload FROM metric_samples WHERE sampled_at_unix_ms>=?1 AND sampled_at_unix_ms<=?2 AND (?3 IS NULL OR repository_id=?3) ORDER BY sampled_at_unix_ms,id")?;let rows=s.query_map(params![from,to,id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?;rows.map(|r|{let(id,p)=r?;Ok(ResourceSample{repository_id:id,resources:parse(&p)?})}).collect()}).await
    }
    pub async fn gaps(&self, from: u64, to: u64, id: Option<String>) -> Result<Vec<Gap>, DbError> {
        self.call(move|c|{let mut s=c.prepare("SELECT repository_id,from_unix_ms,to_unix_ms,reason FROM collection_gaps WHERE to_unix_ms>=?1 AND from_unix_ms<=?2 AND (?3 IS NULL OR repository_id=?3) ORDER BY from_unix_ms,id")?;Ok(s.query_map(params![from,to,id],|r|Ok(Gap{repository_id:r.get(0)?,from_unix_ms:r.get(1)?,to_unix_ms:r.get(2)?,reason:r.get(3)?}))?.collect::<Result<_,_>>()?)}).await
    }
}
fn json(v: &impl Serialize) -> Result<String, DbError> {
    serde_json::to_string(v).map_err(|_| DbError::Metadata)
}
fn parse<T: serde::de::DeserializeOwned>(v: &str) -> Result<T, DbError> {
    serde_json::from_str(v).map_err(|_| DbError::Metadata)
}
fn insert_gap(c: &Connection, id: &str, from: u64, to: u64, reason: &str) -> Result<(), DbError> {
    let reason = match reason {
        "daemon_restart" | "buffer_loss" | "console_restart" | "collection_outage" => reason,
        _ => "collection_outage",
    };
    c.execute("INSERT INTO collection_gaps(repository_id,from_unix_ms,to_unix_ms,reason) VALUES(?1,?2,?3,?4)",params![id,from.min(to),to,reason])?;
    Ok(())
}
pub(crate) fn prune(c: &Connection, now: u64) -> Result<(), DbError> {
    let cutoff = now.saturating_sub(RETENTION_MILLIS);
    for (table, time, cap) in [
        ("request_events", "completed_at_unix_ms", EVENT_CAP),
        ("metric_samples", "sampled_at_unix_ms", METRIC_CAP),
        ("collection_gaps", "to_unix_ms", 10_000),
    ] {
        c.execute(&format!("DELETE FROM {table} WHERE {time}<?1"), [cutoff])?;
        c.execute(&format!("DELETE FROM {table} WHERE id IN(SELECT id FROM {table} ORDER BY {time} DESC,id DESC LIMIT -1 OFFSET ?1)"),[cap])?;
    }
    c.execute(
        "DELETE FROM index_reports WHERE completed_at_unix_ms<?1",
        [cutoff],
    )?;
    c.execute("DELETE FROM index_reports WHERE id IN(SELECT id FROM(SELECT id,ROW_NUMBER() OVER(PARTITION BY repository_id ORDER BY completed_at_unix_ms DESC,id DESC) AS n FROM index_reports) WHERE n>100)",[])?;
    Ok(())
}
fn safe_operation(v: &str) -> &str {
    match v {
        "Search" | "SearchSimilar" | "GetSymbol" | "Index" | "Status" | "Shutdown" | "Hello" => v,
        _ => "unknown",
    }
}
fn safe_outcome(v: &str) -> &str {
    match v {
        "ok" | "error" | "success" | "failed" | "interrupted" => v,
        _ => "unknown",
    }
}
fn safe_error(v: &str) -> &str {
    match v {
        "protocol_version" | "starting" | "index_in_progress" | "symbol_not_found"
        | "symbol_ambiguous" | "store_stale" | "gpu_unavailable" | "request_too_large"
        | "malformed" | "internal" | "observer_forbidden" => v,
        _ => "operation_failed",
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use daemon::{DaemonStatus, EventCursor, Lifecycle, Observation, ResourceSnapshot};
    async fn database() -> Database {
        let d = Database::memory(0).await.unwrap();
        d.call(|c|{c.execute("INSERT INTO registrations VALUES('repo','name','/repo','/store','/model','/daemon')",[])?;Ok(())}).await.unwrap();
        d
    }
    fn observation(sequence: u64, time: u64) -> Observation {
        Observation {
            status: DaemonStatus {
                lifecycle: Lifecycle::Ready,
                instance_id: "a".into(),
                observed_at_unix_ms: time,
                model_id: None,
                chunks_live: None,
                chunks_dead: None,
                indexed_commit: None,
                idle_seconds: 0,
                uptime_seconds: 0,
                current_progress: None,
                last_index: None,
                resources: ResourceSnapshot::unavailable(time),
            },
            events: vec![daemon::RequestEvent {
                cursor: EventCursor {
                    instance_id: "a".into(),
                    sequence,
                },
                connection_id: 3,
                request_id: 4,
                completed_at_unix_ms: time,
                operation: "Search".into(),
                elapsed_micros: 12,
                outcome: "ok".into(),
                error_code: None,
                result_count: Some(1),
                stage_millis: None,
            }],
            next_cursor: EventCursor {
                instance_id: "a".into(),
                sequence,
            },
            gap: false,
            more: false,
        }
    }
    #[tokio::test]
    async fn dedup_and_failed_transaction_never_advance_cursor() {
        let d = database().await;
        d.ingest("repo", observation(1, 10), 10).await.unwrap();
        d.ingest("repo", observation(1, 10), 10).await.unwrap();
        assert_eq!(d.events(0, 20, None).await.unwrap().len(), 1);
        d.call(|c|{c.execute_batch("CREATE TRIGGER fail_metric BEFORE INSERT ON metric_samples BEGIN SELECT RAISE(ABORT,'injected'); END;")?;Ok(())}).await.unwrap();
        assert!(d.ingest("repo", observation(2, 11), 11).await.is_err());
        assert_eq!(d.cursor("repo").await.unwrap().unwrap().sequence, 1);
        assert_eq!(d.events(0, 20, None).await.unwrap().len(), 1);
    }
    #[tokio::test]
    async fn private_input_is_not_persisted_and_wire_fields_survive() {
        let d = database().await;
        let mut o = observation(1, 10);
        o.events[0].operation = "PRIVATE query text".into();
        o.events[0].error_code = Some("PRIVATE raw error".into());
        o.status.resources.device_id = Some("PRIVATE\ncode".into());
        d.ingest("repo", o, 10).await.unwrap();
        let events = d.events(0, 20, None).await.unwrap();
        assert_eq!(events[0].event.connection_id, 3);
        let dump=d.call(|c|{let mut s=c.prepare("SELECT payload FROM request_events UNION ALL SELECT payload FROM metric_samples")?;Ok(s.query_map([],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?.join(""))}).await.unwrap();
        assert!(!dump.contains("PRIVATE"));
    }
    #[tokio::test]
    async fn retention_caps_all_tables_by_timestamp_and_cascades() {
        let d = database().await;
        d.call(|c|{let tx=c.transaction()?;
  tx.execute_batch("WITH RECURSIVE n(x) AS(VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<100000) INSERT INTO request_events(repository_id,instance_id,sequence,completed_at_unix_ms,payload) SELECT 'repo','a',x,100000-x,'{}' FROM n; WITH RECURSIVE n(x) AS(VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<100000) INSERT INTO metric_samples(repository_id,sampled_at_unix_ms,payload) SELECT 'repo',100000-x,'{}' FROM n; WITH RECURSIVE n(x) AS(VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<100) INSERT INTO index_reports(repository_id,instance_id,completed_at_unix_ms,payload) SELECT 'repo','a',100-x,'{}' FROM n; WITH RECURSIVE n(x) AS(VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<10000) INSERT INTO collection_gaps(repository_id,from_unix_ms,to_unix_ms,reason) SELECT 'repo',0,10000-x,'collection_outage' FROM n;")?;prune(&tx,100001)?;
  for (t,count,time) in [("request_events",100000,"completed_at_unix_ms"),("metric_samples",100000,"sampled_at_unix_ms"),("index_reports",100,"completed_at_unix_ms"),("collection_gaps",10000,"to_unix_ms")]{assert_eq!(tx.query_row(&format!("SELECT COUNT(*) FROM {t}"),[],|r|r.get::<_,usize>(0))?,count);assert_eq!(tx.query_row(&format!("SELECT MIN({time}) FROM {t}"),[],|r|r.get::<_,u64>(0))?,1);}
  prune(&tx,RETENTION_MILLIS+100002)?;for t in ["request_events","metric_samples","index_reports","collection_gaps"]{assert_eq!(tx.query_row(&format!("SELECT COUNT(*) FROM {t}"),[],|r|r.get::<_,usize>(0))?,0);}tx.commit()?;Ok(())}).await.unwrap();
        d.ingest("repo", observation(1, 10), 10).await.unwrap();
        d.remove("repo").await.unwrap();
        assert!(d.events(0, 100, None).await.unwrap().is_empty());
        assert!(d.cursor("repo").await.unwrap().is_none());
    }
    #[tokio::test]
    async fn reconnect_restart_and_buffer_loss_are_gaps() {
        let d = database().await;
        d.ingest("repo", observation(1, 10), 10).await.unwrap();
        let mut o = observation(2, 20);
        o.gap = true;
        d.ingest("repo", o, 20).await.unwrap();
        let mut o = observation(3, 30);
        o.next_cursor.instance_id = "b".into();
        d.ingest("repo", o, 30).await.unwrap();
        let gaps = d.gaps(0, 40, None).await.unwrap();
        assert_eq!(gaps.len(), 2);
        assert_eq!(gaps[0].reason, "buffer_loss");
        assert_eq!(gaps[1].reason, "daemon_restart");
    }
    #[test]
    fn percentile_is_nearest_rank_and_empty_is_missing() {
        let values: Vec<u64> = (1..=100).collect();
        assert_eq!(nearest(&values, 95), Some(95));
        assert_eq!(nearest(&[], 95), None);
    }
}
