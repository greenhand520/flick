//! 调用、动作和查询代次的标识符类型。

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

/// 单次动作调用的唯一标识符。
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

/// 单个 Surface 交互会话的唯一标识符。
///
/// 与 [`InvocationId`] 同风格：`Uuid` newtype，支持 `new()/Default/Display` 与 serde。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SurfaceSessionId(Uuid);

impl SurfaceSessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SurfaceSessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SurfaceSessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 查询结果单调递增的代次。
///
/// 用于确保过期的结果永远不会覆盖当前结果。
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

/// 已注册动作的标识符。
///
/// 不得为空白。格式：`flick.builtin.<name>.v<N>.<operation>`。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActionId(Arc<str>);

impl ActionId {
    /// 创建一个新的 `ActionId`，拒绝空白值。
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

    /// 不经验证创建 `ActionId`（供内部/内置使用）。
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
    fn surface_session_ids_are_unique_and_round_trip() {
        let a = SurfaceSessionId::new();
        let b = SurfaceSessionId::new();
        assert_ne!(a, b);
        let json = serde_json::to_string(&a).unwrap();
        let back: SurfaceSessionId = serde_json::from_str(&json).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn round_trip_json() {
        let id = ActionId::from_static("test.action");
        let json = serde_json::to_string(&id).unwrap();
        let back: ActionId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
