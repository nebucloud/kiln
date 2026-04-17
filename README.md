# kiln

Hermetic, content-addressed, parallel task execution.

> Status: **0.1.0 published to crates.io** (April 16, 2026). See the
> milestone plan in [`KLN-PLAN-extraction.md`][plan] in
> `nebucloud/docs` for the full roadmap.

[plan]: https://github.com/nebucloud/docs/blob/main/KLN-PLAN-extraction.md

## What kiln is

kiln runs declarative task pipelines inside Linux namespace
sandboxes, with content-addressed BLAKE3 caching, polyglot shell
dispatch, and DAG-extracted parallelism. It is the public,
re-usable extraction of the hermetic-execution engine that started
life inside `terranoxos/terranox-tools/lattice-exec`.

## Crates

| crates.io | Source | Purpose |
|-----------|--------|---------|
| [`kiln-core`](https://crates.io/crates/kiln-core) | [crates/kiln-core](crates/kiln-core) | Types, planner, validation |
| [`kiln-cache`](https://crates.io/crates/kiln-cache) | [crates/kiln-cache](crates/kiln-cache) | Content-addressed cache (BLAKE3) |
| [`kiln-exec`](https://crates.io/crates/kiln-exec) | [crates/kiln-exec](crates/kiln-exec) | Sandbox + runner + shell backends |
| [`kiln-runtime`](https://crates.io/crates/kiln-runtime) | [crates/kiln-runtime](crates/kiln-runtime) | Facade — re-exports the public surface ¹ |
| [`kiln-cli`](https://crates.io/crates/kiln-cli) | [crates/kiln-cli](crates/kiln-cli) | `kiln` binary (clap) |
| [`kiln-service`](https://crates.io/crates/kiln-service) | [crates/kiln-service](crates/kiln-service) | gRPC server — held back for 0.2.0 |

¹ The canonical short name `kiln` was already claimed (and yanked)
on crates.io. A transfer ticket is open with help@crates.io; if
granted, `kiln` will publish as a re-export of `kiln-runtime` in
0.2.

## Install

```bash
# Just the CLI (binary name `kiln`):
cargo install kiln-cli

# As a library dependency:
cargo add kiln-core kiln-cache kiln-exec
```

## Build from source

```bash
cargo check --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

MSRV: 1.81. Linux-only for 0.1.x — see the risk register in the
extraction plan.

## License

Dual-licensed under either of:

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT license](LICENSE-MIT)

at your option.
