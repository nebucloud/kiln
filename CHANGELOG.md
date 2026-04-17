## Changelog

All notable changes to kiln are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project follows [Semantic Versioning](https://semver.org/) once
1.0.0 ships. Pre-1.0 minor versions may include breaking changes.

## [0.1.0] — 2026-04-16

Initial public release. Five crates published to crates.io:
`kiln-core`, `kiln-cache`, `kiln-exec`, `kiln` (facade),
`kiln-cli`. The `kiln-service` (gRPC) crate is intentionally held
back for the 0.2 release alongside the Conductor MCP integration.

This release ships unsigned. Per
[KLN-PLAN-extraction §7][plan] the first SSF-signed release is a
0.1.x patch that lands once SSF Phase 2 ships (SSF can't sign the
release that gates its own existence).

[plan]: https://github.com/nebucloud/docs/blob/main/KLN-PLAN-extraction.md

### Added — `kiln-core`

- `Pipeline` / `Target` / `TargetId` / `ShellBlock` /
  `PipelineVersion` — the bag-of-targets the planner and executor
  consume. JSON wire format keyed by `TargetId` per
  [KLN-D-extraction-decisions §02][kln-d].
- `FetchSpec` — declarations for the fetch-then-seal phase
  (KLN-D-04). BLAKE3 verification only in 0.1.0; SHA-256 lands
  in 0.2.
- `Resource` / `ResourceRef` / `ResourceId` / `AccessMode` —
  shared mutable state targets can declare.
- `KilnType` — schema metadata for typed inputs/outputs (informational).
- `KilnError` — single situation-specific error per
  M-ERRORS-CANONICAL-STRUCTS, with five `is_*` query methods and
  `std::error::Error::source()` chaining.
- `PipelineBuilder` / `TargetBuilder` — chainable construction
  per KLN-D-02 / M-INIT-BUILDER.
- `manifest::{from_json_str, from_json_reader, to_json_string,
  to_json_string_pretty, to_json_writer}` — JSON I/O helpers,
  also exposed as `Pipeline::from_json_str` etc.
- `validator::validate` — structural cross-reference checks.
  `Pipeline::validate()` delegates here.
- `planner::build_execution_plan` — petgraph-based topological
  sort + Kahn's algorithm with greedy conflict / exclusive-resource
  splitting. Ported from
  `terranoxos/terranox-tools/lattice-exec/src/planner.rs`.

[kln-d]: https://github.com/nebucloud/docs/blob/main/KLN-D-extraction-decisions.md

### Added — `kiln-cache`

- `ContentHash` (BLAKE3 32-byte digest, lowercase-hex serde,
  `FromStr`) — ported from
  `terranoxos/terranox-tools/lattice-core/src/store.rs`.
- `CacheKey` — three-component (input/script/tool) hash with a
  combined-hash address.
- `Store` trait — pluggable byte storage at the (key, name) level.
  `Send + Sync` for cross-wave concurrency.
- `LocalDiskStore` — sharded local-disk implementation.
  Layout: `{root}/{ab}/{cd...}/` to keep any single directory
  bounded.
- `ShardedCache<S>` — high-level wrapper with `lookup`, `store`,
  `contains`, `evict`, `compute_key` for any `Store` impl. Ported
  from `terranoxos/terranox-tools/lattice-exec/src/cache.rs`
  (renamed from `ExecutionCache`).
- `CacheError` — distinct from `KilnError`; covers I/O,
  corruption, hash mismatch, and JSON failures with `is_*` queries.

### Added — `kiln-exec`

- `Sandbox` / `SandboxConfig` — per-target temporary workspace
  with env sanitization. Linux-only namespace isolation
  (`isolate_network` seals the network per KLN-D-04 via
  CLONE_NEWUSER + CLONE_NEWNET). `pivot_root`, cgroups, and PID
  namespace isolation are stored as forward-compatible
  `SandboxConfig` fields but are no-ops in 0.1.0 (deferred to
  kiln 0.2).
- `can_isolate_namespaces()` — host probe respecting
  `KILN_NO_SANDBOX=1` and WSL detection.
- `ShellBackend::resolve` — interpreter lookup via host `which`.
- `run_target` / `run_target_in_sandbox` / `run_target_cached` —
  per-target execution with watchdog-based wall-clock timeout,
  cleanup-block-on-failure, and optional cache lookup/store.
- `Fetcher` trait + `MockFetcher` — pluggable fetch-then-seal
  surface. No bundled HTTP impl in 0.1.0 (M-MOCKABLE-SYSCALLS
  prefers an injectable fetcher).
- `Executor` — public facade that composes planner + cache +
  fetch + per-target run. `execute(&Pipeline) ->
  Result<ExecutionReport, ExecError>` with `with_cache` /
  `with_fetcher` chain methods.
- `ExecutionReport` — per-target results keyed by `TargetId`,
  the executed wave grouping, and `target_count` /
  `cache_hits` / `cache_misses` helpers.
- `ExecError` — boxed-kind error type with eight `is_*` query
  methods and `From` conversions from `CacheError` / `KilnError`.

### Added — `kiln` (facade)

- 0.1.0 ships an empty facade crate scaffold so the name is
  reserved on crates.io. Real re-exports land in 0.2 once the
  cross-crate API surface stabilizes.

### Added — `kiln-cli`

- `kiln run <pipeline.json>` — execute a pipeline; per-wave
  output with `CACHE HIT` / `RAN` badges and a summary.
- `kiln validate <pipeline.json>` — parse + structural check, no
  execution.
- `kiln cache stats --cache <dir>` — entry count + total bytes
  (human-readable).
- `kiln cache clear --cache <dir> --yes` — destructive; requires
  `--yes`.
- `kiln inspect <hex-key> --cache <dir>` — pretty-print one
  cache entry.

### Conventions

- Rust edition 2021, MSRV **1.81** (the ms-rust restriction
  lints require the `reason = "..."` attribute field, stabilized
  in 1.81 alongside `#[expect]`).
- Linux only for 0.1.x (sandbox internals use the `nix` crate's
  `sched`/`mount`/`user` modules). macOS sandbox / Windows job
  objects land in 0.3+.
- Dual-licensed under Apache-2.0 OR MIT.

### Tests

137 unit + 14 doctests across the workspace, all green under
clippy `-D warnings` with the M-STATIC-VERIFICATION lint pack
(see `Cargo.toml [workspace.lints]`).
