// Rust guideline compliant 2026-02-21
//! Shared mutable resources that targets can declare.
//!
//! A [`Resource`] models a piece of state shared between targets — for
//! example a GPU, a network port, or a singleton tool with global state.
//! Targets reference resources via [`ResourceRef`], declaring whether
//! they need exclusive ownership ([`AccessMode::Exclusive`]) or can share
//! the resource with concurrent users ([`AccessMode::Shared`]).
//!
//! The planner uses this information when grouping targets into waves:
//! two targets with overlapping exclusive resource holds will be split
//! into separate waves even when their explicit `requires` edges would
//! allow them to run together.

use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

/// How a target intends to use a [`Resource`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    /// At most one target may hold this resource at a time.
    Exclusive,
    /// Multiple targets may hold this resource concurrently.
    Shared,
}

impl Display for AccessMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exclusive => f.write_str("exclusive"),
            Self::Shared => f.write_str("shared"),
        }
    }
}

/// A globally-unique identifier for a [`Resource`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceId(pub String);

impl ResourceId {
    /// Constructs a `ResourceId` from any string-like value.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl Display for ResourceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl AsRef<str> for ResourceId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for ResourceId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ResourceId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// A target's declared usage of a [`Resource`], pairing the id with an [`AccessMode`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRef {
    /// The resource being referenced.
    pub resource_id: ResourceId,
    /// How this target accesses the resource.
    pub access: AccessMode,
}

impl ResourceRef {
    /// Constructs a new `ResourceRef`.
    #[must_use]
    pub fn new(resource_id: ResourceId, access: AccessMode) -> Self {
        Self {
            resource_id,
            access,
        }
    }
}

/// A shared mutable resource that targets can acquire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    /// Unique identifier.
    pub id: ResourceId,
    /// Default access mode when a [`ResourceRef`] does not specify one.
    pub default_access: AccessMode,
    /// Optional capacity (e.g. max concurrent shared users).
    pub capacity: Option<u32>,
    /// Human-readable description.
    pub description: Option<String>,
    /// Arbitrary metadata for downstream tooling.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Resource {
    /// Constructs a new `Resource` with the given id and default access mode.
    ///
    /// `capacity`, `description`, and `metadata` start empty/`None`.
    #[must_use]
    pub fn new(id: ResourceId, default_access: AccessMode) -> Self {
        Self {
            id,
            default_access,
            capacity: None,
            description: None,
            metadata: HashMap::new(),
        }
    }
}

impl Display for Resource {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "resource {} ({})", self.id, self.default_access)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_mode_round_trip() {
        let json = serde_json::to_string(&AccessMode::Exclusive).unwrap();
        assert_eq!(json, "\"exclusive\"");
        let parsed: AccessMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, AccessMode::Exclusive);
    }

    #[test]
    fn resource_id_conversions() {
        let from_str: ResourceId = "gpu".into();
        let from_string: ResourceId = String::from("gpu").into();
        assert_eq!(from_str, from_string);
        assert_eq!(from_str.as_ref(), "gpu");
        assert_eq!(from_str.to_string(), "gpu");
    }

    #[test]
    fn resource_default_fields() {
        let r = Resource::new(ResourceId::new("network"), AccessMode::Shared);
        assert!(r.capacity.is_none());
        assert!(r.description.is_none());
        assert!(r.metadata.is_empty());
        assert_eq!(r.to_string(), "resource network (shared)");
    }

    #[test]
    fn resource_round_trips_through_json() {
        let r = Resource::new(ResourceId::new("gpu"), AccessMode::Exclusive);
        let json = serde_json::to_string(&r).unwrap();
        let parsed: Resource = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, r);
    }

    #[test]
    fn types_are_send() {
        const fn assert_send<T: Send>() {}
        assert_send::<Resource>();
        assert_send::<ResourceRef>();
        assert_send::<ResourceId>();
        assert_send::<AccessMode>();
    }
}
