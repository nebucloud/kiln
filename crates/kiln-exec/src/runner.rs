// Rust guideline compliant 2026-02-21
// Adapted from terranoxos/terranox-tools/crates/lattice-exec/src/runner.rs
// (commit 7ab5316ac6 baseline). Simplified for kiln 0.1.0:
//
// - Kept: single-target execution with sandbox + wall-clock timeout +
//   cache lookup/store, cleanup-block handling on failure.
// - Deferred to M4 part 2: ExecEvent callback infrastructure,
//   plan-level execute_plan / execute_plan_parallel (those land in
//   `executor.rs`).
//
// See KLN-PLAN-extraction §6 for the provenance convention.
//! Single-target execution.
//!
//! [`run_target`] runs one [`Target`] inside a [`Sandbox`] and
//! captures stdout / stderr / exit code as a [`RunResult`].
//! [`run_target_cached`] wraps it with a [`ShardedCache`] lookup so
//! a target whose inputs / interpreter / script have not changed
//! since the last green run skips execution entirely.

use std::process::Command;
use std::time::{Duration, Instant};

use kiln_cache::{LocalDiskStore, ShardedCache};
use kiln_core::{ShellBlock, Target, TargetId};

use crate::error::ExecError;
use crate::sandbox::{Sandbox, SandboxConfig};

/// The result of running a single [`Target`] to completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    /// The target that was executed.
    pub target_id: TargetId,
    /// The process exit code (`0` on success).
    pub exit_code: i32,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
    /// `true` when the result was served from cache rather than executed.
    pub cache_hit: bool,
}

/// Runs a target inside its sandbox.
///
/// Resolves the interpreter, applies the [`SandboxConfig`] to a fresh
/// [`Command`], and waits for completion. When `config.wall_clock_timeout`
/// is set, a watchdog thread sends `SIGTERM` after the deadline and
/// `SIGKILL` 5 seconds later. On failure (non-zero exit), the target's
/// `cleanup` block runs (if present) before the error propagates.
///
/// # Errors
///
/// - [`ExecError::is_io`] when the interpreter cannot be spawned.
/// - [`ExecError::is_target_failed`] when the run block exits non-zero.
/// - [`ExecError::is_target_timeout`] when the wall-clock deadline fires
///   before the run block finishes.
/// - [`ExecError::is_sandbox_setup`] when the sandbox cannot be created.
pub fn run_target(
    target_id: &TargetId,
    target: &Target,
    config: SandboxConfig,
) -> Result<RunResult, ExecError> {
    let timeout = config.wall_clock_timeout;
    let sandbox = Sandbox::new(config, target_id.as_ref())?;

    let mut cmd = Command::new(&target.run.interpreter);
    cmd.arg("-c").arg(&target.run.code);
    sandbox.apply_to_command(&mut cmd);

    let output = spawn_with_timeout(&mut cmd, timeout, target_id.as_ref())?;
    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if exit_code != 0 {
        if let Some(cleanup) = &target.cleanup {
            let _ = run_block(cleanup);
        }
        return Err(ExecError::target_failed(target_id.0.clone(), exit_code));
    }

    Ok(RunResult {
        target_id: target_id.clone(),
        exit_code,
        stdout,
        stderr,
        cache_hit: false,
    })
}

/// Runs a target with cache lookup / store on either side.
///
/// Behavior:
///
/// 1. Compute the cache key from the target.
/// 2. Lookup; on hit (and `exit_code == 0`), return the cached
///    [`RunResult`] with `cache_hit = true` — no execution.
/// 3. On miss (or cached failure), invoke [`run_target`].
/// 4. On a successful run, store the result back into the cache.
///
/// Cached failures (`exit_code != 0`) are intentionally re-executed —
/// failures are often transient and the cost of a re-run is bounded
/// by the existing `wall_clock_timeout`.
///
/// # Errors
///
/// Returns whatever [`run_target`] returns, plus
/// [`ExecError::is_cache_error`] surfaced from the underlying
/// [`ShardedCache`].
pub fn run_target_cached(
    target_id: &TargetId,
    target: &Target,
    config: SandboxConfig,
    cache: &ShardedCache<LocalDiskStore>,
) -> Result<RunResult, ExecError> {
    let key = cache.compute_key(target);

    if let Some(hit) = cache.lookup(&key)? {
        if hit.entry.exit_code == 0 {
            return Ok(RunResult {
                target_id: target_id.clone(),
                exit_code: hit.entry.exit_code,
                stdout: hit.stdout,
                stderr: hit.stderr,
                cache_hit: true,
            });
        }
    }

    let result = run_target(target_id, target, config)?;

    let _ = cache.store(
        &key,
        target_id.as_ref(),
        result.exit_code,
        &result.stdout,
        &result.stderr,
    );

    Ok(result)
}

/// Spawns `cmd`, optionally enforcing a wall-clock deadline.
///
/// When `timeout` is `None`, blocks on `wait_with_output`. When
/// `Some(d)`, spawns a watchdog thread that signals the child with
/// `SIGTERM` after `d` and `SIGKILL` 5 seconds later. The watchdog
/// is detached — if the child exits in time the watchdog harmlessly
/// signals a dead PID when it later wakes.
fn spawn_with_timeout(
    cmd: &mut Command,
    timeout: Option<Duration>,
    target: &str,
) -> Result<std::process::Output, ExecError> {
    let child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| ExecError::io(err, format!("failed to spawn `{target}`")))?;

    let Some(deadline) = timeout else {
        return child
            .wait_with_output()
            .map_err(|err| ExecError::io(err, format!("wait for `{target}` failed")));
    };

    let started = Instant::now();
    let pid = child.id();

    let _watchdog = std::thread::spawn(move || {
        std::thread::sleep(deadline);
        send_signal(pid, libc_signal_term());
        std::thread::sleep(Duration::from_secs(5));
        send_signal(pid, libc_signal_kill());
    });

    let output = child
        .wait_with_output()
        .map_err(|err| ExecError::io(err, format!("wait for `{target}` failed")))?;

    let elapsed = started.elapsed();
    if elapsed >= deadline && !output.status.success() {
        return Err(ExecError::target_timeout(
            target,
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        ));
    }

    Ok(output)
}

/// Runs a [`ShellBlock`] without a sandbox or output capture.
///
/// Used internally for cleanup blocks where the caller doesn't care
/// about output — only that the cleanup ran.
fn run_block(block: &ShellBlock) -> Result<std::process::ExitStatus, ExecError> {
    Command::new(&block.interpreter)
        .arg("-c")
        .arg(&block.code)
        .status()
        .map_err(|err| {
            ExecError::io(
                err,
                format!(
                    "failed to run cleanup with interpreter `{}`",
                    block.interpreter
                ),
            )
        })
}

#[cfg(target_os = "linux")]
fn send_signal(pid: u32, signal: i32) {
    // SAFETY: kill(2) is async-signal-safe. `pid` came from
    // Child::id(); sending to a no-longer-existing PID is harmless
    // (returns ESRCH and we ignore it).
    unsafe {
        libc::kill(i32::try_from(pid).unwrap_or(libc::pid_t::MAX), signal);
    }
}

#[cfg(not(target_os = "linux"))]
fn send_signal(_pid: u32, _signal: i32) {
    // Watchdog signaling is Linux-only in 0.1.0 (libc dep is Linux-gated).
    // On other platforms a hung child must be killed by the host.
}

#[cfg(target_os = "linux")]
fn libc_signal_term() -> i32 {
    libc::SIGTERM
}

#[cfg(not(target_os = "linux"))]
fn libc_signal_term() -> i32 {
    15
}

#[cfg(target_os = "linux")]
fn libc_signal_kill() -> i32 {
    libc::SIGKILL
}

#[cfg(not(target_os = "linux"))]
fn libc_signal_kill() -> i32 {
    9
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn target(code: &str) -> Target {
        Target::new(ShellBlock::new("bash", code))
    }

    fn temp_cache() -> (ShardedCache<LocalDiskStore>, std::path::PathBuf) {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("kiln-runner-cache-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (ShardedCache::new(LocalDiskStore::new(&dir)), dir)
    }

    #[test]
    fn run_target_captures_stdout() {
        let id = TargetId::new("greet");
        let result = run_target(&id, &target("echo hello"), SandboxConfig::default()).unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(
            result.stdout.contains("hello"),
            "stdout: {:?}",
            result.stdout
        );
        assert_eq!(result.target_id, id);
        assert!(!result.cache_hit);
    }

    #[test]
    fn run_target_captures_stderr() {
        let id = TargetId::new("warn");
        let result = run_target(&id, &target("echo oops >&2"), SandboxConfig::default()).unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(
            result.stderr.contains("oops"),
            "stderr: {:?}",
            result.stderr
        );
    }

    #[test]
    fn nonzero_exit_surfaces_target_failed() {
        let id = TargetId::new("fail");
        let err = run_target(&id, &target("exit 1"), SandboxConfig::default()).unwrap_err();
        assert!(err.is_target_failed());
        assert!(format!("{err}").contains("fail"));
    }

    #[test]
    fn cleanup_block_runs_on_failure() {
        let marker =
            std::env::temp_dir().join(format!("kiln-runner-cleanup-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);

        let mut t = target("exit 1");
        t.cleanup = Some(ShellBlock::new(
            "bash",
            format!("touch {}", marker.display()),
        ));

        let id = TargetId::new("fail-with-cleanup");
        let _ = run_target(&id, &t, SandboxConfig::default());

        let exists = marker.exists();
        let _ = std::fs::remove_file(&marker);
        assert!(exists, "cleanup block should have created the marker file");
    }

    #[test]
    fn timeout_kills_hung_target() {
        let id = TargetId::new("hung");
        let config = SandboxConfig {
            wall_clock_timeout: Some(Duration::from_millis(200)),
            ..SandboxConfig::default()
        };
        let err = run_target(&id, &target("sleep 60"), config).unwrap_err();
        assert!(err.is_target_timeout(), "expected timeout, got: {err}");
    }

    #[test]
    fn no_timeout_completes_normally() {
        let id = TargetId::new("fast");
        let config = SandboxConfig {
            wall_clock_timeout: None,
            ..SandboxConfig::default()
        };
        let result = run_target(&id, &target("echo done"), config).unwrap();
        assert!(result.stdout.contains("done"));
    }

    #[test]
    fn cached_run_misses_then_hits() {
        let (cache, dir) = temp_cache();
        let id = TargetId::new("cached");
        let t = target("echo cached_value");

        let r1 = run_target_cached(&id, &t, SandboxConfig::default(), &cache).unwrap();
        assert!(!r1.cache_hit);
        assert!(r1.stdout.contains("cached_value"));

        let r2 = run_target_cached(&id, &t, SandboxConfig::default(), &cache).unwrap();
        assert!(r2.cache_hit, "second run should hit the cache");
        assert_eq!(r1.stdout, r2.stdout);
        assert_eq!(r1.exit_code, r2.exit_code);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cached_failure_re_executes() {
        let (cache, dir) = temp_cache();
        let id = TargetId::new("flaky");
        let t = target("exit 1");

        // First run: actual failure, propagates as ExecError.
        let err = run_target_cached(&id, &t, SandboxConfig::default(), &cache).unwrap_err();
        assert!(err.is_target_failed());

        // Second run: even though the failure was stored, we re-execute
        // (failures may be transient).
        let err = run_target_cached(&id, &t, SandboxConfig::default(), &cache).unwrap_err();
        assert!(err.is_target_failed());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_interpreter_surfaces_io_error() {
        let id = TargetId::new("nope");
        let mut t = target("echo unused");
        t.run.interpreter = "definitely_not_a_real_interpreter_xyz".to_owned();
        let err = run_target(&id, &t, SandboxConfig::default()).unwrap_err();
        assert!(err.is_io(), "expected IO error, got: {err}");
    }

    #[test]
    fn types_are_send() {
        const fn assert_send<T: Send>() {}
        assert_send::<RunResult>();
    }
}
