//! 宿主（`core`）命名空间的默认设置 schema。

use flick_domain::settings::{
    SettingDecl, SettingKind, SettingNamespace, SettingSchema, SettingValue,
};

/// `core` schema 当前版本。
pub const CORE_SCHEMA_VERSION: u32 = 1;

/// 构造宿主默认 schema。
pub fn core_schema() -> SettingSchema {
    SettingSchema {
        namespace: SettingNamespace::Core,
        declarations: vec![
            SettingDecl {
                key: "theme.mode".to_string(),
                renamed_from: None,
                label: "主题模式".to_string(),
                description: "界面明暗主题；dark 为深色，light 为浅色。".to_string(),
                kind: SettingKind::Enum {
                    variants: vec!["dark".to_string(), "light".to_string()],
                },
                default: Some(SettingValue::Enum("dark".to_string())),
                secret: false,
            },
            SettingDecl {
                key: "search.result_limit".to_string(),
                renamed_from: None,
                label: "搜索结果上限".to_string(),
                description: "单次展示的结果条数上限（展开更多之外的默认值）。".to_string(),
                kind: SettingKind::Integer { min: 1, max: 100 },
                default: Some(SettingValue::Integer(20)),
                secret: false,
            },
        ],
        schema_version: CORE_SCHEMA_VERSION,
    }
}
