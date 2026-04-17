// Rust guideline compliant 2026-02-21
//! Multi-target [`Pipeline`] runner — the public kiln-exec facade.
//!
//! [`Executor`] composes everything kiln-exec ships into a single
//! `execute(&Pipeline)` call:
//!
//! 1. [`build_execution_plan`] groups the pipeline's targets into
//!    dependency-ordered waves.
//! 2. For each target, optionally check the [`ShardedCache`] for a
//!    prior green run.
//! 3. On a cache miss, build the target's [`Sandbox`], pull every
//!    declared [`FetchSpec`] via the configured [`Fetcher`] (network
//!    still open), then call [`run_target_in_sandbox`] (network now
//!    sealed if `isolate_network` is true).
//! 4. On a successful run, store the outcome in the cache.
//!
//! 0.1.0 runs waves and intra-wave targets sequentially. Concurrent
//! intra-wave execution lands in 0.2 alongside the tokio-aware
//! `executor::Async` variant.

use std::collections::BTreeMap;

use kiln_cache::{LocalDiskStore, ShardedCache};
use kiln_core::{build_execution_plan, ExecutionPlan, FetchSpec, Pipeline, Target, TargetId};

use crate::error::ExecError;
use crate::fetch::Fetcher;
use crate::runner::{run_target_in_sandbox, RunResult};
use crate::sandbox::{Sandbox, SandboxConfig};

/// The aggregated outcome of executing a [`Pipeline`].
///
/// `waves` mirrors the planner's grouping so callers can correlate
/// `results` with the original execution order. `results` is keyed by
/// [`TargetId`] for stable iteration.
#[derive(Debug, Clone)]
pub struct ExecutionReport {
    /// Per-target outcomes keyed by id.
    pub results: BTreeMap<TargetId, RunResult>,
    /// The wave grouping that was executed (mirrors [`ExecutionPlan::waves`]).
    pub waves: Vec<Vec<TargetId>>,
}

impl ExecutionReport {
    /// Returns the number of targets executed (cache hits included).
    #[must_use]
    pub fn target_count(&self) -> usize {
        self.results.len()
    }

    /// Returns the number of targets served from cache.
    #[must_use]
    pub fn cache_hits(&self) -> usize {
        self.results.values().filter(|r| r.cache_hit).count()
    }

    /// Returns the number of targets that actually executed (cache misses).
    #[must_use]
    pub fn cache_misses(&self) -> usize {
        self.target_count() - self.cache_hits()
    }
}

/// Multi-target [`Pipeline`] executor.
///
/// Holds the per-execution configuration (sandbox defaults, optional
/// cache, optional fetcher) and runs every target a [`Pipeline`]
/// declares in dependency order.
///
/// # Examples
///
/// ```no_run
/// use kiln_cache::{LocalDiskStore, ShardedCache};
/// use kiln_core::{Pipeline, Target, TargetId};
/// use kiln_exec::{Executor, MockFetcher, SandboxConfig};
///
/// let pipeline = Pipeline::builder()
///     .target(
///         "build",
///         Target::builder()
///             .shell("bash")
///             .run("cargo build")
///             .build()
///             .unwrap(),
///     )
///     .build()
///     .unwrap();
///
/// let cache = ShardedCache::new(LocalDiskStore::new("/var/cache/kiln"));
///
/// let report = Executor::new(SandboxConfig::default())
///     .with_cache(cache)
///     .with_fetcher(MockFetcher::new())
///     .execute(&pipeline)
///     .unwrap();
///
/// println!(
///     "{} targets ran ({} cache hits)",
///     report.target_count(),
///     report.cache_hits(),
/// );
/// ```
pub struct Executor {
    sandbox_config: SandboxConfig,
    cache: Option<ShardedCache<LocalDiskStore>>,
    fetcher: Option<Box<dyn Fetcher>>,
}

impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Executor")
            .field("sandbox_config", &self.sandbox_config)
            .field("cache", &self.cache.as_ref().map(|_| "ShardedCache"))
            .field("fetcher", &self.fetcher.as_ref().map(|_| "dyn Fetcher"))
            .finish()
    }
}

impl Executor {
    /// Constructs an `Executor` with the given default [`SandboxConfig`].
    ///
    /// The cache and fetcher are unset by default. Add them via
    /// [`Self::with_cache`] / [`Self::with_fetcher`].
    #[must_use]
    pub fn new(sandbox_config: SandboxConfig) -> Self {
        Self {
            sandbox_config,
            cache: None,
            fetcher: None,
        }
    }

    /// Attaches a [`ShardedCache`] for cache lookup / store on each target.
    #[must_use]
    pub fn with_cache(mut self, cache: ShardedCache<LocalDiskStore>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Attaches a [`Fetcher`] used to pull each target's declared fetches.
    ///
    /// Required when any target in the pipeline declares
    /// [`FetchSpec`] entries; otherwise execution returns
    /// [`ExecError::is_fetch_failed`] for those targets.
    #[must_use]
    pub fn with_fetcher<F: Fetcher + 'static>(mut self, fetcher: F) -> Self {
        self.fetcher = Some(Box::new(fetcher));
        self
    }

    /// Returns the configured default [`SandboxConfig`].
    #[must_use]
    pub fn sandbox_config(&self) -> &SandboxConfig {
        &self.sandbox_config
    }

    /// Executes a [`Pipeline`] end-to-end.
    ///
    /// Steps:
    ///
    /// 1. Validate the pipeline structurally
    ///    ([`Pipeline::validate`](kiln_core::Pipeline::validate)).
    /// 2. Build an [`ExecutionPlan`] from the pipeline.
    /// 3. Execute each wave sequentially. For each target in the
    ///    wave: cache lookup, fetch staging, sandboxed run, cache
    ///    store.
    /// 4. Return an [`ExecutionReport`] aggregating every target's
    ///    [`RunResult`].
    ///
    /// # Errors
    ///
    /// Surfaces every variant [`ExecError`] can produce. The first
    /// failed target stops execution — subsequent waves do not run.
    pub fn execute(&self, pipeline: &Pipeline) -> Result<ExecutionReport, ExecError> {
        pipeline.validate()?;
        let plan: ExecutionPlan = build_execution_plan(pipeline)?;

        let mut results: BTreeMap<TargetId, RunResult> = BTreeMap::new();

        for wave in &plan.waves {
            for target_id in wave {
                let target = pipeline.target(target_id).ok_or_else(|| {
                    // Defensive: validate() + build_execution_plan() above
                    // already guarantee every wave id is in the pipeline.
                    // If we hit this branch the planner returned a stale id.
                    ExecError::sandbox_setup(
                        format!("planner referenced unknown target id `{target_id}`"),
                        None,
                    )
                })?;

                let result = self.execute_target(target_id, target)?;
                results.insert(target_id.clone(), result);
            }
        }

        Ok(ExecutionReport {
            results,
            waves: plan.waves,
        })
    }

    /// Runs a single target through the cache + fetch + sandbox pipeline.
    fn execute_target(
        &self,
        target_id: &TargetId,
        target: &Target,
    ) -> Result<RunResult, ExecError> {
        // 1. Cache lookup before any execution work.
        if let Some(cache) = &self.cache {
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
                // Cached failure — re-execute (failures may be transient).
            }
        }

        // 2. Build the sandbox up front so fetches land in its workspace.
        let sandbox = Sandbox::new(self.sandbox_config.clone(), target_id.as_ref())?;

        // 3. Fetch phase — runs while the network is still open
        //    (apply_to_command in run_target_in_sandbox is what
        //    actually seals the network on Linux).
        if !target.fetches.is_empty() {
            let fetcher = self.fetcher.as_deref().ok_or_else(|| {
                ExecError::fetch_failed(
                    "<no fetcher>",
                    format!(
                        "target `{target_id}` declares {} fetch(es) but the executor has no Fetcher attached; \
                         call Executor::with_fetcher",
                        target.fetches.len()
                    ),
                )
            })?;
            stage_fetches(target.fetches.as_slice(), fetcher, sandbox.work_dir())?;
        }

        // 4. Run target with the sandbox we just staged.
        let result = run_target_in_sandbox(target_id, target, &sandbox)?;

        // 5. Cache store on success.
        if let Some(cache) = &self.cache {
            let key = cache.compute_key(target);
            // Best-effort store: if writing the cache fails, the run's
            // result is still valid — we just lose the cache benefit
            // for next time.
            let _ = cache.store(
                &key,
                target_id.as_ref(),
                result.exit_code,
                &result.stdout,
                &result.stderr,
            );
        }

        Ok(result)
    }
}

/// Pulls each [`FetchSpec`] via the [`Fetcher`] and writes the bytes
/// into the sandbox workspace.
///
/// Verification is delegated to the fetcher: if a [`FetchSpec`]
/// carries a `blake3_hex` digest, this function parses it into a
/// [`ContentHash`](kiln_cache::ContentHash) and asks the fetcher to
/// verify on its side. Fetchers must reject mismatches with
/// [`ExecError::is_fetch_failed`].
fn stage_fetches(
    specs: &[FetchSpec],
    fetcher: &dyn Fetcher,
    workspace: &std::path::Path,
) -> Result<(), ExecError> {
    for spec in specs {
        let expected = match &spec.blake3_hex {
            Some(hex) => Some(kiln_cache::ContentHash::from_hex(hex).map_err(|err| {
                ExecError::fetch_failed(
                    spec.url.clone(),
                    format!("invalid blake3 hex in fetch spec: {err}"),
                )
            })?),
            None => None,
        };
        let bytes = fetcher.fetch(&spec.url, expected.as_ref())?;

        let dest = if spec.destination.is_absolute() {
            spec.destination.clone()
        } else {
            workspace.join(&spec.destination)
        };

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                ExecError::io(
                    err,
                    format!("creating fetch destination parent `{}`", parent.display()),
                )
            })?;
        }

        std::fs::write(&dest, &bytes).map_err(|err| {
            ExecError::io(
                err,
                format!("writing fetched bytes to `{}`", dest.display()),
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::MockFetcher;
    use kiln_cache::ContentHash;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(prefix: &str) -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("kiln-exec-{prefix}-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn pipeline_with_one_target(id: &str, code: &str) -> Pipeline {
        Pipeline::builder()
            .target(
                id,
                Target::builder().shell("bash").run(code).build().unwrap(),
            )
            .build()
            .unwrap()
    }

    #[test]
    fn execute_single_target_pipeline_succeeds() {
        let pipeline = pipeline_with_one_target("a", "echo hello");
        let report = Executor::new(SandboxConfig::default())
            .execute(&pipeline)
            .unwrap();

        assert_eq!(report.target_count(), 1);
        assert_eq!(report.cache_hits(), 0);
        assert_eq!(report.waves, vec![vec![TargetId::new("a")]]);
        let result = report.results.get(&TargetId::new("a")).unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello"));
    }

    #[test]
    fn execute_runs_waves_in_dependency_order() {
        // a writes a marker; b (requires a) reads + cleans it up.
        let marker = temp_dir("wave-order").join("marker");
        let _ = std::fs::create_dir_all(marker.parent().unwrap());

        let mut b = Target::builder()
            .shell("bash")
            .run(format!(
                "cat {} && rm {}",
                marker.display(),
                marker.display()
            ))
            .build()
            .unwrap();
        b.requires.push(TargetId::new("a"));

        let pipeline = Pipeline::builder()
            .target(
                "a",
                Target::builder()
                    .shell("bash")
                    .run(format!("echo wave-a > {}", marker.display()))
                    .build()
                    .unwrap(),
            )
            .target("b", b)
            .build()
            .unwrap();

        let report = Executor::new(SandboxConfig::default())
            .execute(&pipeline)
            .unwrap();

        assert_eq!(report.waves.len(), 2);
        assert_eq!(report.waves[0], vec![TargetId::new("a")]);
        assert_eq!(report.waves[1], vec![TargetId::new("b")]);
        let b_result = report.results.get(&TargetId::new("b")).unwrap();
        assert!(b_result.stdout.contains("wave-a"));

        let _ = std::fs::remove_dir_all(marker.parent().unwrap());
    }

    #[test]
    fn execute_first_failure_stops_subsequent_waves() {
        let marker = temp_dir("fail-stop").join("c-ran");
        let _ = std::fs::create_dir_all(marker.parent().unwrap());

        let mut c = Target::builder()
            .shell("bash")
            .run(format!("touch {}", marker.display()))
            .build()
            .unwrap();
        c.requires.push(TargetId::new("b"));

        let pipeline = Pipeline::builder()
            .target(
                "a",
                Target::builder().shell("bash").run("true").build().unwrap(),
            )
            .target(
                "b",
                Target::builder()
                    .shell("bash")
                    .run("exit 1")
                    .build()
                    .unwrap(),
            )
            .target("c", c)
            .build()
            .unwrap();

        let err = Executor::new(SandboxConfig::default())
            .execute(&pipeline)
            .unwrap_err();
        assert!(err.is_target_failed());
        assert!(!marker.exists(), "wave 2 should not run after wave 1 fails");

        let _ = std::fs::remove_dir_all(marker.parent().unwrap());
    }

    #[test]
    fn cache_hit_serves_from_store_without_re_execution() {
        let cache_dir = temp_dir("cache-hit");
        let cache = ShardedCache::new(LocalDiskStore::new(&cache_dir));

        let pipeline = pipeline_with_one_target("a", "echo cached");

        let executor1 = Executor::new(SandboxConfig::default()).with_cache(cache.clone());
        let report1 = executor1.execute(&pipeline).unwrap();
        assert_eq!(report1.cache_hits(), 0);

        // Re-run with a fresh executor pointing at the same cache. The
        // second run should be a hit.
        let executor2 = Executor::new(SandboxConfig::default()).with_cache(cache);
        let report2 = executor2.execute(&pipeline).unwrap();
        assert_eq!(report2.cache_hits(), 1);
        let result = report2.results.get(&TargetId::new("a")).unwrap();
        assert!(result.cache_hit);
        assert!(result.stdout.contains("cached"));

        let _ = std::fs::remove_dir_all(&cache_dir);
    }

    #[test]
    fn fetch_target_writes_bytes_into_workspace_then_runs() {
        // Pre-register a mock response and set up a target whose run
        // block reads the fetched file and prints its contents.
        let fetcher = MockFetcher::new();
        let body = b"the answer is 42".to_vec();
        let url = "https://example/fetched.txt";
        fetcher.insert(url, body.clone());

        let target = Target::builder()
            .shell("bash")
            .run("cat fetched.txt")
            .fetch(url, "fetched.txt")
            .build()
            .unwrap();

        let pipeline = Pipeline::builder()
            .target("uses_fetch", target)
            .build()
            .unwrap();

        let report = Executor::new(SandboxConfig::default())
            .with_fetcher(fetcher)
            .execute(&pipeline)
            .unwrap();

        let result = report.results.get(&TargetId::new("uses_fetch")).unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(
            result.stdout.contains("the answer is 42"),
            "expected fetched contents in stdout, got: {:?}",
            result.stdout
        );
    }

    #[test]
    fn fetch_with_blake3_match_succeeds() {
        let fetcher = MockFetcher::new();
        let body = b"verified payload".to_vec();
        let url = "https://example/verified";
        let hex = ContentHash::from_data(&body).to_hex();
        fetcher.insert(url, body);

        let target = Target::builder()
            .shell("bash")
            .run("cat blob")
            .fetch_with_blake3(url, "blob", hex)
            .build()
            .unwrap();

        let pipeline = Pipeline::builder()
            .target("verified", target)
            .build()
            .unwrap();

        let report = Executor::new(SandboxConfig::default())
            .with_fetcher(fetcher)
            .execute(&pipeline)
            .unwrap();
        assert!(report
            .results
            .get(&TargetId::new("verified"))
            .unwrap()
            .stdout
            .contains("verified payload"));
    }

    #[test]
    fn fetch_with_blake3_mismatch_propagates_fetch_failed() {
        let fetcher = MockFetcher::new();
        fetcher.insert("https://example/x", b"actual body".to_vec());

        let wrong_hex = ContentHash::from_data(b"different body").to_hex();

        let target = Target::builder()
            .shell("bash")
            .run("true")
            .fetch_with_blake3("https://example/x", "blob", wrong_hex)
            .build()
            .unwrap();

        let pipeline = Pipeline::builder().target("bad", target).build().unwrap();

        let err = Executor::new(SandboxConfig::default())
            .with_fetcher(fetcher)
            .execute(&pipeline)
            .unwrap_err();
        assert!(err.is_fetch_failed(), "expected fetch_failed, got: {err}");
    }

    #[test]
    fn target_with_fetches_and_no_fetcher_errors_clearly() {
        let target = Target::builder()
            .shell("bash")
            .run("true")
            .fetch("https://example/x", "blob")
            .build()
            .unwrap();
        let pipeline = Pipeline::builder()
            .target("needs_fetch", target)
            .build()
            .unwrap();

        let err = Executor::new(SandboxConfig::default())
            .execute(&pipeline)
            .unwrap_err();
        assert!(err.is_fetch_failed());
        assert!(format!("{err}").contains("no Fetcher attached"));
    }

    #[test]
    fn empty_pipeline_produces_empty_report() {
        let pipeline = Pipeline::new();
        let report = Executor::new(SandboxConfig::default())
            .execute(&pipeline)
            .unwrap();
        assert!(report.results.is_empty());
        assert!(report.waves.is_empty());
        assert_eq!(report.target_count(), 0);
        assert_eq!(report.cache_hits(), 0);
    }

    #[test]
    fn report_counts_helpers_are_consistent() {
        let cache_dir = temp_dir("counts");
        let cache = ShardedCache::new(LocalDiskStore::new(&cache_dir));

        // Use distinct run blocks so each target gets its own cache key.
        // (kiln caches by target content, not id — two targets with the
        // same interpreter + code + inputs share an entry, which is a
        // feature, not what this test wants to exercise.)
        let pipeline = Pipeline::builder()
            .target(
                "a",
                Target::builder()
                    .shell("bash")
                    .run("echo target-a")
                    .build()
                    .unwrap(),
            )
            .target(
                "b",
                Target::builder()
                    .shell("bash")
                    .run("echo target-b")
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();

        // First pass: both miss.
        let report1 = Executor::new(SandboxConfig::default())
            .with_cache(cache.clone())
            .execute(&pipeline)
            .unwrap();
        assert_eq!(report1.target_count(), 2);
        assert_eq!(report1.cache_misses(), 2);

        // Second pass: both hit.
        let report2 = Executor::new(SandboxConfig::default())
            .with_cache(cache)
            .execute(&pipeline)
            .unwrap();
        assert_eq!(report2.target_count(), 2);
        assert_eq!(report2.cache_hits(), 2);
        assert_eq!(report2.cache_misses(), 0);

        let _ = std::fs::remove_dir_all(&cache_dir);
    }

    #[test]
    fn types_are_send() {
        const fn assert_send<T: Send>() {}
        assert_send::<ExecutionReport>();
        // Executor holds Box<dyn Fetcher> which is Send when the
        // trait says so; the trait does. Verifying explicitly:
        assert_send::<Executor>();
    }
}
