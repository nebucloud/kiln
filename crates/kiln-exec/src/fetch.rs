// Rust guideline compliant 2026-02-21
//! Fetch-then-seal phase per [KLN-D-04][kln-d-04].
//!
//! kiln targets seal the network by default. To bring external
//! resources (crate downloads, container blobs, registries) into the
//! sandbox they declare them as fetches; the runner pulls each fetch
//! before the network is sealed and verifies its content hash. The
//! `run` block then executes with no outbound network access.
//!
//! 0.1.0 ships the [`Fetcher`] trait and a [`MockFetcher`] used by
//! the test suite. A real HTTP implementation is intentionally not
//! bundled — consumers plug in their own (the SSF use case in
//! particular wants its own Cosign-aware client). M-MOCKABLE-SYSCALLS
//! treats outbound network as the most important surface to keep
//! mockable, and a dependency-free Fetcher trait is the sharpest way
//! to enforce that.
//!
//! [kln-d-04]: https://github.com/nebucloud/docs/blob/main/KLN-D-extraction-decisions.md

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Mutex;

use kiln_cache::ContentHash;

use crate::error::ExecError;

/// Pluggable resource fetcher.
///
/// Implementations must be `Send + Sync` so they can be reused across
/// concurrent execution waves.
///
/// `expected_blake3` is the expected BLAKE3 digest of the response
/// body. When provided, the fetcher must reject the response on
/// mismatch — kiln uses this to make fetches deterministic and
/// content-addressable on the cache side.
pub trait Fetcher: Send + Sync + Debug {
    /// Fetches `url` and returns its bytes, optionally verifying the BLAKE3 digest.
    ///
    /// # Errors
    ///
    /// Returns [`ExecError::is_fetch_failed`] for transport failures,
    /// non-2xx responses, or hash mismatches.
    fn fetch(&self, url: &str, expected_blake3: Option<&ContentHash>)
        -> Result<Vec<u8>, ExecError>;
}

/// In-memory [`Fetcher`] for tests and offline pipelines.
///
/// Pre-register responses with [`MockFetcher::insert`]; the fetcher
/// returns them verbatim and verifies their hash when an expected
/// digest is supplied.
#[derive(Debug, Default)]
pub struct MockFetcher {
    responses: Mutex<HashMap<String, Vec<u8>>>,
}

impl MockFetcher {
    /// Constructs an empty `MockFetcher`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-registers a response body for `url`.
    ///
    /// Replaces any prior body registered for the same URL.
    ///
    /// # Panics
    ///
    /// Panics if the inner [`Mutex`] is poisoned (a previous
    /// `insert`/`fetch` call panicked while holding the lock).
    pub fn insert(&self, url: impl Into<String>, body: impl Into<Vec<u8>>) {
        let mut guard = self
            .responses
            .lock()
            .expect("MockFetcher mutex was poisoned");
        guard.insert(url.into(), body.into());
    }
}

impl Fetcher for MockFetcher {
    fn fetch(
        &self,
        url: &str,
        expected_blake3: Option<&ContentHash>,
    ) -> Result<Vec<u8>, ExecError> {
        let body = {
            let guard = self.responses.lock().map_err(|err| {
                ExecError::fetch_failed(url, format!("mock mutex poisoned: {err}"))
            })?;
            guard
                .get(url)
                .cloned()
                .ok_or_else(|| ExecError::fetch_failed(url, "no mock response registered"))?
        };

        if let Some(expected) = expected_blake3 {
            let actual = ContentHash::from_data(&body);
            if &actual != expected {
                return Err(ExecError::fetch_failed(
                    url,
                    format!("BLAKE3 mismatch: expected {expected}, got {actual}"),
                ));
            }
        }

        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_fetcher_returns_registered_body() {
        let fetcher = MockFetcher::new();
        fetcher.insert("https://example/x", b"hello".to_vec());
        let body = fetcher.fetch("https://example/x", None).unwrap();
        assert_eq!(body, b"hello");
    }

    #[test]
    fn mock_fetcher_unknown_url_is_fetch_failed() {
        let fetcher = MockFetcher::new();
        let err = fetcher.fetch("https://example/missing", None).unwrap_err();
        assert!(err.is_fetch_failed());
        assert!(format!("{err}").contains("https://example/missing"));
    }

    #[test]
    fn mock_fetcher_verifies_blake3_match() {
        let fetcher = MockFetcher::new();
        fetcher.insert("https://example/x", b"hello".to_vec());
        let expected = ContentHash::from_data(b"hello");
        let body = fetcher.fetch("https://example/x", Some(&expected)).unwrap();
        assert_eq!(body, b"hello");
    }

    #[test]
    fn mock_fetcher_rejects_blake3_mismatch() {
        let fetcher = MockFetcher::new();
        fetcher.insert("https://example/x", b"hello".to_vec());
        let wrong = ContentHash::from_data(b"goodbye");
        let err = fetcher
            .fetch("https://example/x", Some(&wrong))
            .unwrap_err();
        assert!(err.is_fetch_failed());
        assert!(format!("{err}").contains("BLAKE3 mismatch"));
    }

    #[test]
    fn mock_fetcher_replaces_existing_body() {
        let fetcher = MockFetcher::new();
        fetcher.insert("https://example/x", b"first".to_vec());
        fetcher.insert("https://example/x", b"second".to_vec());
        let body = fetcher.fetch("https://example/x", None).unwrap();
        assert_eq!(body, b"second");
    }

    #[test]
    fn fetcher_trait_is_object_safe() {
        // Compile-time assertion: trait can be used as `dyn Fetcher`.
        let _ = std::mem::size_of::<&dyn Fetcher>();
    }

    #[test]
    fn types_are_send_and_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MockFetcher>();
    }
}
