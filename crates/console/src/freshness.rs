use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Freshness {
    pub head: Option<String>,
    pub indexed_commit: Option<String>,
    pub dirty: Option<bool>,
    pub unavailable_reason: Option<String>,
    pub inspected_at_unix_ms: u64,
}
#[derive(Clone, Default)]
pub struct FreshnessCache {
    cache: Arc<Mutex<BTreeMap<String, (Instant, Freshness)>>>,
    worker: Arc<Mutex<Option<Arc<Semaphore>>>>,
}
impl FreshnessCache {
    pub async fn forget(&self, id: &str) {
        self.cache.lock().await.remove(id);
    }
    pub async fn inspect(
        &self,
        id: String,
        path: std::path::PathBuf,
        indexed_commit: Option<String>,
    ) -> Freshness {
        let mut cache = self.cache.lock().await;
        if let Some((time, entry)) = cache.get(&id)
            && time.elapsed() < Duration::from_secs(5)
        {
            let mut result = entry.clone();
            result.indexed_commit = indexed_commit;
            return result;
        }
        let semaphore = self
            .worker
            .lock()
            .await
            .get_or_insert_with(|| Arc::new(Semaphore::new(1)))
            .clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let now = crate::now_ms();
        let result = tokio::time::timeout(Duration::from_secs(5), async move {
            let permit = semaphore.acquire_owned().await.map_err(|_| ())?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                inspect_path(&path, flag)
            })
            .await
            .map_err(|_| ())?
        })
        .await;
        let (head, dirty, unavailable_reason) = match result {
            Ok(Ok((head, dirty))) => (head, Some(dirty), None),
            Ok(Err(())) => (None, None, Some("inspection_unavailable".into())),
            Err(_) => {
                cancelled.store(true, Ordering::Relaxed);
                (None, None, Some("inspection_timeout".into()))
            }
        };
        let value = Freshness {
            head,
            indexed_commit,
            dirty,
            unavailable_reason,
            inspected_at_unix_ms: now,
        };
        cache.insert(id, (Instant::now(), value.clone()));
        value
    }
}
fn inspect_path(
    path: &std::path::Path,
    interrupt: Arc<AtomicBool>,
) -> Result<(Option<String>, bool), ()> {
    let repo = gix::open(path).map_err(|_| ())?;
    let head = repo
        .head()
        .map_err(|_| ())?
        .try_peel_to_id_in_place()
        .map_err(|_| ())?
        .map(|id| id.to_string());
    let mut dirty = false;
    let status = repo
        .status(gix::progress::Discard)
        .map_err(|_| ())?
        .untracked_files(gix::status::UntrackedFiles::Files)
        .should_interrupt_owned(interrupt)
        .into_iter(Vec::new())
        .map_err(|_| ())?;
    for item in status {
        item.map_err(|_| ())?;
        dirty = true;
    }
    Ok((head, dirty))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn git(path: &std::path::Path, args: &[&str]) {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    #[tokio::test]
    async fn freshness_distinguishes_unborn_untracked_clean_and_unknown() {
        let t = tempfile::tempdir().unwrap();
        git(t.path(), &["init", "-q"]);
        let cache = FreshnessCache::default();
        let a = cache.inspect("a".into(), t.path().into(), None).await;
        assert_eq!(a.head, None);
        assert_eq!(a.dirty, Some(false));
        std::fs::write(t.path().join("untracked"), "x").unwrap();
        cache.forget("a").await;
        assert_eq!(
            cache.inspect("a".into(), t.path().into(), None).await.dirty,
            Some(true)
        );
        git(t.path(), &["add", "."]);
        git(
            t.path(),
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                "fixture",
            ],
        );
        cache.forget("a").await;
        let a = cache.inspect("a".into(), t.path().into(), None).await;
        assert!(a.head.is_some());
        assert_eq!(a.dirty, Some(false));
        let unknown = cache
            .inspect("b".into(), t.path().join("missing"), None)
            .await;
        assert_eq!(unknown.dirty, None);
        assert!(unknown.unavailable_reason.is_some());
    }
}
