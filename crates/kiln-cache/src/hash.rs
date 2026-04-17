// Rust guideline compliant 2026-02-21
// Ported from terranoxos/terranox-tools/crates/lattice-core/src/store.rs
// (commit 7ab5316ac6 baseline). Renamed `StoreKey` -> `CacheKey`,
// dropped `StorePath` and `STORE_ROOT` (kiln-cache abstracts paths
// via the `Store` trait, no shared global root). See KLN-PLAN-extraction §6.
//! Hash primitives and cache keys.
//!
//! [`ContentHash`] wraps BLAKE3's 32-byte digest with serde, hex, and
//! ergonomics. [`CacheKey`] composes three [`ContentHash`]es covering
//! the inputs, script source, and interpreter (per the lattice
//! convention) into a single combined cache address.
//!
//! Hashes serialize as lowercase 64-character hex strings.

use std::fmt::{self, Debug, Display, Formatter};
use std::str::FromStr;

use kiln_core::Target;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::CacheError;

/// A BLAKE3 content hash (32 bytes).
///
/// Serialize/deserialize as a lowercase 64-char hex string.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    /// Creates a `ContentHash` from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Computes a `ContentHash` over arbitrary input bytes.
    #[must_use]
    pub fn from_data(data: &[u8]) -> Self {
        let hash = blake3::hash(data);
        Self(*hash.as_bytes())
    }

    /// Parses a `ContentHash` from a 64-character lowercase hex string.
    ///
    /// # Errors
    ///
    /// Returns [`CacheError::is_corruption`] when the string is not
    /// exactly 64 characters long or contains non-hex digits.
    pub fn from_hex(s: &str) -> Result<Self, CacheError> {
        let s = s.trim();
        if s.len() != 64 {
            return Err(CacheError::corruption(format!(
                "invalid hash length: expected 64 hex chars, got {}",
                s.len()
            )));
        }
        let mut bytes = [0u8; 32];
        for i in 0..32 {
            bytes[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_err| {
                CacheError::corruption(format!(
                    "invalid hex character at position {} in `{s}`",
                    i * 2
                ))
            })?;
        }
        Ok(Self(bytes))
    }

    /// Returns the lowercase hex digest (64 chars).
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            use std::fmt::Write as _;
            write!(s, "{b:02x}").expect("writing to a String never fails");
        }
        s
    }

    /// Returns the first 8 hex characters — handy for log lines.
    #[must_use]
    pub fn short(&self) -> String {
        self.to_hex()[..8].to_owned()
    }
}

impl Debug for ContentHash {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({})", self.short())
    }
}

impl Display for ContentHash {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.to_hex(), f)
    }
}

impl FromStr for ContentHash {
    type Err = CacheError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_hex(s)
    }
}

impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// A cache key composed from input, script, and tool hashes.
///
/// The combined cache address is `BLAKE3(input || script || tool)`,
/// computed lazily by [`CacheKey::combined`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CacheKey {
    /// Hash of the declared inputs.
    pub input_hash: ContentHash,
    /// Hash of the run-block source.
    pub script_hash: ContentHash,
    /// Hash of the interpreter (or pinned tool digest, eventually).
    pub tool_hash: ContentHash,
}

impl CacheKey {
    /// Constructs a `CacheKey` from its three constituent hashes.
    #[must_use]
    pub fn new(input_hash: ContentHash, script_hash: ContentHash, tool_hash: ContentHash) -> Self {
        Self {
            input_hash,
            script_hash,
            tool_hash,
        }
    }

    /// Returns the combined cache address: `BLAKE3(input || script || tool)`.
    #[must_use]
    pub fn combined(&self) -> ContentHash {
        let mut data = [0u8; 96];
        data[..32].copy_from_slice(&self.input_hash.0);
        data[32..64].copy_from_slice(&self.script_hash.0);
        data[64..].copy_from_slice(&self.tool_hash.0);
        ContentHash::from_data(&data)
    }

    /// Computes a `CacheKey` for the given [`Target`].
    ///
    /// Hashing inputs:
    ///
    /// - `input_hash`: BLAKE3 over the target's `inputs` name list,
    ///   joined by `\n` (the list is already kept in declaration order
    ///   by [`kiln_core::Target`]).
    /// - `script_hash`: BLAKE3 over `target.run.code`.
    /// - `tool_hash`: BLAKE3 over `target.run.interpreter`.
    #[must_use]
    pub fn from_target(target: &Target) -> Self {
        let input_blob: String = target.inputs.join("\n");
        Self::new(
            ContentHash::from_data(input_blob.as_bytes()),
            ContentHash::from_data(target.run.code.as_bytes()),
            ContentHash::from_data(target.run.interpreter.as_bytes()),
        )
    }
}

impl Display for CacheKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.combined(), f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_core::ShellBlock;

    fn target(interpreter: &str, code: &str) -> Target {
        Target::new(ShellBlock::new(interpreter, code))
    }

    #[test]
    fn content_hash_from_data_is_deterministic() {
        let h1 = ContentHash::from_data(b"hello world");
        let h2 = ContentHash::from_data(b"hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn content_hash_from_data_distinguishes_inputs() {
        let h1 = ContentHash::from_data(b"hello");
        let h2 = ContentHash::from_data(b"world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn content_hash_hex_round_trip() {
        let original = ContentHash::from_data(b"test data");
        let hex = original.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(ContentHash::from_hex(&hex).unwrap(), original);
    }

    #[test]
    fn content_hash_from_hex_rejects_wrong_length() {
        let err = ContentHash::from_hex("abcd").expect_err("too short");
        assert!(err.is_corruption());
    }

    #[test]
    fn content_hash_from_hex_rejects_non_hex_characters() {
        let bad = "z".repeat(64);
        let err = ContentHash::from_hex(&bad).expect_err("non-hex");
        assert!(err.is_corruption());
    }

    #[test]
    fn content_hash_serde_round_trip() {
        let hash = ContentHash::from_data(b"serde test");
        let json = serde_json::to_string(&hash).unwrap();
        assert!(json.starts_with('"') && json.ends_with('"'));
        let parsed: ContentHash = serde_json::from_str(&json).unwrap();
        assert_eq!(hash, parsed);
    }

    #[test]
    fn content_hash_short_first_eight() {
        let hash = ContentHash::from_data(b"short test");
        let short = hash.short();
        assert_eq!(short.len(), 8);
        assert!(hash.to_hex().starts_with(&short));
    }

    #[test]
    fn cache_key_combined_is_deterministic() {
        let key = CacheKey::new(
            ContentHash::from_data(b"in"),
            ContentHash::from_data(b"src"),
            ContentHash::from_data(b"tool"),
        );
        let c1 = key.combined();
        let c2 = key.combined();
        assert_eq!(c1, c2);
    }

    #[test]
    fn cache_key_from_target_changes_when_code_changes() {
        let t1 = target("bash", "make all");
        let t2 = target("bash", "make clean");
        let k1 = CacheKey::from_target(&t1);
        let k2 = CacheKey::from_target(&t2);
        assert_ne!(k1.combined(), k2.combined());
    }

    #[test]
    fn cache_key_from_target_changes_when_interpreter_changes() {
        let t1 = target("bash", "echo hi");
        let t2 = target("zsh", "echo hi");
        let k1 = CacheKey::from_target(&t1);
        let k2 = CacheKey::from_target(&t2);
        assert_ne!(k1.combined(), k2.combined());
    }

    #[test]
    fn cache_key_from_target_changes_when_inputs_change() {
        let mut t1 = target("bash", "make");
        t1.inputs.push("src".to_owned());
        let t2 = target("bash", "make");
        let k1 = CacheKey::from_target(&t1);
        let k2 = CacheKey::from_target(&t2);
        assert_ne!(k1.combined(), k2.combined());
    }

    #[test]
    fn cache_key_from_target_is_stable_across_calls() {
        let t = target("bash", "make all");
        let k1 = CacheKey::from_target(&t);
        let k2 = CacheKey::from_target(&t);
        assert_eq!(k1.combined(), k2.combined());
    }

    #[test]
    fn types_are_send() {
        const fn assert_send<T: Send>() {}
        assert_send::<ContentHash>();
        assert_send::<CacheKey>();
    }
}
