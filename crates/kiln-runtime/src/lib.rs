// Rust guideline compliant 2026-02-21
//! kiln-runtime — facade over `kiln-core`, `kiln-cache`, and `kiln-exec`.
//!
//! Crate name was chosen because the canonical short name `kiln` was
//! already claimed (and yanked) on crates.io. The transfer ticket is
//! open with help@crates.io; if granted, the `kiln` crate will be
//! published as a re-export of `kiln-runtime` and this crate will be
//! deprecated cleanly.
//!
//! 0.1.0 ships an empty facade so the name is reserved on crates.io
//! and downstream consumers have a stable handle to depend on.
//! Convenience re-exports + a `prelude` module land in 0.2 once the
//! cross-crate API surface stabilizes.
