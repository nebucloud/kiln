# kiln

Hermetic, content-addressed, parallel task execution.

> Status: 0.1.0 in development. M1 (workspace skeleton) only.
> Real content lands in M2–M7. See the milestone plan in
> [`KLN-PLAN-extraction.md`][plan] in `nebucloud/docs`.

[plan]: https://github.com/nebucloud/docs/blob/main/KLN-PLAN-extraction.md

## What kiln is

kiln runs declarative task pipelines inside Linux namespace
sandboxes, with content-addressed BLAKE3 caching, polyglot shell
dispatch, and DAG-extracted parallelism. It is the public,
re-usable extraction of the hermetic-execution engine that started
life inside `terranoxos/terranox-tools/lattice-exec`.

## Crates

| Crate | Purpose | Milestone |
|-------|---------|-----------|
| [`kiln-core`](crates/kiln-core) | Types, planner, validation | M2 |
| [`kiln-cache`](crates/kiln-cache) | Content-addressed cache (BLAKE3) | M3 |
| [`kiln-exec`](crates/kiln-exec) | Sandbox + runner + shell backends | M4 |
| [`kiln`](crates/kiln) | Facade — re-exports public surface | M2–M4 |
| [`kiln-cli`](crates/kiln-cli) | `kiln` binary (clap) | M5 |
| [`kiln-service`](crates/kiln-service) | gRPC server (planned 0.2.0) | M8 |

## Build

```bash
cargo check --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

MSRV: 1.75. Linux-only for 0.1.x — see the risk register in the
extraction plan.

## License

Dual-licensed under either of:

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT license](LICENSE-MIT)

at your option.
