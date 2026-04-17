// Rust guideline compliant 2026-02-21
//! Pluggable byte-store backend for the cache.
//!
//! The [`Store`] trait abstracts where bytes go. The 0.1.0 release
//! ships [`LocalDiskStore`] (sharded by hex prefix); future releases
//! can add S3-compatible, Cloudflare Artifacts, OCI-distribution, and
//! other remote backends without changing the [`crate::ShardedCache`]
//! surface that consumes them.
//!
//! Each cache entry is a small set of named byte blobs:
//! `result.json` (metadata), `stdout`, `stderr`, etc. The [`Store`]
//! trait operates at the (key, name) level — higher-level concepts
//! like "this entry belongs to target X" live in
//! [`crate::ShardedCache`].

use std::fmt::Debug;
use std::path::{Path, PathBuf};

use crate::error::CacheError;
use crate::hash::CacheKey;

/// Pluggable byte-store for cache entries.
///
/// Implementations must be `Send + Sync` so they can be shared between
/// concurrent execution waves.
pub trait Store: Send + Sync + Debug {
    /// Reads the named blob for `key`. Returns `Ok(None)` on miss.
    ///
    /// # Errors
    ///
    /// Implementation-specific. [`CacheError::is_io`] for filesystem
    /// failures, [`CacheError::is_corruption`] for backend integrity
    /// problems.
    fn get(&self, key: &CacheKey, name: &str) -> Result<Option<Vec<u8>>, CacheError>;

    /// Writes `data` to the named blob under `key`, overwriting any prior value.
    ///
    /// # Errors
    ///
    /// Implementation-specific. [`CacheError::is_io`] for filesystem
    /// failures.
    fn put(&self, key: &CacheKey, name: &str, data: &[u8]) -> Result<(), CacheError>;

    /// Returns `true` if any blob is stored under `key`.
    fn contains(&self, key: &CacheKey) -> bool;

    /// Removes every blob stored under `key`.
    ///
    /// Returns `true` if at least one blob was present and removed,
    /// `false` if `key` had no entry to begin with.
    ///
    /// # Errors
    ///
    /// Implementation-specific. [`CacheError::is_io`] for filesystem
    /// failures.
    fn evict(&self, key: &CacheKey) -> Result<bool, CacheError>;
}

/// Local filesystem [`Store`] using a sharded directory layout.
///
/// Entries live at `{root}/{ab}/{cd...}` where `ab` is the first two
/// hex characters of the combined cache hash and `cd...` is the
/// remaining 62. The sharding keeps any one directory's child count
/// below ~257 even for very large caches, which keeps directory
/// scans fast on typical filesystems.
///
/// # Examples
///
/// ```no_run
/// use kiln_cache::{LocalDiskStore, Store, CacheKey, ContentHash};
///
/// let store = LocalDiskStore::new("/var/cache/kiln");
/// let key = CacheKey::new(
///     ContentHash::from_data(b"in"),
///     ContentHash::from_data(b"src"),
///     ContentHash::from_data(b"tool"),
/// );
///
/// store.put(&key, "result.json", b"{}").unwrap();
/// let bytes = store.get(&key, "result.json").unwrap().unwrap();
/// assert_eq!(bytes, b"{}");
/// ```
#[derive(Debug, Clone)]
pub struct LocalDiskStore {
    root: PathBuf,
}

impl LocalDiskStore {
    /// Constructs a `LocalDiskStore` rooted at `root`.
    ///
    /// The directory does not need to exist; it is created on the
    /// first [`put`](Self::put) operation.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns the root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the entry directory for `key` under the sharded layout.
    fn entry_dir(&self, key: &CacheKey) -> PathBuf {
        let hex = key.combined().to_hex();
        // The hex string is always 64 chars (assertion holds by construction
        // in ContentHash::to_hex), so split_at(2) is safe.
        let (shard, rest) = hex.split_at(2);
        self.root.join(shard).join(rest)
    }
}

impl Store for LocalDiskStore {
    fn get(&self, key: &CacheKey, name: &str) -> Result<Option<Vec<u8>>, CacheError> {
        let path = self.entry_dir(key).join(name);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(CacheError::io(err, Some(path))),
        }
    }

    fn put(&self, key: &CacheKey, name: &str, data: &[u8]) -> Result<(), CacheError> {
        let dir = self.entry_dir(key);
        std::fs::create_dir_all(&dir).map_err(|e| CacheError::io(e, Some(dir.clone())))?;
        let path = dir.join(name);
        std::fs::write(&path, data).map_err(|e| CacheError::io(e, Some(path)))?;
        Ok(())
    }

    fn contains(&self, key: &CacheKey) -> bool {
        self.entry_dir(key).exists()
    }

    fn evict(&self, key: &CacheKey) -> Result<bool, CacheError> {
        let dir = self.entry_dir(key);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| CacheError::io(e, Some(dir.clone())))?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::ContentHash;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("kiln-cache-store-{}-{}", std::process::id(), id));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn key() -> CacheKey {
        CacheKey::new(
            ContentHash::from_data(b"in"),
            ContentHash::from_data(b"src"),
            ContentHash::from_data(b"tool"),
        )
    }

    #[test]
    fn missing_blob_returns_none() {
        let dir = temp_root();
        let store = LocalDiskStore::new(&dir);
        assert!(store.get(&key(), "result.json").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn put_then_get_round_trips() {
        let dir = temp_root();
        let store = LocalDiskStore::new(&dir);
        store.put(&key(), "result.json", b"{}").unwrap();
        let bytes = store.get(&key(), "result.json").unwrap().unwrap();
        assert_eq!(bytes, b"{}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn put_creates_sharded_directory() {
        let dir = temp_root();
        let store = LocalDiskStore::new(&dir);
        store.put(&key(), "blob", b"x").unwrap();

        let hex = key().combined().to_hex();
        let (shard, rest) = hex.split_at(2);
        let entry_path = dir.join(shard).join(rest).join("blob");
        assert!(
            entry_path.exists(),
            "expected sharded entry at {entry_path:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn put_overwrites_existing_blob() {
        let dir = temp_root();
        let store = LocalDiskStore::new(&dir);
        store.put(&key(), "result.json", b"first").unwrap();
        store.put(&key(), "result.json", b"second").unwrap();
        let bytes = store.get(&key(), "result.json").unwrap().unwrap();
        assert_eq!(bytes, b"second");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn contains_after_put_is_true() {
        let dir = temp_root();
        let store = LocalDiskStore::new(&dir);
        assert!(!store.contains(&key()));
        store.put(&key(), "result.json", b"{}").unwrap();
        assert!(store.contains(&key()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn evict_existing_returns_true() {
        let dir = temp_root();
        let store = LocalDiskStore::new(&dir);
        store.put(&key(), "result.json", b"{}").unwrap();
        assert!(store.evict(&key()).unwrap());
        assert!(!store.contains(&key()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn evict_missing_returns_false() {
        let dir = temp_root();
        let store = LocalDiskStore::new(&dir);
        assert!(!store.evict(&key()).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_is_send_and_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LocalDiskStore>();
    }
}
