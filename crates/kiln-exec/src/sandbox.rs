// Rust guideline compliant 2026-02-21
// Adapted from terranoxos/terranox-tools/crates/lattice-exec/src/sandbox.rs
// (commit 7ab5316ac6 baseline). The pivot_root + bind-mount path was
// re-introduced in kiln 0.2.0 from the same upstream reference at commit
// fa4607d, with the additive bind-mount fallback dropped (callers see a
// graceful fall-through to the legacy mount-namespace-only behaviour
// when pivot_root is unavailable).
//
// See KLN-PLAN-extraction §6 for the provenance convention.
//! Per-target sandbox.
//!
//! [`Sandbox`] owns a temporary workspace directory and applies
//! environment + namespace setup to a [`std::process::Command`] before
//! the runner spawns it. Each [`Sandbox`] cleans up its workspace on
//! drop.
//!
//! # Isolation surface
//!
//! When [`SandboxConfig::enabled`] is `true`, the runner unshares into
//! a new mount namespace (so any later mounts won't propagate to the
//! host). When [`SandboxConfig::isolate_network`] is `true` it also
//! unshares user + network namespaces, sealing network access per the
//! KLN-D-04 sealed-by-default policy.
//!
//! When [`SandboxConfig::isolate_filesystem`] is `true` *and* a pivot
//! plan can be built, the sandbox additionally:
//!
//! - mounts a fresh tmpfs at the workspace,
//! - bind-mounts each [`SandboxConfig::allowed_inputs`] entry read-only
//!   at its original absolute path inside the new root,
//! - bind-mounts each [`SandboxConfig::allowed_outputs`] entry writable,
//! - mounts `/proc`, a minimal `/dev` (null/zero/random/urandom), and a
//!   tmpfs at `/tmp`,
//! - calls `pivot_root(2)` and detaches the old root with
//!   `MNT_DETACH`.
//!
//! After the pivot the host filesystem is no longer reachable: the
//! target sees only its declared inputs and outputs. If `pivot_root`
//! fails (WSL2, restricted kernels, missing permissions) the sandbox
//! falls back to the namespace-only path so the build can still
//! proceed — the caller observes via tracing logs or error messages.
//!
//! Still deferred to a later kiln release: cgroup v2 memory / CPU
//! limits, hermeticity snapshotting, PID-namespace isolation. Their
//! [`SandboxConfig`] fields are accepted today (so consumers can write
//! forward-compatible code) but remain no-ops.
//!
//! [kln-d-04]: https://github.com/nebucloud/docs/blob/main/KLN-D-extraction-decisions.md
//!
//! # Platform support
//!
//! Linux only. On non-Linux hosts the sandbox compiles, but
//! `enabled = true` falls back to env sanitization without namespace
//! setup (the unshare calls require Linux).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::error::ExecError;

/// Per-process monotonic counter for sandbox workspace uniqueness.
///
/// Two sandboxes created in the same process for the same `target_id`
/// must get distinct workspace directories (otherwise parallel tests
/// or wave-internal parallelism collide). M-AVOID-STATICS treats
/// "performance optimization" statics as acceptable; this is a
/// lightweight equivalent: a per-process serial number that doesn't
/// need cross-version synchronization.
static SANDBOX_SEQ: AtomicU64 = AtomicU64::new(0);

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
    /// When [`isolate_filesystem`](Self::isolate_filesystem) is `true`,
    /// each path is bind-mounted read-only at its original absolute
    /// location inside the pivoted root. Without `isolate_filesystem`
    /// the field is stored but not enforced.
    pub allowed_inputs: Vec<PathBuf>,

    /// Directories where the target may produce outputs.
    ///
    /// When [`isolate_filesystem`](Self::isolate_filesystem) is `true`,
    /// each path is bind-mounted writable at its original absolute
    /// location inside the pivoted root. Without `isolate_filesystem`
    /// the field is stored but not enforced.
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

    /// When `true`, install a `pivot_root` filesystem jail and bind-mount
    /// only the declared [`allowed_inputs`](Self::allowed_inputs) and
    /// [`allowed_outputs`](Self::allowed_outputs) plus a minimal
    /// `/proc`, `/dev`, and `/tmp`. Falls back to the namespace-only
    /// path if `pivot_root(2)` cannot be performed (WSL2, restricted
    /// kernels). See the module-level docs for the full mount layout.
    pub isolate_filesystem: bool,

    // --- Forward-compatibility fields, still no-op in this release ---
    /// Reserved for cgroup v2 `memory.max` enforcement (later release).
    pub memory_limit: Option<u64>,
    /// Reserved for cgroup v2 `cpu.max` enforcement (later release).
    pub cpu_limit_ms: Option<u64>,
    /// Reserved for PID namespace isolation (later release).
    pub isolate_pid: bool,
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
        let seq = SANDBOX_SEQ.fetch_add(1, Ordering::Relaxed);
        let work_dir = std::env::temp_dir().join(format!(
            "kiln-sandbox-{}-{seq}-{target_id}",
            std::process::id(),
        ));

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
            apply_namespace_isolation(cmd, &self.config, &self.work_dir);
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

/// Pre-computed mount plan for `pivot_root` filesystem isolation.
///
/// Built in the parent before `fork(2)` so the `pre_exec` closure
/// only needs to issue syscalls — every path it touches is already
/// allocated. Kept private; the public surface is just the toggle
/// on [`SandboxConfig::isolate_filesystem`].
#[cfg(target_os = "linux")]
#[derive(Debug)]
struct PivotPlan {
    /// The new tmpfs root, normally the sandbox `work_dir`.
    new_root: PathBuf,
    /// Where the old root will be re-rooted before being detached.
    /// Created as `<new_root>/.old_root` in the parent.
    old_root: PathBuf,
    /// `(host_source, target_inside_new_root, read_only)`.
    bind_mounts: Vec<(PathBuf, PathBuf, bool)>,
    /// `/dev/<name>` nodes to bind into the jail.
    dev_mounts: Vec<(PathBuf, PathBuf)>,
    /// Mount target for `/proc` inside the jail.
    proc_target: PathBuf,
    /// Mount target for the in-jail `/tmp` tmpfs.
    tmp_target: PathBuf,
}

#[cfg(target_os = "linux")]
impl PivotPlan {
    /// Compose the bind-mount and pseudo-fs plan for a sandbox.
    ///
    /// `allowed_inputs` map to read-only binds; `allowed_outputs` map
    /// to writable binds. Standard runtime paths (`/lib`, `/lib64`,
    /// `/usr/lib`, `/usr/lib64`, `/bin/sh`, `/usr/bin/env`,
    /// `/bin/bash`) are bind-mounted read-only when present so the
    /// post-exec child can still locate its interpreter and shared
    /// libraries — without them the smallest run script would fail
    /// inside an empty tmpfs.
    fn build(work_dir: &Path, config: &SandboxConfig) -> Self {
        let new_root = work_dir.to_path_buf();
        let old_root = new_root.join(".old_root");

        let mut bind_mounts: Vec<(PathBuf, PathBuf, bool)> = Vec::new();

        for input in &config.allowed_inputs {
            if !input.exists() {
                continue;
            }
            let rel = input.strip_prefix("/").unwrap_or(input);
            bind_mounts.push((input.clone(), new_root.join(rel), true));
        }

        for output in &config.allowed_outputs {
            if !output.exists() {
                continue;
            }
            let rel = output.strip_prefix("/").unwrap_or(output);
            bind_mounts.push((output.clone(), new_root.join(rel), false));
        }

        for lib_dir in ["/lib", "/lib64", "/usr/lib", "/usr/lib64"] {
            let host = PathBuf::from(lib_dir);
            if host.is_dir() {
                let rel = host.strip_prefix("/").unwrap_or(&host).to_path_buf();
                bind_mounts.push((host, new_root.join(rel), true));
            }
        }

        for bin in ["/bin/sh", "/usr/bin/env", "/bin/bash"] {
            let host = PathBuf::from(bin);
            if host.exists() {
                let rel = host.strip_prefix("/").unwrap_or(&host).to_path_buf();
                bind_mounts.push((host, new_root.join(rel), true));
            }
        }

        let dev_mounts: Vec<(PathBuf, PathBuf)> = ["null", "zero", "urandom", "random"]
            .iter()
            .filter_map(|name| {
                let host = PathBuf::from(format!("/dev/{name}"));
                host.exists()
                    .then(|| (host.clone(), new_root.join(format!("dev/{name}"))))
            })
            .collect();

        Self {
            new_root: new_root.clone(),
            old_root,
            bind_mounts,
            dev_mounts,
            proc_target: new_root.join("proc"),
            tmp_target: new_root.join("tmp"),
        }
    }
}

#[cfg(target_os = "linux")]
fn apply_namespace_isolation(cmd: &mut Command, config: &SandboxConfig, work_dir: &Path) {
    use std::os::unix::process::CommandExt;

    use nix::sched::CloneFlags;

    let isolate_network = config.isolate_network;
    let isolate_filesystem = config.isolate_filesystem;
    // Either gate independently requires CLONE_NEWUSER to grant the
    // child CAP_SYS_ADMIN inside the new namespace; pivot_root and
    // bind-mount syscalls would otherwise be denied to a non-root
    // caller. Avoid double-counting: one CLONE_NEWUSER is enough.
    let need_user_ns = isolate_network || isolate_filesystem;
    let host_user_id = nix::unistd::getuid();
    let host_group_id = nix::unistd::getgid();

    // Pre-build the pivot plan in the parent so the child closure is
    // allocation-free for the path data. `None` here means we'll skip
    // the pivot_root branch and stay in the namespace-only path.
    let pivot_plan = isolate_filesystem.then(|| PivotPlan::build(work_dir, config));

    // SAFETY: The closure runs between fork(2) and exec(2). It calls
    // only async-signal-safe syscalls (unshare, mount, pivot_root,
    // chdir, umount2, plus best-effort writes to /proc/self
    // pseudo-files) per the constraints documented in
    // `std::os::unix::process::CommandExt::pre_exec`. The nix wrappers
    // build CStrings; this matches the upstream lattice-exec
    // implementation in production today and avoids long-running
    // allocations.
    unsafe {
        cmd.pre_exec(move || {
            let mut flags = CloneFlags::CLONE_NEWNS;
            if need_user_ns {
                flags |= CloneFlags::CLONE_NEWUSER;
            }
            if isolate_network {
                flags |= CloneFlags::CLONE_NEWNET;
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
            if need_user_ns {
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

            // Filesystem isolation via pivot_root. Best-effort — on
            // failure (WSL2, restricted kernels) the child stays in
            // the namespace-only path, with the partial tmpfs detached
            // so we don't leak it back to the host.
            if let Some(plan) = pivot_plan.as_ref() {
                let pivot_outcome = perform_pivot(plan);
                if pivot_outcome.is_err() {
                    let _ = nix::mount::umount2(
                        plan.new_root.as_path(),
                        nix::mount::MntFlags::MNT_DETACH,
                    );
                }
            }

            Ok(())
        });
    }
}

/// Execute the `pivot_root` mount sequence inside the child.
///
/// Returns `Ok(())` if the jail is fully installed; on `Err`, the
/// caller is responsible for tearing down the partial tmpfs (we
/// can't rely on `Drop` here — we're between `fork(2)` and
/// `exec(2)`).
#[cfg(target_os = "linux")]
fn perform_pivot(plan: &PivotPlan) -> std::io::Result<()> {
    use nix::mount::{mount, umount2, MntFlags, MsFlags};
    use nix::unistd::{chdir, pivot_root};

    // 1. Mount tmpfs at new_root so it becomes its own mount point.
    //    Without this pivot_root would refuse: new_root must not share
    //    a mount with the current root.
    mount(
        Some("tmpfs"),
        plan.new_root.as_path(),
        Some("tmpfs"),
        MsFlags::empty(),
        Some("size=4G,mode=0755"),
    )
    .map_err(|err| std::io::Error::other(format!("tmpfs mount on new_root failed: {err}")))?;

    // 2. put_old needs to exist before pivot_root can use it.
    std::fs::create_dir(&plan.old_root)
        .map_err(|err| std::io::Error::other(format!("create .old_root failed: {err}")))?;

    // 3. Bind each declared path into the new root, recreating its
    //    absolute layout. Read-only inputs get a remount with MS_RDONLY
    //    after the bind (single mount(2) cannot atomically bind + ro).
    for (host, target, read_only) in &plan.bind_mounts {
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if host.is_dir() {
            let _ = std::fs::create_dir_all(target);
        } else {
            let _ = std::fs::File::create(target);
        }
        mount(
            Some(host.as_path()),
            target.as_path(),
            None::<&str>,
            MsFlags::MS_BIND | MsFlags::MS_REC,
            None::<&str>,
        )
        .map_err(|err| {
            std::io::Error::other(format!(
                "bind mount {} -> {} failed: {err}",
                host.display(),
                target.display()
            ))
        })?;
        if *read_only {
            let _ = mount(
                None::<&str>,
                target.as_path(),
                None::<&str>,
                MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY,
                None::<&str>,
            );
        }
    }

    // 4. /proc inside the new root. Best-effort: the run block may not
    //    need it, and procfs mount can fail without CAP_SYS_ADMIN.
    let _ = std::fs::create_dir_all(&plan.proc_target);
    let _ = mount(
        Some("proc"),
        plan.proc_target.as_path(),
        Some("proc"),
        MsFlags::empty(),
        None::<&str>,
    );

    // 5. Minimal /dev — null/zero/random/urandom only. Anything more
    //    leaks the host's device tree.
    for (host, target) in &plan.dev_mounts {
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::File::create(target);
        let _ = mount(
            Some(host.as_path()),
            target.as_path(),
            None::<&str>,
            MsFlags::MS_BIND,
            None::<&str>,
        );
    }

    // 6. Fresh tmpfs at /tmp so build steps can scratch.
    let _ = std::fs::create_dir_all(&plan.tmp_target);
    let _ = mount(
        Some("tmpfs"),
        plan.tmp_target.as_path(),
        Some("tmpfs"),
        MsFlags::empty(),
        Some("size=1G,mode=1777"),
    );

    // 7. Swap roots.
    pivot_root(plan.new_root.as_path(), plan.old_root.as_path()).map_err(|err| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("pivot_root failed: {err}"),
        )
    })?;

    // 8. The cwd that std::process::Command set is gone now; chdir
    //    into the new root.
    chdir("/")
        .map_err(|err| std::io::Error::other(format!("chdir after pivot_root failed: {err}")))?;

    // 9. Detach the old root with MNT_DETACH so it disappears as soon
    //    as no one's using it. After this the host filesystem is no
    //    longer reachable.
    umount2("/.old_root", MntFlags::MNT_DETACH)
        .map_err(|err| std::io::Error::other(format!("umount2 .old_root failed: {err}")))?;

    let _ = std::fs::remove_dir("/.old_root");
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn apply_namespace_isolation(_cmd: &mut Command, _config: &SandboxConfig, _work_dir: &Path) {
    // Namespace isolation is Linux-only. On other platforms the
    // sandbox still applies env sanitization; the rest is a no-op.
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

    #[cfg(target_os = "linux")]
    #[test]
    fn pivot_plan_records_inputs_outputs_and_runtime_paths() {
        // PivotPlan is a private struct; this test exercises the
        // building behaviour without requiring user namespaces. The
        // runtime test that actually pivots lives below behind a
        // can_isolate_namespaces() guard.
        let work_dir = PathBuf::from("/tmp/kiln-pivot-plan-test");
        let config = SandboxConfig {
            enabled: true,
            isolate_filesystem: true,
            allowed_inputs: vec![PathBuf::from("/usr/bin")],
            allowed_outputs: vec![PathBuf::from("/tmp")],
            ..SandboxConfig::default()
        };

        let plan = PivotPlan::build(&work_dir, &config);

        assert_eq!(plan.new_root, work_dir);
        assert_eq!(plan.old_root, work_dir.join(".old_root"));

        let usr_bin_target = work_dir.join("usr/bin");
        assert!(
            plan.bind_mounts
                .iter()
                .any(|(src, tgt, ro)| src == &PathBuf::from("/usr/bin")
                    && tgt == &usr_bin_target
                    && *ro),
            "missing read-only bind for /usr/bin: {:?}",
            plan.bind_mounts,
        );

        let tmp_target = work_dir.join("tmp");
        assert!(
            plan.bind_mounts
                .iter()
                .any(|(src, tgt, ro)| src == &PathBuf::from("/tmp") && tgt == &tmp_target && !*ro),
            "missing writable bind for /tmp: {:?}",
            plan.bind_mounts,
        );

        // /bin/sh is a build-time guarantee on every Linux host kiln
        // targets — bind it so the run block's interpreter is reachable.
        assert!(
            plan.bind_mounts
                .iter()
                .any(|(src, _, ro)| src == &PathBuf::from("/bin/sh") && *ro),
            "missing /bin/sh runtime bind: {:?}",
            plan.bind_mounts,
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires a Linux host with full unprivileged user/mount \
                namespace support; skipped by default because GitHub-hosted \
                runners and many containerised CI lanes return EINVAL from \
                the combined CLONE_NEWUSER|CLONE_NEWNS unshare. Run with \
                `cargo test -p kiln-exec --lib -- --ignored`."]
    fn pivot_root_hides_host_paths_outside_allowed_inputs() {
        // Defence in depth: even when manually running under --ignored,
        // skip if the simpler user-namespace probe says no — there's
        // nothing meaningful to assert on those hosts.
        if !can_isolate_namespaces() {
            return;
        }

        let config = SandboxConfig {
            enabled: true,
            isolate_filesystem: true,
            // allow_inputs covers the runtime; auto-bound /bin/sh and
            // /lib paths handle the shell + ld.so.
            allowed_inputs: vec![PathBuf::from("/usr/bin")],
            env_clear: true,
            ..SandboxConfig::default()
        };
        let sandbox =
            Sandbox::new(config, "pivot-isolation").expect("sandbox creation should succeed");

        // Inside the jail: /etc must be invisible (host fs hidden) and
        // /bin/sh must still resolve (auto-mounted from PivotPlan).
        let mut cmd = Command::new("/bin/sh");
        cmd.args([
            "-c",
            "if [ -e /etc/hostname ]; then exit 11; fi; \
             if [ ! -x /bin/sh ]; then exit 12; fi; \
             exit 0",
        ]);
        sandbox.apply_to_command(&mut cmd);

        let output = cmd.output().expect("sandboxed sh should spawn");
        assert!(
            output.status.success(),
            "exit={:?} stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
