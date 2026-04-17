# CLAUDE.md — kiln workspace

## What this repo is

kiln is the public extraction of the hermetic execution engine from
`terranoxos/terranox-tools/lattice-exec`. It is owned by the
`nebucloud` GitHub org and published to crates.io under the
`nebucloud` umbrella. The canonical milestone plan lives in
`KLN-PLAN-extraction.md` in `nebucloud/docs`; structural decisions
live in `KLN-D-extraction-decisions.md` (KLN-D-01..04).

## Workspace layout

```
kiln/
├── Cargo.toml                  # workspace root
├── LICENSE-APACHE / LICENSE-MIT
├── README.md / CHANGELOG.md / CLAUDE.md
├── .github/workflows/ci.yml    # check, test, clippy, fmt
└── crates/
    ├── kiln-core/              # types, planner, validation (no I/O)
    ├── kiln-cache/             # content-addressed cache (BLAKE3)
    ├── kiln-exec/              # sandbox, runner, shell backends
    ├── kiln/                   # facade — re-exports
    ├── kiln-cli/               # `kiln` binary (clap)
    └── kiln-service/           # gRPC server (planned 0.2.0)
```

## Crate dependency graph

Dependencies flow strictly downward. No cycles.

```
kiln-cli ──→ kiln ──→ kiln-exec ──→ kiln-cache ──→ kiln-core
                          │                            ↑
                          └────────────────────────────┘
kiln-service ──→ kiln
```

`kiln-core` is the foundation: types and pure algorithms (planner,
validator). Zero internal deps. `kiln-cache` and `kiln-exec` build
on it. `kiln` re-exports the public surface used by binaries.

## Code conventions

- Rust edition 2021, MSRV 1.75
- `cargo fmt --all`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- Conventional commits: `feat(core): add planner`, `fix(exec): …`,
  `docs(cli): …`. Scope is the crate name without the `kiln-`
  prefix.
- `///` doc comments on public types and functions; `//!` for
  module-level docs.
- No `unwrap()` in library crates. `expect("…")` only with a
  descriptive message and only in binaries.
- All `unsafe` blocks need a `// SAFETY: …` comment explaining the
  invariant.
- Serde derives on all public types in `kiln-core`. Use
  `#[serde(rename_all = "snake_case")]` on enums.

## Build & test

```bash
cargo check --workspace
cargo test --workspace
cargo fmt --check --all
cargo clippy --workspace --all-targets -- -D warnings
cargo doc --workspace --no-deps --open
```

## Provenance for ported code

Files ported from `terranoxos/terranox-tools/crates/lattice-exec`
or `lattice-core` carry an attribution header (per
`KLN-PLAN-extraction.md` §6):

```rust
// Ported from terranoxos/terranox-tools/crates/lattice-exec/src/<file>.rs
// at commit <SHA>. See KLN-D-extraction-decisions for rationale.
```

## Things to watch out for

- `kiln-exec` integration tests use Linux namespaces and need
  `CAP_SYS_ADMIN` or a user namespace. CI runs them via
  `unshare --user --map-root-user` (see `ci.yml` once those tests
  land in M4).
- `tonic` / `prost` are declared in `[workspace.dependencies]` but
  only consumed by `kiln-service` (M8). Adding them to other
  crates pulls in the full gRPC stack — don't.
- The `nix` crate (Rust bindings for Linux syscalls) is unrelated
  to the Nix package manager. Both are referenced by the upstream
  lattice-exec source — context matters when porting.
