// Rust guideline compliant 2026-02-21
// Ported from terranoxos/terranox-tools/crates/lattice-exec/src/cache.rs
// (commit 7ab5316ac6 baseline). `ExecutionCache` -> `ShardedCache` per
// KLN-PLAN-extraction §5 M3. Adapted to delegate byte storage to the
// `Store` trait so backends are pluggable, and to use kiln-core's
// stripped Target type (Vec<String> inputs, no LatticeType). See
// KLN-PLAN-extraction §6 for the provenance convention.
//! High-level content-addressed cache.
//!
//! Wraps any [`Store`] implementation in an API tailored to caching
//! [`Target`] execution results. Each entry holds metadata
//! ([`CacheEntry`]) plus captured stdout / stderr.

use std::time::{SystemTime, UNIX_EPOCH};

use kiln_core::Target;
use serde::{Deserialize, Serialize};

use crate::error::CacheError;
use crate::hash::{CacheKey, ContentHash};
use crate::store::Store;

/// File names used inside an entry directory.
const RESULT_FILE: &str = "result.json";
const STDOUT_FILE: &str = "stdout";
const STDERR_FILE: &str = "stderr";

/// Metadata stored alongside captured output for each cache entry.
///
/// `key` records the combined cache hash of the entry. On read,
/// [`ShardedCache::lookup`] re-derives the expected hash from the
/// supplied [`CacheKey`] and rejects the entry with
/// [`CacheError::is_hash_mismatch`] if the two disagree. That guards
/// against accidental cross-contamination from corrupted store layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    /// Combined hex cache key under which this entry is stored.
    pub key: String,
    /// The target id that produced this entry (for diagnostics).
    pub target_id: String,
    /// Process exit code captured at execution time.
    pub exit_code: i32,
    /// Unix epoch seconds when the entry was written.
    pub stored_at: u64,
}

/// A complete cached execution result: metadata plus captured streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedResult {
    /// Metadata (key, target id, exit code, timestamp).
    pub entry: CacheEntry,
    /// Captured standard output. Empty string when no stdout was recorded.
    pub stdout: String,
    /// Captured standard error. Empty string when no stderr was recorded.
    pub stderr: String,
}

/// Content-addressed execution cache backed by a pluggable [`Store`].
///
/// Each cached target writes three blobs into the store under the
/// combined [`CacheKey`] hash: `result.json` (metadata), `stdout`, and
/// `stderr`. Lookups re-validate the recorded hash against the
/// computed one to defend against corruption.
///
/// "Sharded" refers to the storage layout: the default
/// [`LocalDiskStore`](crate::LocalDiskStore) splits entries into 256
/// hex-prefix shards so no single directory holds the entire cache.
/// Other backends are free to implement their own partitioning.
///
/// # Examples
///
/// ```no_run
/// use kiln_cache::{LocalDiskStore, ShardedCache};
/// use kiln_core::{Target, ShellBlock};
///
/// let cache = ShardedCache::new(LocalDiskStore::new("/var/cache/kiln"));
/// let target = Target::new(ShellBlock::new("bash", "echo hi"));
/// let key = cache.compute_key(&target);
///
/// // Miss path: nothing in the cache yet.
/// assert!(cache.lookup(&key).unwrap().is_none());
///
/// // Store an execution outcome and look it up.
/// cache.store(&key, "demo", 0, "hi\n", "").unwrap();
/// let hit = cache.lookup(&key).unwrap().unwrap();
/// assert_eq!(hit.entry.exit_code, 0);
/// assert_eq!(hit.stdout, "hi\n");
/// ```
#[derive(Debug, Clone)]
pub struct ShardedCache<S> {
    store: S,
}

impl<S: Store> ShardedCache<S> {
    /// Constructs a cache backed by `store`.
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Borrows the underlying [`Store`] backend.
    ///
    /// Useful for tests and for backends that expose extra inspection
    /// APIs beyond the [`Store`] trait.
    pub fn backend(&self) -> &S {
        &self.store
    }

    /// Computes the [`CacheKey`] for a target.
    ///
    /// Equivalent to [`CacheKey::from_target`] — exposed here so callers
    /// don't need to import the `hash` module.
    #[must_use]
    pub fn compute_key(&self, target: &Target) -> CacheKey {
        CacheKey::from_target(target)
    }

    /// Looks up a cached execution result.
    ///
    /// Returns `Ok(None)` on miss, `Ok(Some(_))` on a valid hit.
    ///
    /// # Errors
    ///
    /// - [`CacheError::is_io`] for filesystem failures inside the
    ///   underlying [`Store`].
    /// - [`CacheError::is_corruption`] when the entry has metadata
    ///   but is structurally invalid.
    /// - [`CacheError::is_json_error`] when `result.json` cannot be
    ///   parsed.
    /// - [`CacheError::is_hash_mismatch`] when the stored hex key
    ///   disagrees with the computed combined hash.
    pub fn lookup(&self, key: &CacheKey) -> Result<Option<CachedResult>, CacheError> {
        let Some(result_bytes) = self.store.get(key, RESULT_FILE)? else {
            return Ok(None);
        };

        let entry: CacheEntry = serde_json::from_slice(&result_bytes).map_err(CacheError::json)?;

        let expected = key.combined();
        let actual = ContentHash::from_hex(&entry.key)?;
        if actual != expected {
            return Err(CacheError::hash_mismatch(expected, actual));
        }

        let stdout = self.read_string(key, STDOUT_FILE)?;
        let stderr = self.read_string(key, STDERR_FILE)?;

        Ok(Some(CachedResult {
            entry,
            stdout,
            stderr,
        }))
    }

    /// Stores an execution result, overwriting any prior entry at `key`.
    ///
    /// Returns the [`CacheEntry`] metadata that was written.
    ///
    /// # Errors
    ///
    /// - [`CacheError::is_io`] for filesystem failures.
    /// - [`CacheError::is_json_error`] when serializing the metadata
    ///   fails (only possible if `target_id` somehow contains
    ///   non-finite UTF-16 surrogate halves; in practice this never
    ///   fires for regular target ids).
    pub fn store(
        &self,
        key: &CacheKey,
        target_id: &str,
        exit_code: i32,
        stdout: &str,
        stderr: &str,
    ) -> Result<CacheEntry, CacheError> {
        let stored_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();

        let entry = CacheEntry {
            key: key.combined().to_hex(),
            target_id: target_id.to_owned(),
            exit_code,
            stored_at,
        };

        let result_bytes = serde_json::to_vec_pretty(&entry).map_err(CacheError::json)?;
        self.store.put(key, RESULT_FILE, &result_bytes)?;
        self.store.put(key, STDOUT_FILE, stdout.as_bytes())?;
        self.store.put(key, STDERR_FILE, stderr.as_bytes())?;

        Ok(entry)
    }

    /// Returns `true` if the cache holds an entry for `key`.
    pub fn contains(&self, key: &CacheKey) -> bool {
        self.store.contains(key)
    }

    /// Removes the entry at `key`. Returns `true` when it existed.
    ///
    /// # Errors
    ///
    /// - [`CacheError::is_io`] when the underlying store fails to
    ///   remove the entry.
    pub fn evict(&self, key: &CacheKey) -> Result<bool, CacheError> {
        self.store.evict(key)
    }

    /// Reads a UTF-8 blob, returning the empty string when missing.
    ///
    /// stdout/stderr files are optional from the consumer's viewpoint —
    /// a target that produced no stderr should look the same as one
    /// that wrote zero bytes to stderr.
    fn read_string(&self, key: &CacheKey, name: &str) -> Result<String, CacheError> {
        let Some(bytes) = self.store.get(key, name)? else {
            return Ok(String::new());
        };
        String::from_utf8(bytes).map_err(|err| {
            CacheError::corruption(format!("non-UTF-8 bytes in `{name}` blob: {err}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::LocalDiskStore;
    use kiln_core::ShellBlock;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_cache() -> (ShardedCache<LocalDiskStore>, PathBuf) {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("kiln-cache-{}-{}", std::process::id(), id));
        let _ = std::fs::remove_dir_all(&dir);
        (ShardedCache::new(LocalDiskStore::new(&dir)), dir)
    }

    fn target(code: &str) -> Target {
        Target::new(ShellBlock::new("bash", code))
    }

    #[test]
    fn lookup_miss_returns_none() {
        let (cache, dir) = temp_cache();
        let key = cache.compute_key(&target("make"));
        assert!(cache.lookup(&key).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_then_lookup_round_trips() {
        let (cache, dir) = temp_cache();
        let key = cache.compute_key(&target("make"));
        let entry = cache.store(&key, "build", 0, "hello\n", "warn\n").unwrap();
        assert_eq!(entry.target_id, "build");
        assert_eq!(entry.exit_code, 0);
        assert!(entry.stored_at > 0);

        let hit = cache.lookup(&key).unwrap().unwrap();
        assert_eq!(hit.entry, entry);
        assert_eq!(hit.stdout, "hello\n");
        assert_eq!(hit.stderr, "warn\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_overwrites_existing_entry() {
        let (cache, dir) = temp_cache();
        let key = cache.compute_key(&target("make"));
        cache.store(&key, "build", 0, "first", "").unwrap();
        cache.store(&key, "build", 1, "second", "err").unwrap();

        let hit = cache.lookup(&key).unwrap().unwrap();
        assert_eq!(hit.entry.exit_code, 1);
        assert_eq!(hit.stdout, "second");
        assert_eq!(hit.stderr, "err");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn distinct_targets_get_distinct_entries() {
        let (cache, dir) = temp_cache();
        let k1 = cache.compute_key(&target("make all"));
        let k2 = cache.compute_key(&target("make clean"));
        cache.store(&k1, "all", 0, "all", "").unwrap();
        cache.store(&k2, "clean", 0, "clean", "").unwrap();

        let h1 = cache.lookup(&k1).unwrap().unwrap();
        let h2 = cache.lookup(&k2).unwrap().unwrap();
        assert_eq!(h1.stdout, "all");
        assert_eq!(h2.stdout, "clean");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn contains_reports_presence() {
        let (cache, dir) = temp_cache();
        let key = cache.compute_key(&target("make"));
        assert!(!cache.contains(&key));
        cache.store(&key, "build", 0, "", "").unwrap();
        assert!(cache.contains(&key));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn evict_removes_entry() {
        let (cache, dir) = temp_cache();
        let key = cache.compute_key(&target("make"));
        cache.store(&key, "build", 0, "", "").unwrap();
        assert!(cache.evict(&key).unwrap());
        assert!(!cache.contains(&key));
        assert!(cache.lookup(&key).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn evict_missing_returns_false() {
        let (cache, dir) = temp_cache();
        let key = cache.compute_key(&target("make"));
        assert!(!cache.evict(&key).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupted_result_json_surfaces_json_error() {
        let (cache, dir) = temp_cache();
        let key = cache.compute_key(&target("make"));
        cache.store.put(&key, RESULT_FILE, b"not json").unwrap();
        let err = cache.lookup(&key).expect_err("malformed result.json");
        assert!(err.is_json_error());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tampered_key_in_metadata_surfaces_hash_mismatch() {
        let (cache, dir) = temp_cache();
        let key = cache.compute_key(&target("make"));

        // Manually write a result.json whose `key` field disagrees with
        // the actual combined hash. ContentHash::from_data(b"bogus") gives
        // us a deterministically wrong hex key.
        let bogus = ContentHash::from_data(b"bogus").to_hex();
        let entry = CacheEntry {
            key: bogus,
            target_id: "build".to_owned(),
            exit_code: 0,
            stored_at: 1,
        };
        let bytes = serde_json::to_vec(&entry).unwrap();
        cache.store.put(&key, RESULT_FILE, &bytes).unwrap();

        let err = cache.lookup(&key).expect_err("hash must mismatch");
        assert!(err.is_hash_mismatch());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_is_send_and_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ShardedCache<LocalDiskStore>>();
    }
}
