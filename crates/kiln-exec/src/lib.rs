// Rust guideline compliant 2026-02-21
//! Hermetic execution engine for kiln pipelines.
//!
//! kiln-exec runs the targets a [`kiln_core::Pipeline`] declares: it
//! consults the [`kiln_cache::ShardedCache`] for prior outcomes,
//! resolves shell interpreters, sets up a per-target [`Sandbox`], and
//! drives the run + cleanup blocks via the [`runner`] module
//! (delivered in M4 part 2).
//!
//! # 0.1.0 scope
//!
//! M4 part 1 (this drop) ships the foundations:
//!
//! - [`ExecError`] — single situation-specific error per
//!   M-ERRORS-CANONICAL-STRUCTS, with [`From`] conversions from
//!   [`kiln_cache::CacheError`] and [`kiln_core::KilnError`].
//! - [`ShellBackend`] — interpreter resolution against the host
//!   `PATH`.
//! - [`Sandbox`] / [`SandboxConfig`] — per-target workspace, env
//!   sanitization, and (Linux only) optional namespace isolation
//!   sealing the network per [KLN-D-04][kln-d-04].
//!
//! M4 part 2 ships the [`runner`] module (fork / exec / capture /
//! timeout), the `fetch` module (fetch-then-seal phase), and the
//! [`executor`] facade that composes cache + sandbox + runner into
//! one call.
//!
//! Filesystem isolation via `pivot_root`, cgroup memory / CPU limits,
//! and PID-namespace isolation are deferred to kiln 0.2+. The
//! corresponding [`SandboxConfig`] fields are accepted today as
//! forward-compatible API surface.
//!
//! [kln-d-04]: https://github.com/nebucloud/docs/blob/main/KLN-D-extraction-decisions.md

pub mod error;
pub mod fetch;
pub mod runner;
pub mod sandbox;
pub mod shell_backend;

#[doc(inline)]
pub use crate::error::ExecError;
#[doc(inline)]
pub use crate::fetch::{Fetcher, MockFetcher};
#[doc(inline)]
pub use crate::runner::{run_target, run_target_cached, RunResult};
#[doc(inline)]
pub use crate::sandbox::{can_isolate_namespaces, Sandbox, SandboxConfig};
#[doc(inline)]
pub use crate::shell_backend::ShellBackend;
