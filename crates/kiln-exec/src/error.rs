// Rust guideline compliant 2026-02-21
//! Error type for kiln-exec.
//!
//! Following M-ERRORS-CANONICAL-STRUCTS, kiln-exec exposes a single
//! [`ExecError`] struct with an opaque internal kind. Callers query the
//! cause via the `is_*` boolean methods and reach the underlying
//! [`kiln_cache::CacheError`] / [`kiln_core::KilnError`] (when present)
//! through [`std::error::Error::source`].
//!
//! [`ExecError`] is `Send` so it composes with futures and tokio
//! tasks.

use std::backtrace::Backtrace;
use std::fmt::{self, Display, Formatter};
use std::io;
use std::path::PathBuf;

use kiln_cache::CacheError;
use kiln_core::KilnError;

/// Errors raised by kiln-exec when running a [`kiln_core::Target`].
///
/// # Examples
///
/// ```no_run
/// use kiln_exec::ExecError;
///
/// fn handle(err: ExecError) {
///     if err.is_target_failed() {
///         eprintln!("target failed: {err}");
///     } else if err.is_target_timeout() {
///         eprintln!("target timed out: {err}");
///     } else if err.is_interpreter_not_found() {
///         eprintln!("missing interpreter: {err}");
///     } else {
///         eprintln!("kiln-exec error: {err}");
///     }
/// }
/// ```
#[derive(Debug)]
pub struct ExecError {
    // Boxed to keep `Result<T, ExecError>` small (clippy::result_large_err).
    // The inner enum is large because of the embedded io::Error / CacheError /
    // KilnError variants; consumers shouldn't pay for that size on the happy path.
    kind: Box<ErrorKind>,
    backtrace: Backtrace,
}

impl ExecError {
    /// Returns `true` when the underlying cause is a filesystem or process I/O failure.
    #[must_use]
    pub fn is_io(&self) -> bool {
        matches!(*self.kind, ErrorKind::Io { .. })
    }

    /// Returns `true` when the requested interpreter could not be resolved on the host.
    #[must_use]
    pub fn is_interpreter_not_found(&self) -> bool {
        matches!(*self.kind, ErrorKind::InterpreterNotFound { .. })
    }

    /// Returns `true` when the target's run block exited with a non-zero status.
    #[must_use]
    pub fn is_target_failed(&self) -> bool {
        matches!(*self.kind, ErrorKind::TargetFailed { .. })
    }

    /// Returns `true` when the target was killed for exceeding its wall-clock timeout.
    #[must_use]
    pub fn is_target_timeout(&self) -> bool {
        matches!(*self.kind, ErrorKind::TargetTimeout { .. })
    }

    /// Returns `true` when sandbox setup failed (workspace, namespace, or env).
    #[must_use]
    pub fn is_sandbox_setup(&self) -> bool {
        matches!(*self.kind, ErrorKind::SandboxSetup { .. })
    }

    /// Returns `true` when a fetch step failed (network, hash mismatch, etc.).
    #[must_use]
    pub fn is_fetch_failed(&self) -> bool {
        matches!(*self.kind, ErrorKind::FetchFailed { .. })
    }

    /// Returns `true` when the underlying cause is a [`CacheError`].
    #[must_use]
    pub fn is_cache_error(&self) -> bool {
        matches!(*self.kind, ErrorKind::Cache(_))
    }

    /// Returns `true` when the underlying cause is a [`KilnError`].
    #[must_use]
    pub fn is_core_error(&self) -> bool {
        matches!(*self.kind, ErrorKind::Core(_))
    }

    /// Returns the captured backtrace.
    pub fn backtrace(&self) -> &Backtrace {
        &self.backtrace
    }

    pub(crate) fn io(source: io::Error, context: impl Into<String>) -> Self {
        Self {
            kind: Box::new(ErrorKind::Io {
                source,
                context: context.into(),
            }),
            backtrace: Backtrace::capture(),
        }
    }

    pub(crate) fn interpreter_not_found(name: impl Into<String>) -> Self {
        Self {
            kind: Box::new(ErrorKind::InterpreterNotFound { name: name.into() }),
            backtrace: Backtrace::capture(),
        }
    }

    pub(crate) fn target_failed(target: impl Into<String>, exit_code: i32) -> Self {
        Self {
            kind: Box::new(ErrorKind::TargetFailed {
                target: target.into(),
                exit_code,
            }),
            backtrace: Backtrace::capture(),
        }
    }

    pub(crate) fn target_timeout(target: impl Into<String>, elapsed_ms: u64) -> Self {
        Self {
            kind: Box::new(ErrorKind::TargetTimeout {
                target: target.into(),
                elapsed_ms,
            }),
            backtrace: Backtrace::capture(),
        }
    }

    pub(crate) fn sandbox_setup(context: impl Into<String>, path: Option<PathBuf>) -> Self {
        Self {
            kind: Box::new(ErrorKind::SandboxSetup {
                context: context.into(),
                path,
            }),
            backtrace: Backtrace::capture(),
        }
    }

    pub(crate) fn fetch_failed(url: impl Into<String>, context: impl Into<String>) -> Self {
        Self {
            kind: Box::new(ErrorKind::FetchFailed {
                url: url.into(),
                context: context.into(),
            }),
            backtrace: Backtrace::capture(),
        }
    }
}

impl Display for ExecError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &*self.kind {
            ErrorKind::Io { source, context } => {
                write!(f, "kiln-exec I/O error ({context}): {source}")
            }
            ErrorKind::InterpreterNotFound { name } => {
                write!(f, "interpreter `{name}` not found on PATH")
            }
            ErrorKind::TargetFailed { target, exit_code } => {
                write!(f, "target `{target}` exited with code {exit_code}")
            }
            ErrorKind::TargetTimeout { target, elapsed_ms } => {
                write!(
                    f,
                    "target `{target}` killed after {elapsed_ms}ms (wall-clock timeout)"
                )
            }
            ErrorKind::SandboxSetup { context, path } => match path {
                Some(p) => write!(f, "sandbox setup failed ({context}) at `{}`", p.display()),
                None => write!(f, "sandbox setup failed: {context}"),
            },
            ErrorKind::FetchFailed { url, context } => {
                write!(f, "fetch of `{url}` failed: {context}")
            }
            ErrorKind::Cache(err) => write!(f, "cache error: {err}"),
            ErrorKind::Core(err) => write!(f, "kiln-core error: {err}"),
        }
    }
}

impl std::error::Error for ExecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &*self.kind {
            ErrorKind::Io { source, .. } => Some(source),
            ErrorKind::Cache(err) => Some(err),
            ErrorKind::Core(err) => Some(err),
            ErrorKind::InterpreterNotFound { .. }
            | ErrorKind::TargetFailed { .. }
            | ErrorKind::TargetTimeout { .. }
            | ErrorKind::SandboxSetup { .. }
            | ErrorKind::FetchFailed { .. } => None,
        }
    }
}

impl From<CacheError> for ExecError {
    fn from(value: CacheError) -> Self {
        Self {
            kind: Box::new(ErrorKind::Cache(value)),
            backtrace: Backtrace::capture(),
        }
    }
}

impl From<KilnError> for ExecError {
    fn from(value: KilnError) -> Self {
        Self {
            kind: Box::new(ErrorKind::Core(value)),
            backtrace: Backtrace::capture(),
        }
    }
}

#[derive(Debug)]
pub(crate) enum ErrorKind {
    Io {
        source: io::Error,
        context: String,
    },
    InterpreterNotFound {
        name: String,
    },
    TargetFailed {
        target: String,
        exit_code: i32,
    },
    TargetTimeout {
        target: String,
        elapsed_ms: u64,
    },
    SandboxSetup {
        context: String,
        path: Option<PathBuf>,
    },
    FetchFailed {
        url: String,
        context: String,
    },
    Cache(CacheError),
    Core(KilnError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_query_and_message() {
        let err = ExecError::io(io::Error::other("disk full"), "writing workspace");
        assert!(err.is_io());
        assert!(format!("{err}").contains("disk full"));
        assert!(format!("{err}").contains("writing workspace"));
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn interpreter_not_found_query() {
        let err = ExecError::interpreter_not_found("zsh");
        assert!(err.is_interpreter_not_found());
        assert!(!err.is_target_failed());
        assert!(format!("{err}").contains("zsh"));
    }

    #[test]
    fn target_failed_query() {
        let err = ExecError::target_failed("build", 42);
        assert!(err.is_target_failed());
        assert!(format!("{err}").contains("build"));
        assert!(format!("{err}").contains("42"));
    }

    #[test]
    fn target_timeout_query() {
        let err = ExecError::target_timeout("build", 1500);
        assert!(err.is_target_timeout());
        assert!(format!("{err}").contains("1500"));
    }

    #[test]
    fn sandbox_setup_query_with_path() {
        let err = ExecError::sandbox_setup("mount failed", Some(PathBuf::from("/tmp/x")));
        assert!(err.is_sandbox_setup());
        assert!(format!("{err}").contains("/tmp/x"));
    }

    #[test]
    fn fetch_failed_query() {
        let err = ExecError::fetch_failed("https://example/x", "404");
        assert!(err.is_fetch_failed());
        assert!(format!("{err}").contains("https://example/x"));
        assert!(format!("{err}").contains("404"));
    }

    #[test]
    fn core_error_round_trip_via_from() {
        // CacheError's constructors are pub(crate) so the From<CacheError>
        // path is exercised by integration tests in executor.rs (M4 part 2).
        // KilnError can be triggered via its public API, so we cover the
        // From<KilnError> path here.
        let err: KilnError = kiln_core::Pipeline::from_json_str("nope").unwrap_err();
        let exec_err: ExecError = err.into();
        assert!(exec_err.is_core_error());
        assert!(std::error::Error::source(&exec_err).is_some());
    }

    #[test]
    fn error_is_send() {
        const fn assert_send<T: Send>() {}
        assert_send::<ExecError>();
    }
}
