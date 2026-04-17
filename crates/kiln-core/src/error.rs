// Rust guideline compliant 2026-02-21
//! Error types for kiln-core.
//!
//! kiln-core exposes a single situation-specific error struct,
//! [`KilnError`], following M-ERRORS-CANONICAL-STRUCTS. Internally
//! it carries an opaque [`ErrorKind`] enum so callers cannot pattern
//! match on every possible variant — only the documented `is_*`
//! query methods. New variants can therefore be added without a
//! semver break.
//!
//! Errors capture a [`Backtrace`] when they are constructed.
//! Backtraces are only walked when the `RUST_BACKTRACE` environment
//! variable requests them, so the cost of constructing a [`KilnError`]
//! in the happy path is a few CPU instructions.

use std::backtrace::Backtrace;
use std::fmt::{self, Display, Formatter};

/// Errors raised by kiln-core when validating or planning a pipeline.
///
/// Pipelines fail to plan or validate for a small number of structural
/// reasons. Callers that care about the cause can inspect the error
/// via the [`is_cyclic_dependency`], [`is_unknown_target_reference`],
/// and [`is_duplicate_target`] helpers; callers that just want a
/// human-readable message can use [`Display`].
///
/// # Examples
///
/// ```
/// use kiln_core::{Pipeline, Target, TargetId, ShellBlock};
///
/// let mut pipeline = Pipeline::new();
/// let mut a = Target::new(ShellBlock::new("bash", "true"));
/// a.requires.push(TargetId::new("nope"));
/// pipeline.add(TargetId::new("a"), a);
///
/// let err = pipeline.validate().expect_err("references a missing target");
/// assert!(err.is_unknown_target_reference());
/// ```
///
/// [`is_cyclic_dependency`]: KilnError::is_cyclic_dependency
/// [`is_unknown_target_reference`]: KilnError::is_unknown_target_reference
/// [`is_duplicate_target`]: KilnError::is_duplicate_target
#[derive(Debug)]
pub struct KilnError {
    kind: ErrorKind,
    backtrace: Backtrace,
}

impl KilnError {
    /// Returns `true` when this error indicates a cyclic dependency between targets.
    #[must_use]
    pub fn is_cyclic_dependency(&self) -> bool {
        matches!(self.kind, ErrorKind::CyclicDependency { .. })
    }

    /// Returns `true` when a target referenced an ID that is not in the pipeline.
    #[must_use]
    pub fn is_unknown_target_reference(&self) -> bool {
        matches!(self.kind, ErrorKind::UnknownTargetReference { .. })
    }

    /// Returns `true` when two targets in the pipeline share an identifier.
    #[must_use]
    pub fn is_duplicate_target(&self) -> bool {
        matches!(self.kind, ErrorKind::DuplicateTarget { .. })
    }

    /// Returns `true` when a builder reached `.build()` without a required field.
    #[must_use]
    pub fn is_missing_required(&self) -> bool {
        matches!(self.kind, ErrorKind::MissingRequired { .. })
    }

    /// Returns `true` when JSON parsing failed.
    #[must_use]
    pub fn is_json_parse_error(&self) -> bool {
        matches!(self.kind, ErrorKind::JsonParse { .. })
    }

    /// Returns the captured backtrace.
    pub fn backtrace(&self) -> &Backtrace {
        &self.backtrace
    }

    pub(crate) fn cyclic_dependency(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::CyclicDependency {
                message: message.into(),
            },
            backtrace: Backtrace::capture(),
        }
    }

    pub(crate) fn unknown_target_reference(
        target: impl Into<String>,
        referenced_by: impl Into<String>,
        relation: ReferenceRelation,
    ) -> Self {
        Self {
            kind: ErrorKind::UnknownTargetReference {
                target: target.into(),
                referenced_by: referenced_by.into(),
                relation,
            },
            backtrace: Backtrace::capture(),
        }
    }

    pub(crate) fn duplicate_target(target: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::DuplicateTarget {
                target: target.into(),
            },
            backtrace: Backtrace::capture(),
        }
    }

    pub(crate) fn missing_required(field: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::MissingRequired {
                field: field.into(),
            },
            backtrace: Backtrace::capture(),
        }
    }

    pub(crate) fn json_parse(source: serde_json::Error) -> Self {
        Self {
            kind: ErrorKind::JsonParse { source },
            backtrace: Backtrace::capture(),
        }
    }
}

impl Display for KilnError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::CyclicDependency { message } => {
                write!(f, "kiln pipeline has a cyclic dependency: {message}")
            }
            ErrorKind::UnknownTargetReference {
                target,
                referenced_by,
                relation,
            } => {
                write!(
                    f,
                    "target `{referenced_by}` declares a `{relation}` on unknown target `{target}`"
                )
            }
            ErrorKind::DuplicateTarget { target } => {
                write!(f, "target id `{target}` is declared more than once")
            }
            ErrorKind::MissingRequired { field } => {
                write!(f, "builder is missing the required `{field}` field")
            }
            ErrorKind::JsonParse { source } => {
                write!(f, "failed to parse pipeline JSON: {source}")
            }
        }
    }
}

impl std::error::Error for KilnError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::JsonParse { source } => Some(source),
            ErrorKind::CyclicDependency { .. }
            | ErrorKind::UnknownTargetReference { .. }
            | ErrorKind::DuplicateTarget { .. }
            | ErrorKind::MissingRequired { .. } => None,
        }
    }
}

/// Internal enum representing the precise failure mode.
///
/// This is `pub(crate)` on purpose: callers reach the variants through the
/// `is_*` query methods on [`KilnError`], not by pattern matching. Hiding
/// the enum lets us add variants in the future without a semver bump.
#[derive(Debug)]
pub(crate) enum ErrorKind {
    CyclicDependency {
        message: String,
    },
    UnknownTargetReference {
        target: String,
        referenced_by: String,
        relation: ReferenceRelation,
    },
    DuplicateTarget {
        target: String,
    },
    MissingRequired {
        field: String,
    },
    JsonParse {
        source: serde_json::Error,
    },
}

/// What kind of cross-target reference triggered an [`KilnError::unknown_target_reference`] error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReferenceRelation {
    /// A `requires` edge.
    Requires,
    /// A `conflicts` edge.
    Conflicts,
}

impl Display for ReferenceRelation {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Requires => f.write_str("requires"),
            Self::Conflicts => f.write_str("conflicts"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cyclic_dependency_query() {
        let err = KilnError::cyclic_dependency("a -> b -> a");
        assert!(err.is_cyclic_dependency());
        assert!(!err.is_unknown_target_reference());
        assert!(!err.is_duplicate_target());
    }

    #[test]
    fn unknown_reference_query() {
        let err = KilnError::unknown_target_reference("nope", "a", ReferenceRelation::Requires);
        assert!(err.is_unknown_target_reference());
        assert!(!err.is_cyclic_dependency());
    }

    #[test]
    fn duplicate_query() {
        let err = KilnError::duplicate_target("a");
        assert!(err.is_duplicate_target());
    }

    #[test]
    fn display_renders_human_readable_message() {
        let err = KilnError::unknown_target_reference("nope", "a", ReferenceRelation::Requires);
        let rendered = err.to_string();
        assert!(rendered.contains("`a`"), "{rendered}");
        assert!(rendered.contains("`nope`"), "{rendered}");
        assert!(rendered.contains("requires"), "{rendered}");
    }

    #[test]
    fn missing_required_query() {
        let err = KilnError::missing_required("shell");
        assert!(err.is_missing_required());
        assert!(!err.is_cyclic_dependency());
    }

    #[test]
    fn json_parse_query_and_source_chain() {
        let json_err = serde_json::from_str::<i32>("not valid").expect_err("invalid json");
        let err = KilnError::json_parse(json_err);
        assert!(err.is_json_parse_error());

        // The serde_json::Error must be reachable through std::error::Error::source
        // for compatibility with the broader error-handling ecosystem, even
        // though the type itself is not exposed in our public API surface.
        let source = std::error::Error::source(&err);
        assert!(source.is_some());
    }

    #[test]
    fn error_is_send() {
        const fn assert_send<T: Send>() {}
        assert_send::<KilnError>();
    }
}
