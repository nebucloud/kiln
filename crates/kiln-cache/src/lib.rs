// Rust guideline compliant 2026-02-21
//! Content-addressed caching for kiln pipelines.
//!
//! kiln-cache is the second foundation crate of the workspace. It
//! turns a [`Target`](kiln_core::Target) plus its declared inputs into
//! a deterministic [`CacheKey`] and stores execution results under
//! that key via a pluggable [`Store`] backend.
//!
//! Hash inputs per the lattice convention:
//!
//! - `input_hash` — BLAKE3 over the newline-joined input name list
//! - `script_hash` — BLAKE3 over `target.run.code`
//! - `tool_hash` — BLAKE3 over `target.run.interpreter`
//!
//! Combined: `BLAKE3(input || script || tool)`.
//!
//! # Example
//!
//! ```no_run
//! use kiln_cache::{LocalDiskStore, ShardedCache};
//! use kiln_core::{ShellBlock, Target};
//!
//! let cache = ShardedCache::new(LocalDiskStore::new("/var/cache/kiln"));
//! let target = Target::new(ShellBlock::new("bash", "echo hi"));
//! let key = cache.compute_key(&target);
//!
//! if cache.lookup(&key).unwrap().is_none() {
//!     // Execute the target out-of-band, then store its outcome.
//!     cache.store(&key, "demo", 0, "hi\n", "").unwrap();
//! }
//! ```
//!
//! # Backends
//!
//! The 0.1.0 release ships [`LocalDiskStore`], a sharded filesystem
//! backend that splits entries into 256 hex-prefix directories so no
//! single directory holds the entire cache. Additional backends
//! (S3-compatible, Cloudflare Artifacts, OCI distribution) can be
//! added by implementing the [`Store`] trait without touching
//! [`ShardedCache`].

pub mod cache;
pub mod error;
pub mod hash;
pub mod store;

#[doc(inline)]
pub use crate::cache::{CacheEntry, CachedResult, ShardedCache};
#[doc(inline)]
pub use crate::error::CacheError;
#[doc(inline)]
pub use crate::hash::{CacheKey, ContentHash};
#[doc(inline)]
pub use crate::store::{LocalDiskStore, Store};
