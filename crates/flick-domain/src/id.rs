//! Identity types for invocations, actions, and query generations.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

/// Unique identifier for a single Action invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InvocationId(Uuid);

impl InvocationId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for InvocationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for InvocationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Monotonically increasing generation for query results.
///
/// Used to ensure stale results never overwrite current ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct QueryGeneration(u64);

impl QueryGeneration {
    pub const INITIAL: Self = Self(1);

    pub fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    pub fn value(self) -> u64 {
        self.0
    }
}

impl Default for QueryGeneration {
    fn default() -> Self {
        Self::INITIAL
    }
}

impl fmt::Display for QueryGeneration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Identifier for a registered Action.
///
/// Must be non-blank. Format: `flick.builtin.<name>.v<N>.<operation>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActionId(Arc<str>);

impl ActionId {
    /// Create a new `ActionId`, rejecting blank values.
    pub fn new(id: impl Into<String>) -> Result<Self, crate::ActionError> {
        let s: String = id.into();
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(crate::ActionError::InvalidInput {
                message: "ActionId must not be blank".into(),
            });
        }
        Ok(Self(Arc::from(trimmed)))
    }

    /// Create an `ActionId` without validation (for internal/builtin use).
    pub fn from_static(id: &'static str) -> Self {
        Self(Arc::from(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_id_rejects_blank() {
        assert!(ActionId::new("").is_err());
        assert!(ActionId::new("   ").is_err());
        assert!(ActionId::new("valid.id").is_ok());
    }

    #[test]
    fn query_generation_is_monotonically_increasing() {
        let g1 = QueryGeneration::INITIAL;
        let g2 = g1.next();
        let g3 = g2.next();
        assert!(g1 < g2);
        assert!(g2 < g3);
    }

    #[test]
    fn invocation_ids_are_unique() {
        let a = InvocationId::new();
        let b = InvocationId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn round_trip_json() {
        let id = ActionId::from_static("test.action");
        let json = serde_json::to_string(&id).unwrap();
        let back: ActionId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
