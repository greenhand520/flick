//! 搜索查询类型。

use crate::action::InputType;
use serde::{Deserialize, Serialize};

/// 用户提交的搜索查询。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub raw_text: String,
    pub input_type: InputType,
}

impl SearchQuery {
    pub fn plain_text(text: impl Into<String>) -> Self {
        Self {
            raw_text: text.into(),
            input_type: InputType::PlainText,
        }
    }
}
