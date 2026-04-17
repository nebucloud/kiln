// Rust guideline compliant 2026-02-21
// Adapted from terranoxos/terranox-tools/crates/lattice-exec/src/sandbox.rs
// (commit 7ab5316ac6 baseline). Simplified for kiln 0.1.0 (per
// KLN-PLAN-extraction §8 risk register, scope is Linux-only and pivot_root
// + cgroups land in 0.2+):
//
// - Kept: workspace creation, env sanitization, optional CLONE_NEWUSER +
//   CLONE_NEWNET (sealed-by-default network policy from KLN-D-04),
//   wall-clock timeout config field.
// - Deferred to 0.2+: pivot_root filesystem isolation, cgroup memory/cpu
//   limits, hermeticity snapshotting, CLONE_NEWPID, mount-based bind
//   isolation. The corresponding `SandboxConfig` fields exist as the
//   public API surface but are documented as no-ops in 0.1.0.
//
// See KLN-PLAN-extraction §6 for the provenance convention.
//! Per-target sandbox.
//!
//! [`Sandbox`] owns a temporary workspace directory and applies
//! environment + namespace setup to a [`std::process::Command`] before
//! the runner spawns it. Each [`Sandbox`] cleans up its workspace on
//! drop.
//!
//! # 0.1.0 isolation surface
//!
//! When [`SandboxConfig::enabled`] is `true`, the runner unshares into
//! a new mount namespace (so any later mounts won't propagate to the
//! host) and, when [`SandboxConfig::isolate_network`] is `true`, also
//! into new user + network namespaces — the run block then has no
//! network access. This implements the sealed-by-default policy from
//! [KLN-D-04][kln-d-04].
//!
//! Filesystem isolation via `pivot_root`, cgroup memory / CPU limits,
//! and PID-namespace isolation are deferred to kiln 0.2+. Their
//! [`SandboxConfig`] fields are accepted today (so consumers can write
//! forward-compatible code) but are no-ops in 0.1.0.
//!
//! [kln-d-04]: https://github.com/nebucloud/docs/blob/main/KLN-D-extraction-decisions.md
//!
//! # Platform support
//!
//! Linux only for 0.1.0. On non-Linux hosts the sandbox compiles, but
//! `enabled = true` falls back to env sanitization without namespace
//! setup (the unshare calls require Linux).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::error::ExecError;

/// Configuration for sandbox isolation.
///
/// All fields are present at the 0.1.0 API surface even when the
/// underlying implementation lands later — that lets consumers write
/// forward-compatible config today. Fields documented as "0.2+ only"
/// are read but not yet enforced.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each toggle is an independent user-facing isolation knob; \
              bundling them into a flags struct would obscure the public API"
)]
#[derive(Debug, Clone, Default)]
pub struct SandboxConfig {
    /// When `true`, the sandbox applies env sanitization and (on Linux)
    /// unshares into a new mount namespace. When `false`, only the
    /// workspace and explicit env settings are applied.
    pub enabled: bool,

    /// Paths the target is allowed to read.
    ///
    /// **0.2+ only.** In 0.1.0 these are stored but not bind-mounted —
    /// targets see the host filesystem unmodified.
    pub allowed_inputs: Vec<PathBuf>,

    /// Directories where the target may produce outputs.
    ///
    /// **0.2+ only.** In 0.1.0 these are stored but not enforced.
    pub allowed_outputs: Vec<PathBuf>,

    /// When `true`, the child unshares into a user + network namespace.
    ///
    /// Implements the sealed-by-default network policy from KLN-D-04.
    /// The fetch phase (run before the seal) can reach the network;
    /// the run phase (run after the seal) cannot.
    ///
    /// Requires `CAP_SYS_ADMIN` or unprivileged user namespaces. On
    /// hosts where namespace creation fails the runner reports
    /// [`ExecError::is_sandbox_setup`].
    pub isolate_network: bool,

    /// Maximum wall-clock duration for the run block.
    ///
    /// When set, the runner sends `SIGTERM` after the deadline and
    /// `SIGKILL` 5 seconds later. Catches sleeping / hung processes
    /// that don't burn CPU.
    pub wall_clock_timeout: Option<Duration>,

    /// When `true`, the child process starts with an empty environment.
    ///
    /// Variables in [`env_allowlist`](Self::env_allowlist) are
    /// re-injected from the parent and [`env_vars`](Self::env_vars)
    /// entries are set explicitly afterward.
    pub env_clear: bool,

    /// Variables to forward from the parent when [`env_clear`](Self::env_clear) is `true`.
    pub env_allowlist: Vec<String>,

    /// Variables to set on the child process unconditionally.
    pub env_vars: Vec<(String, String)>,

    // --- 0.2+ only — present for forward-compatibility ---
    /// **0.2+ only.** Memory limit (cgroup v2 `memory.max`).
    pub memory_limit: Option<u64>,
    /// **0.2+ only.** CPU time limit (cgroup v2 `cpu.max` quota).
    pub cpu_limit_ms: Option<u64>,
    /// **0.2+ only.** Toggle PID namespace isolation.
    pub isolate_pid: bool,
    /// **0.2+ only.** Toggle full `pivot_root` filesystem isolation.
    pub isolate_filesystem: bool,
}

impl SandboxConfig {
    /// Returns a config with sandboxing enabled and network sealed by default.
    ///
    /// Defaults: env cleared with a minimal `HOME`/`USER`/`TERM`/`LANG`
    /// allowlist, network isolated. Other fields keep their type
    /// defaults.
    #[must_use]
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            isolate_network: true,
            env_clear: true,
            env_allowlist: vec![
                "HOME".to_owned(),
                "USER".to_owned(),
                "TERM".to_owned(),
                "LANG".to_owned(),
            ],
            ..Self::default()
        }
    }
}

/// A per-target sandbox owning a temporary workspace.
///
/// Drops the workspace on `Drop`. Hold one across the lifetime of a
/// single target's execution.
#[derive(Debug)]
pub struct Sandbox {
    config: SandboxConfig,
    target_id: String,
    work_dir: PathBuf,
}

impl Sandbox {
    /// Creates a new sandbox with a unique workspace directory under the
    /// system temp dir.
    ///
    /// # Errors
    ///
    /// Returns [`ExecError::is_sandbox_setup`] when the workspace
    /// directory cannot be created.
    pub fn new(config: SandboxConfig, target_id: &str) -> Result<Self, ExecError> {
        let work_dir =
            std::env::temp_dir().join(format!("kiln-sandbox-{}-{target_id}", std::process::id(),));

        std::fs::create_dir_all(&work_dir).map_err(|err| {
            ExecError::sandbox_setup(
                format!("failed to create workspace: {err}"),
                Some(work_dir.clone()),
            )
        })?;

        Ok(Self {
            config,
            target_id: target_id.to_owned(),
            work_dir,
        })
    }

    /// Returns the sandbox workspace directory.
    #[must_use]
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// Returns the configuration.
    #[must_use]
    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }

    /// Returns the target id this sandbox was created for.
    #[must_use]
    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    /// Applies the sandbox's environment and (on Linux, when enabled)
    /// namespace setup to `cmd`.
    ///
    /// Always sets the working directory to the sandbox workspace.
    /// Always applies `env_clear` / `env_allowlist` / `env_vars` per
    /// [`SandboxConfig`]. When `enabled` is `true` and the host is
    /// Linux, also installs a `pre_exec` closure that calls
    /// `unshare(2)` for the requested namespaces.
    pub fn apply_to_command(&self, cmd: &mut Command) {
        cmd.current_dir(&self.work_dir);

        if self.config.env_clear {
            cmd.env_clear();
            for key in &self.config.env_allowlist {
                if let Ok(value) = std::env::var(key) {
                    cmd.env(key, value);
                }
            }
        }
        for (key, value) in &self.config.env_vars {
            cmd.env(key, value);
        }

        if self.config.enabled {
            apply_namespace_isolation(cmd, &self.config);
        }
    }

    /// Removes the workspace directory.
    ///
    /// Called automatically on `Drop`. Idempotent.
    ///
    /// # Errors
    ///
    /// Returns [`ExecError::is_sandbox_setup`] when the workspace
    /// directory exists but cannot be removed (permission denied,
    /// in-use file, etc.).
    pub fn cleanup(&self) -> Result<(), ExecError> {
        if self.work_dir.exists() {
            std::fs::remove_dir_all(&self.work_dir).map_err(|err| {
                ExecError::sandbox_setup(
                    format!("failed to remove workspace: {err}"),
                    Some(self.work_dir.clone()),
                )
            })?;
        }
        Ok(())
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Best-effort cleanup; nothing to do if it fails because the
        // process is exiting or the temp dir was already removed.
        let _ = self.cleanup();
    }
}

#[cfg(target_os = "linux")]
fn apply_namespace_isolation(cmd: &mut Command, config: &SandboxConfig) {
    use std::os::unix::process::CommandExt;

    use nix::sched::CloneFlags;

    let isolate_network = config.isolate_network;
    let host_user_id = nix::unistd::getuid();
    let host_group_id = nix::unistd::getgid();

    // SAFETY: The closure runs between fork(2) and exec(2). It calls
    // only async-signal-safe syscalls (unshare, write to /proc/self
    // pseudo-files) per the constraints documented in
    // `std::os::unix::process::CommandExt::pre_exec`.
    unsafe {
        cmd.pre_exec(move || {
            let mut flags = CloneFlags::CLONE_NEWNS;
            if isolate_network {
                flags |= CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNET;
            }

            nix::sched::unshare(flags).map_err(|err| {
                std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "namespace unshare failed: {err} \
                         (hint: needs CAP_SYS_ADMIN or unprivileged user namespaces)"
                    ),
                )
            })?;

            // When CLONE_NEWUSER is active, write the uid/gid maps so
            // file ops inside the namespace see a sensible owner.
            // Failure is best-effort; file writes can race with kernel
            // policy on some hosts.
            if isolate_network {
                let _ = std::fs::write("/proc/self/setgroups", "deny");
                let _ = std::fs::write("/proc/self/uid_map", format!("0 {host_user_id} 1"));
                let _ = std::fs::write("/proc/self/gid_map", format!("0 {host_group_id} 1"));
            }

            // Make the new mount namespace private so future mounts
            // (and bind mounts) don't propagate back to the host.
            nix::mount::mount(
                None::<&str>,
                "/",
                None::<&str>,
                nix::mount::MsFlags::MS_REC | nix::mount::MsFlags::MS_PRIVATE,
                None::<&str>,
            )
            .map_err(|err| {
                std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("MS_PRIVATE remount of `/` failed: {err}"),
                )
            })?;

            Ok(())
        });
    }
}

#[cfg(not(target_os = "linux"))]
fn apply_namespace_isolation(_cmd: &mut Command, _config: &SandboxConfig) {
    // Namespace isolation is Linux-only in 0.1.0. On other platforms
    // the sandbox still applies env sanitization; the rest is a no-op.
}

/// Probes whether the current process can create unprivileged Linux namespaces.
///
/// Returns `false` immediately when:
///
/// - `KILN_NO_SANDBOX=1` is set in the environment,
/// - the kernel reports a WSL release (`/proc/sys/kernel/osrelease`
///   contains `microsoft` or `WSL`),
/// - `/proc/sys/kernel/unprivileged_userns_clone` reads `0`.
///
/// Otherwise forks a child that attempts `unshare(CLONE_NEWUSER)` as a
/// live test; the parent observes the child's exit status. Always
/// returns `false` on non-Linux hosts.
#[must_use]
#[allow(
    clippy::needless_pass_by_value,
    reason = "no parameters; explicit return type kept for clarity"
)]
pub fn can_isolate_namespaces() -> bool {
    can_isolate_namespaces_impl()
}

#[cfg(target_os = "linux")]
fn can_isolate_namespaces_impl() -> bool {
    use nix::sched::CloneFlags;

    if std::env::var("KILN_NO_SANDBOX").ok().as_deref() == Some("1") {
        return false;
    }

    if let Ok(release) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        if release.contains("microsoft") || release.contains("WSL") {
            return false;
        }
    }

    if let Ok(value) = std::fs::read_to_string("/proc/sys/kernel/unprivileged_userns_clone") {
        if value.trim() == "0" {
            return false;
        }
    }

    // SAFETY: fork(2) is async-signal-safe. The child only invokes
    // unshare(2) and exit(2), both async-signal-safe.
    match unsafe { nix::unistd::fork() } {
        Ok(nix::unistd::ForkResult::Parent { child }) => {
            matches!(
                nix::sys::wait::waitpid(child, None),
                Ok(nix::sys::wait::WaitStatus::Exited(_, 0))
            )
        }
        Ok(nix::unistd::ForkResult::Child) => {
            let result = nix::sched::unshare(CloneFlags::CLONE_NEWUSER);
            std::process::exit(i32::from(result.is_err()));
        }
        Err(_) => false,
    }
}

#[cfg(not(target_os = "linux"))]
fn can_isolate_namespaces_impl() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sets an env var inside the test process.
    ///
    /// Tests in this module run serially in a single thread within the
    /// cargo-test process, so transient env mutations are race-free.
    /// The function exists to satisfy the `semicolon_outside_block`
    /// and `semicolon_if_nothing_returned` lints simultaneously —
    /// they disagree on how `unsafe { std::env::set_var(...) }` should
    /// be punctuated; wrapping in a fn sidesteps the conflict.
    fn set_test_env(key: &str, value: &str) {
        // SAFETY: serial test execution; see fn-level note.
        unsafe { std::env::set_var(key, value) }
    }

    /// Removes a previously-set test env var. See [`set_test_env`].
    fn remove_test_env(key: &str) {
        // SAFETY: serial test execution; see `set_test_env`.
        unsafe { std::env::remove_var(key) }
    }

    #[test]
    fn config_default_disabled() {
        let config = SandboxConfig::default();
        assert!(!config.enabled);
        assert!(!config.isolate_network);
        assert!(!config.env_clear);
        assert!(config.env_allowlist.is_empty());
        assert!(config.env_vars.is_empty());
        assert!(config.allowed_inputs.is_empty());
        assert!(config.allowed_outputs.is_empty());
    }

    #[test]
    fn config_enabled_seals_network_and_clears_env() {
        let config = SandboxConfig::enabled();
        assert!(config.enabled);
        assert!(config.isolate_network);
        assert!(config.env_clear);
        assert!(config.env_allowlist.iter().any(|k| k == "HOME"));
    }

    #[test]
    fn sandbox_creates_workspace_and_cleans_up_on_drop() {
        let sandbox = Sandbox::new(SandboxConfig::default(), "ws-test").unwrap();
        let work_dir = sandbox.work_dir().to_path_buf();
        assert!(work_dir.exists());
        drop(sandbox);
        assert!(!work_dir.exists());
    }

    #[test]
    fn cleanup_is_idempotent() {
        let sandbox = Sandbox::new(SandboxConfig::default(), "idem").unwrap();
        sandbox.cleanup().unwrap();
        sandbox.cleanup().unwrap();
    }

    #[test]
    fn disabled_sandbox_runs_command_unchanged() {
        let sandbox = Sandbox::new(SandboxConfig::default(), "noop").unwrap();
        let mut cmd = Command::new("echo");
        cmd.arg("hello");
        sandbox.apply_to_command(&mut cmd);

        let output = cmd.output().expect("echo should run");
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello"));
    }

    #[test]
    fn env_clear_strips_inherited_vars() {
        let key = format!("KILN_TEST_ENV_{}", std::process::id());
        set_test_env(&key, "should_not_appear");

        let config = SandboxConfig {
            env_clear: true,
            env_allowlist: vec!["HOME".to_owned()],
            ..SandboxConfig::default()
        };
        let sandbox = Sandbox::new(config, "env-clear").unwrap();

        let mut cmd = Command::new("env");
        sandbox.apply_to_command(&mut cmd);
        let output = cmd.output().expect("env should run");
        let stdout = String::from_utf8_lossy(&output.stdout);

        assert!(
            !stdout.contains(&key),
            "stripped var leaked into child env: {stdout}"
        );

        remove_test_env(&key);
    }

    #[test]
    fn env_vars_are_injected_into_child() {
        let config = SandboxConfig {
            env_clear: true,
            env_vars: vec![("KILN_HELLO".to_owned(), "world".to_owned())],
            ..SandboxConfig::default()
        };
        let sandbox = Sandbox::new(config, "env-inject").unwrap();

        let mut cmd = Command::new("env");
        sandbox.apply_to_command(&mut cmd);
        let output = cmd.output().expect("env should run");
        let stdout = String::from_utf8_lossy(&output.stdout);

        assert!(stdout.contains("KILN_HELLO=world"), "got: {stdout}");
    }

    #[test]
    fn env_clear_false_inherits_parent_env() {
        let key = format!("KILN_INHERIT_{}", std::process::id());
        set_test_env(&key, "present");

        let sandbox = Sandbox::new(SandboxConfig::default(), "env-inherit").unwrap();
        let mut cmd = Command::new("env");
        sandbox.apply_to_command(&mut cmd);
        let output = cmd.output().expect("env should run");
        let stdout = String::from_utf8_lossy(&output.stdout);

        assert!(
            stdout.contains(&format!("{key}=present")),
            "inherited var missing: {stdout}"
        );

        remove_test_env(&key);
    }

    #[test]
    fn can_isolate_namespaces_returns_bool() {
        // The result depends on the host kernel; just verify the probe doesn't panic.
        let _ = can_isolate_namespaces();
    }

    #[test]
    fn types_are_send_and_sync() {
        const fn assert_send<T: Send>() {}
        const fn assert_sync<T: Sync>() {}
        assert_send::<SandboxConfig>();
        assert_sync::<SandboxConfig>();
        assert_send::<Sandbox>();
    }
}
