// Rust guideline compliant 2026-02-21
// Ported from terranoxos/terranox-tools/crates/lattice-exec/src/shell_backend.rs
// (commit 7ab5316ac6 baseline). Renamed `resolve_interpreter` -> `resolve` for
// brevity; error type adapted to ExecError. See KLN-PLAN-extraction §6.
//! Polyglot interpreter resolution.
//!
//! [`ShellBackend::resolve`] looks up an interpreter name (e.g. `bash`,
//! `python3`, `pwsh`) on the host's `PATH` and returns the absolute
//! path to the binary. The runner uses this to validate that a
//! target's declared interpreter exists before forking, and to invoke
//! it via its full path (so the target sees the same binary
//! regardless of any later `PATH` munging).

use std::path::PathBuf;
use std::process::Command;

use crate::error::ExecError;

/// Looks up shell interpreters by name on the host system.
///
/// 0.1.0 only ships a single resolver implementation (`which`). The
/// type stays as a unit struct so it can grow features (cached
/// resolutions, sandbox-relative resolution) in 0.2+ without API
/// breakage.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShellBackend;

impl ShellBackend {
    /// Constructs a `ShellBackend`.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Resolves an interpreter name to its absolute path.
    ///
    /// Uses the system `which` binary to perform the lookup so kiln
    /// inherits whatever shell-aware path resolution the host is
    /// configured for (login shells, `~/.local/bin` overrides, etc.).
    ///
    /// # Errors
    ///
    /// Returns [`ExecError::is_interpreter_not_found`] when:
    ///
    /// - the `which` binary itself cannot be invoked,
    /// - `which` exits non-zero,
    /// - `which` returns an empty path (unusual but possible on some
    ///   shells when the lookup expands to nothing).
    pub fn resolve(name: &str) -> Result<PathBuf, ExecError> {
        let output = Command::new("which")
            .arg(name)
            .output()
            .map_err(|err| ExecError::interpreter_not_found(format!("{name}: {err}")))?;

        if !output.status.success() {
            return Err(ExecError::interpreter_not_found(name.to_owned()));
        }

        let path_str = String::from_utf8_lossy(&output.stdout);
        let path_str = path_str.trim();
        if path_str.is_empty() {
            return Err(ExecError::interpreter_not_found(name.to_owned()));
        }

        Ok(PathBuf::from(path_str))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_resolves_on_typical_unix_host() {
        let path = ShellBackend::resolve("bash").expect("bash should be on PATH for tests");
        assert!(
            path.to_string_lossy().contains("bash"),
            "resolved path should contain 'bash': {path:?}"
        );
        assert!(path.exists(), "resolved path should exist: {path:?}");
    }

    #[test]
    fn missing_interpreter_returns_typed_error() {
        let err = ShellBackend::resolve("definitely_not_a_real_interpreter_xyz").expect_err("miss");
        assert!(err.is_interpreter_not_found());
        assert!(format!("{err}").contains("definitely_not_a_real_interpreter_xyz"));
    }
}
