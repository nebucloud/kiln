// Rust guideline compliant 2026-02-21
//! Error type for kiln-cache.
//!
//! Cache failures are distinct enough from kiln-core's planning /
//! validation failures that they get their own situation-specific
//! struct per M-ERRORS-CANONICAL-STRUCTS. Callers query the kind via
//! the `is_*` boolean methods on [`CacheError`]; the [`ErrorKind`] enum
//! is `pub(crate)` so adding new variants is not a semver break.
//!
//! [`CacheError`] is `Send` so it composes with futures and tokio
//! tasks downstream in `kiln-exec`.

use std::backtrace::Backtrace;
use std::fmt::{self, Display, Formatter};
use std::io;
use std::path::PathBuf;

/// Errors raised by kiln-cache operations against a [`Store`](crate::Store).
///
/// Callers receive these from [`ShardedCache`](crate::ShardedCache)
/// operations and inspect the cause via the `is_*` query methods;
/// construction is crate-private.
///
/// # Examples
///
/// ```no_run
/// use kiln_cache::{CacheKey, ContentHash, LocalDiskStore, ShardedCache};
///
/// let cache = ShardedCache::new(LocalDiskStore::new("/var/cache/kiln"));
/// let key = CacheKey::new(
///     ContentHash::from_data(b"in"),
///     ContentHash::from_data(b"src"),
///     ContentHash::from_data(b"tool"),
/// );
///
/// match cache.lookup(&key) {
///     Ok(Some(hit)) => println!("cache hit: {}", hit.entry.target_id),
///     Ok(None) => println!("cache miss"),
///     Err(err) if err.is_io() => eprintln!("cache is on a broken disk"),
///     Err(err) if err.is_hash_mismatch() => eprintln!("cache corruption: {err}"),
///     Err(err) => eprintln!("cache error: {err}"),
/// }
/// ```
#[derive(Debug)]
pub struct CacheError {
    kind: ErrorKind,
    backtrace: Backtrace,
}

impl CacheError {
    /// Returns `true` when the underlying cause is a filesystem I/O failure.
    #[must_use]
    pub fn is_io(&self) -> bool {
        matches!(self.kind, ErrorKind::Io { .. })
    }

    /// Returns `true` when the cache entry exists but is structurally invalid.
    #[must_use]
    pub fn is_corruption(&self) -> bool {
        matches!(self.kind, ErrorKind::Corruption { .. })
    }

    /// Returns `true` when a stored entry's recorded hash does not match its key.
    #[must_use]
    pub fn is_hash_mismatch(&self) -> bool {
        matches!(self.kind, ErrorKind::HashMismatch { .. })
    }

    /// Returns `true` when JSON serialization or deserialization failed.
    #[must_use]
    pub fn is_json_error(&self) -> bool {
        matches!(self.kind, ErrorKind::Json { .. })
    }

    /// Returns the captured backtrace.
    pub fn backtrace(&self) -> &Backtrace {
        &self.backtrace
    }

    pub(crate) fn io(source: io::Error, path: impl Into<Option<PathBuf>>) -> Self {
        Self {
            kind: ErrorKind::Io {
                source,
                path: path.into(),
            },
            backtrace: Backtrace::capture(),
        }
    }

    pub(crate) fn corruption(context: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Corruption {
                context: context.into(),
            },
            backtrace: Backtrace::capture(),
        }
    }

    pub(crate) fn hash_mismatch(
        expected: crate::hash::ContentHash,
        actual: crate::hash::ContentHash,
    ) -> Self {
        Self {
            kind: ErrorKind::HashMismatch {
                expected: expected.to_hex(),
                actual: actual.to_hex(),
            },
            backtrace: Backtrace::capture(),
        }
    }

    pub(crate) fn json(source: serde_json::Error) -> Self {
        Self {
            kind: ErrorKind::Json { source },
            backtrace: Backtrace::capture(),
        }
    }
}

impl Display for CacheError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::Io { source, path } => match path {
                Some(p) => write!(f, "kiln-cache I/O error at `{}`: {source}", p.display()),
                None => write!(f, "kiln-cache I/O error: {source}"),
            },
            ErrorKind::Corruption { context } => {
                write!(f, "kiln-cache entry corruption: {context}")
            }
            ErrorKind::HashMismatch { expected, actual } => {
                write!(
                    f,
                    "kiln-cache hash mismatch: expected `{expected}`, found `{actual}`"
                )
            }
            ErrorKind::Json { source } => {
                write!(f, "kiln-cache JSON error: {source}")
            }
        }
    }
}

impl std::error::Error for CacheError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::Io { source, .. } => Some(source),
            ErrorKind::Json { source } => Some(source),
            ErrorKind::Corruption { .. } | ErrorKind::HashMismatch { .. } => None,
        }
    }
}

#[derive(Debug)]
pub(crate) enum ErrorKind {
    Io {
        source: io::Error,
        path: Option<PathBuf>,
    },
    Corruption {
        context: String,
    },
    HashMismatch {
        expected: String,
        actual: String,
    },
    Json {
        source: serde_json::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::ContentHash;

    #[test]
    fn io_error_query() {
        let err = CacheError::io(io::Error::other("nope"), Some(PathBuf::from("/tmp/x")));
        assert!(err.is_io());
        assert!(!err.is_corruption());
        assert!(format!("{err}").contains("/tmp/x"));
    }

    #[test]
    fn corruption_query() {
        let err = CacheError::corruption("bad data");
        assert!(err.is_corruption());
        assert!(format!("{err}").contains("bad data"));
    }

    #[test]
    fn hash_mismatch_query_and_message() {
        let expected = ContentHash::from_data(b"one");
        let actual = ContentHash::from_data(b"two");
        let err = CacheError::hash_mismatch(expected, actual);
        assert!(err.is_hash_mismatch());

        let rendered = err.to_string();
        assert!(rendered.contains(&expected.to_hex()));
        assert!(rendered.contains(&actual.to_hex()));
    }

    #[test]
    fn json_error_chains_through_source() {
        let json_err = serde_json::from_str::<i32>("not valid").expect_err("invalid");
        let err = CacheError::json(json_err);
        assert!(err.is_json_error());
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn error_is_send() {
        const fn assert_send<T: Send>() {}
        assert_send::<CacheError>();
    }
}
