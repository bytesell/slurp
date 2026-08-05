use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tracing::{debug, warn};

// Two-level cache: L1 in-memory (moka), L2 on-disk with sidecar .meta TTL files.
pub struct ImageCache {
    memory: moka::future::Cache<String, Vec<u8>>,
    disk: Arc<DiskCache>,
}

/// The on-disk half. Behind an `Arc` so every blocking file operation can be moved onto the
/// blocking pool without copying the configuration.
struct DiskCache {
    dir: PathBuf,
    ttl: Duration,
    max_bytes: u64,
    /// Bytes written since the last sweep. The periodic sweeper bounds the cache only every
    /// `disk_sweep_interval_secs`; this counter triggers an out-of-band sweep when a burst of
    /// writes would otherwise blow past the budget in between.
    bytes_since_sweep: AtomicU64,
}

impl ImageCache {
    #[must_use]
    pub fn new(disk_dir: PathBuf, ttl_seconds: u64, memory_size: u64, max_disk_bytes: u64) -> Self {
        let memory = moka::future::Cache::builder()
            .max_capacity(memory_size)
            .time_to_live(Duration::from_secs(ttl_seconds))
            .build();

        Self {
            memory,
            disk: Arc::new(DiskCache {
                dir: disk_dir,
                ttl: Duration::from_secs(ttl_seconds),
                max_bytes: max_disk_bytes,
                bytes_since_sweep: AtomicU64::new(0),
            }),
        }
    }

    #[must_use]
    pub fn cache_key(src: &str, width: u32, format: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(src.as_bytes());
        hasher.update(b"\x00");
        hasher.update(width.to_le_bytes());
        hasher.update(b"\x00");
        hasher.update(format.as_bytes());
        hex::encode(hasher.finalize())
    }

    pub async fn get(&self, key: &str) -> Option<Vec<u8>> {
        if let Some(data) = self.memory.get(key).await {
            debug!(key, "cache hit (memory)");
            return Some(data);
        }

        // `std::fs::read` of a multi-megabyte cache entry is blocking work and must not run on an
        // async worker.
        let disk = self.disk.clone();
        let owned_key = key.to_string();
        let result = tokio::task::spawn_blocking(move || disk.disk_get(&owned_key)).await;

        match result {
            Ok(Ok(Some(data))) => {
                debug!(key, "cache hit (disk)");
                self.memory.insert(key.to_string(), data.clone()).await;
                Some(data)
            }
            Ok(Ok(None)) => None,
            Ok(Err(e)) => {
                warn!(key, error = %e, "disk cache read error");
                None
            }
            Err(e) => {
                warn!(key, error = %e, "disk cache read task failed");
                None
            }
        }
    }

    pub async fn set(&self, key: &str, data: Vec<u8>) {
        self.memory.insert(key.to_string(), data.clone()).await;

        // Blocking file I/O, as in `get`, and the write may additionally trigger an inline sweep.
        let disk = self.disk.clone();
        let owned_key = key.to_string();
        let joined = tokio::task::spawn_blocking(move || {
            if let Err(e) = disk.disk_set(&owned_key, &data) {
                warn!(key = %owned_key, error = %e, "disk cache write error");
            }
        })
        .await;
        if let Err(e) = joined {
            warn!(key, error = %e, "disk cache write task failed");
        }
    }

    /// Enforce the disk-cache byte budget. Blocking I/O - call from the background sweeper, never on
    /// the request path.
    pub fn sweep(&self, max_bytes: u64) {
        self.disk.sweep(max_bytes);
    }

    /// As [`sweep`](Self::sweep), with an explicit grace period before an orphaned data file may
    /// be reclaimed. Test seam: production always uses [`ORPHAN_GRACE`].
    #[cfg(test)]
    fn sweep_with_grace(&self, max_bytes: u64, orphan_grace: Duration) {
        self.disk.sweep_with_grace(max_bytes, orphan_grace);
    }
}

/// How long an orphaned data file (one with no `.meta` sidecar) is left alone before the sweep
/// reclaims it.
///
/// `disk_set` writes the data file and THEN the meta file, so between those two writes a
/// perfectly good entry looks exactly like an orphan. Reclaiming immediately would let a
/// concurrent sweep delete the bytes of an entry that is mid-write, leaving a meta file pointing
/// at nothing - a permanent cache miss for that key, and a thrash rather than a saving. The
/// window between the two writes is microseconds, so any grace period at all closes it; a minute
/// is chosen because the cost of waiting is bounded (the orphan is reclaimed on the next sweep)
/// and the cost of being wrong is not.
const ORPHAN_GRACE: Duration = Duration::from_secs(60);

impl DiskCache {
    /// Enforce the disk-cache byte budget. Deletes expired entries, then evicts the oldest remaining
    /// (soonest-to-expire, a proxy for least-recently-written) until the total is under `max_bytes`.
    /// Best-effort: per-entry I/O errors are skipped.
    fn sweep(&self, max_bytes: u64) {
        self.sweep_with_grace(max_bytes, ORPHAN_GRACE);
    }

    fn sweep_with_grace(&self, max_bytes: u64, orphan_grace: Duration) {
        self.bytes_since_sweep.store(0, Ordering::Relaxed);
        let Ok(read_dir) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // ONE listing, split into the two file kinds, because the sweep has to reason about both.
        //
        // THE SWEEP MUST ENUMERATE DATA FILES, not `.meta` sidecars with the data path derived from
        // them. A data file whose meta is missing is invisible to a meta-driven walk: never counted
        // toward the budget, never deleted, so the directory can sit permanently over its limit
        // with the sweeper reporting nothing to do. Those bytes are also unreachable, because
        // `disk_get` refuses an entry with no meta. `disk_set` writes data then meta, so every
        // crash or partial write between the two leaves one behind.
        let mut meta_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut data_files: Vec<PathBuf> = Vec::new();
        let mut metas: Vec<PathBuf> = Vec::new();
        for entry in read_dir.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            match name.strip_suffix(".meta") {
                Some(key) => {
                    meta_keys.insert(key.to_string());
                    metas.push(path);
                }
                None => data_files.push(path),
            }
        }

        // Reclaim orphaned data files first, so their bytes are gone before the budget is
        // measured rather than counted and then evicted alongside live entries.
        for data_path in &data_files {
            let Some(name) = data_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if meta_keys.contains(name) {
                continue;
            }
            // Inside the grace window this is far more likely to be an entry being written right
            // now than a leftover. See `ORPHAN_GRACE`.
            let young = std::fs::metadata(data_path)
                .and_then(|m| m.modified())
                .and_then(|t| {
                    t.elapsed()
                        .map_err(|e| std::io::Error::other(e.to_string()))
                })
                .map(|age| age < orphan_grace)
                .unwrap_or(false);
            if young {
                continue;
            }
            debug!(path = %data_path.display(), "reclaiming orphaned cache data file");
            let _ = std::fs::remove_file(data_path);
        }

        // (data_path, meta_path, size, expires_at) for every unexpired entry; expired ones are
        // deleted in the same pass.
        let mut live: Vec<(PathBuf, PathBuf, u64, u64)> = Vec::new();
        let mut total: u64 = 0;
        for meta_path in metas {
            let Some(key) = meta_path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".meta"))
            else {
                continue;
            };
            let data_path = self.dir.join(key);
            let expires_at = std::fs::read_to_string(&meta_path)
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(0);
            if now >= expires_at {
                let _ = std::fs::remove_file(&data_path);
                let _ = std::fs::remove_file(&meta_path);
                continue;
            }
            let size = std::fs::metadata(&data_path).map(|m| m.len()).unwrap_or(0);
            total += size;
            live.push((data_path, meta_path, size, expires_at));
        }

        if total <= max_bytes {
            return;
        }
        live.sort_by_key(|(_, _, _, expires_at)| *expires_at);
        for (data_path, meta_path, size, _) in live {
            if total <= max_bytes {
                break;
            }
            let _ = std::fs::remove_file(&data_path);
            let _ = std::fs::remove_file(&meta_path);
            total = total.saturating_sub(size);
        }
    }

    fn data_path(&self, key: &str) -> PathBuf {
        self.dir.join(key)
    }

    fn meta_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.meta"))
    }

    fn disk_get(&self, key: &str) -> Result<Option<Vec<u8>>, DiskCacheError> {
        let meta_path = self.meta_path(key);
        let data_path = self.data_path(key);

        if !meta_path.exists() || !data_path.exists() {
            return Ok(None);
        }

        let meta = std::fs::read_to_string(&meta_path).map_err(DiskCacheError::Io)?;
        let expires_at: u64 = meta
            .trim()
            .parse()
            .map_err(|_| DiskCacheError::CorruptMeta)?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(DiskCacheError::SystemTime)?
            .as_secs();

        if now >= expires_at {
            let _ = std::fs::remove_file(&data_path);
            let _ = std::fs::remove_file(&meta_path);
            return Ok(None);
        }

        let data = std::fs::read(&data_path).map_err(DiskCacheError::Io)?;
        Ok(Some(data))
    }

    fn disk_set(&self, key: &str, data: &[u8]) -> Result<(), DiskCacheError> {
        ensure_dir(&self.dir)?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(DiskCacheError::SystemTime)?
            .as_secs();

        let expires_at = now + self.ttl.as_secs();

        // Write data before meta, so a crash between the two leaves an orphaned data file rather
        // than a meta pointing at bytes that are not there. `disk_get` requires BOTH to exist, so
        // the orphan is unreachable rather than a corrupt hit.
        //
        // The orphan is not free, only unreachable: it still occupies disk. `sweep` is what
        // reclaims it, which is why that walk must enumerate data files - see the comment there.
        std::fs::write(self.data_path(key), data).map_err(DiskCacheError::Io)?;
        std::fs::write(self.meta_path(key), expires_at.to_string()).map_err(DiskCacheError::Io)?;

        // SECURITY (disk exhaustion): the periodic sweeper bounds the cache only once per
        // interval, so without this the budget could be blown wide open in between. Sweep out of
        // band once a burst has written a meaningful fraction of the budget. Already on the
        // blocking pool here, so this costs no async worker.
        let written = self
            .bytes_since_sweep
            .fetch_add(data.len() as u64, Ordering::Relaxed)
            + data.len() as u64;
        if written >= self.sweep_trigger_bytes() {
            self.sweep(self.max_bytes);
        }

        Ok(())
    }

    /// Sweep after an eighth of the budget has been written, floored so a tiny configured budget
    /// does not sweep on every single write.
    fn sweep_trigger_bytes(&self) -> u64 {
        (self.max_bytes / 8).max(4 * 1024 * 1024)
    }
}

fn ensure_dir(path: &Path) -> Result<(), DiskCacheError> {
    std::fs::create_dir_all(path).map_err(DiskCacheError::Io)
}

#[derive(Debug, thiserror::Error)]
enum DiskCacheError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Corrupt meta file")]
    CorruptMeta,
    #[error("System time error: {0}")]
    SystemTime(std::time::SystemTimeError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_cache(dir: &TempDir) -> ImageCache {
        ImageCache::new(dir.path().to_path_buf(), 3600, 64, 2 * 1024 * 1024 * 1024)
    }

    fn disk_bytes(dir: &TempDir) -> u64 {
        std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| !e.path().to_string_lossy().ends_with(".meta"))
            .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
            .sum()
    }

    #[test]
    fn cache_key_is_deterministic() {
        let k1 = ImageCache::cache_key("https://example.com/a.jpg", 800, "webp");
        let k2 = ImageCache::cache_key("https://example.com/a.jpg", 800, "webp");
        assert_eq!(k1, k2);
    }

    #[test]
    fn cache_key_differs_on_width() {
        let k1 = ImageCache::cache_key("https://example.com/a.jpg", 800, "webp");
        let k2 = ImageCache::cache_key("https://example.com/a.jpg", 400, "webp");
        assert_ne!(k1, k2);
    }

    #[test]
    fn cache_key_differs_on_format() {
        let k1 = ImageCache::cache_key("https://example.com/a.jpg", 800, "webp");
        let k2 = ImageCache::cache_key("https://example.com/a.jpg", 800, "avif");
        assert_ne!(k1, k2);
    }

    #[test]
    fn cache_key_differs_on_src() {
        let k1 = ImageCache::cache_key("https://example.com/a.jpg", 800, "webp");
        let k2 = ImageCache::cache_key("https://example.com/b.jpg", 800, "webp");
        assert_ne!(k1, k2);
    }

    #[tokio::test]
    async fn sweep_evicts_over_budget() {
        let dir = TempDir::new().unwrap();
        let cache = make_cache(&dir);
        for i in 0..5 {
            cache.set(&format!("k{i}"), vec![0u8; 1000]).await;
        }
        // Budget of 2500 bytes: sweep must evict data files down to <= budget.
        cache.sweep(2500);
        let total = disk_bytes(&dir);
        assert!(
            total <= 2500,
            "disk cache not bounded by sweep: {total} bytes"
        );
    }

    #[tokio::test]
    async fn disk_cache_is_bounded_between_scheduled_sweeps() {
        // The background sweeper only runs every `disk_sweep_interval_secs`, so a burst of writes
        // in between must not be able to exceed the budget. `sweep` is never called here: the
        // write path has to self-limit.
        let dir = TempDir::new().unwrap();
        let budget = 8 * 1024 * 1024; // trigger threshold floors at 4 MB, so this sweeps at 1 MB
        let cache = ImageCache::new(dir.path().to_path_buf(), 3600, 4, budget);
        for i in 0..48 {
            cache.set(&format!("burst{i}"), vec![0u8; 512 * 1024]).await;
        }
        // 48 x 512 KB is 24 MB written against an 8 MB budget.
        let total = disk_bytes(&dir);
        assert!(
            total <= budget,
            "disk cache grew past its budget between sweeps: {total} bytes"
        );
    }

    /// Pins that an orphaned data file - a payload whose `.meta` sidecar is missing - is both
    /// counted toward the disk budget and reclaimed. A sweep driven off `.meta` files cannot see
    /// one, which lets the directory sit permanently over `max_disk_cache_bytes` while reporting
    /// nothing to do. See the comment in `sweep`.
    #[tokio::test]
    async fn sweep_reclaims_orphaned_data_files() {
        let dir = TempDir::new().unwrap();
        let cache = make_cache(&dir);

        // A healthy entry, so the test can prove the sweep is selective rather than just
        // emptying the directory.
        cache.set("healthy", vec![0u8; 1000]).await;

        // An orphan, exactly as a crash between `disk_set`'s two writes would leave it.
        let orphan = dir.path().join("orphaned-key");
        std::fs::write(&orphan, vec![0u8; 4000]).unwrap();
        assert!(orphan.is_file());

        // Zero grace: production waits `ORPHAN_GRACE` so a concurrent write is not mistaken for
        // an orphan (see the const), but that window is not what this test is about.
        cache.sweep_with_grace(1024 * 1024, Duration::ZERO);

        assert!(
            !orphan.exists(),
            "the orphaned data file survived the sweep, so its bytes stay uncounted forever"
        );
        assert!(
            dir.path().join("healthy").is_file(),
            "the sweep deleted a healthy entry"
        );
        assert_eq!(
            cache.get("healthy").await,
            Some(vec![0u8; 1000]),
            "the healthy entry must still be readable"
        );
    }

    /// The grace window: a data file that has only just appeared is left alone, because that is
    /// what an entry being written RIGHT NOW looks like. Deleting it would strand a meta file
    /// pointing at nothing.
    #[tokio::test]
    async fn a_freshly_written_data_file_is_not_mistaken_for_an_orphan() {
        let dir = TempDir::new().unwrap();
        let cache = make_cache(&dir);
        let in_flight = dir.path().join("being-written");
        std::fs::write(&in_flight, vec![0u8; 4000]).unwrap();

        cache.sweep_with_grace(1024 * 1024, Duration::from_secs(60));

        assert!(
            in_flight.is_file(),
            "a data file inside the grace window was reclaimed; a concurrent disk_set would lose              its bytes between writing the data and writing the meta"
        );
    }

    /// Orphaned bytes count against the budget too, which is the point of reclaiming them: the
    /// budget is a statement about the DIRECTORY, not about the entries the sweeper can see.
    #[tokio::test]
    async fn orphaned_bytes_do_not_keep_the_directory_over_budget() {
        let dir = TempDir::new().unwrap();
        let cache = make_cache(&dir);
        for i in 0..8 {
            std::fs::write(dir.path().join(format!("orphan{i}")), vec![0u8; 100_000]).unwrap();
        }
        cache.set("healthy", vec![0u8; 1000]).await;

        cache.sweep_with_grace(50_000, Duration::ZERO);

        let total = disk_bytes(&dir);
        assert!(
            total <= 50_000,
            "directory is {total} bytes against a 50000 byte budget; orphans are still uncounted"
        );
    }

    #[tokio::test]
    async fn memory_round_trip() {
        let dir = TempDir::new().unwrap();
        let cache = make_cache(&dir);
        let key = "testkey";
        let data = vec![1u8, 2, 3, 4];

        assert!(cache.get(key).await.is_none());
        cache.set(key, data.clone()).await;
        assert_eq!(cache.get(key).await, Some(data));
    }

    #[tokio::test]
    async fn disk_persistence() {
        let dir = TempDir::new().unwrap();
        let data = vec![9u8, 8, 7];
        let key = "persisted";

        {
            let cache = make_cache(&dir);
            cache.set(key, data.clone()).await;
        }

        let cache2 = make_cache(&dir);
        assert_eq!(cache2.get(key).await, Some(data));
    }
}
