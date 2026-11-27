//! 插件数据目录提供者。
//!
//! 这里只定义能力契约，不解析任何平台路径：具体逻辑根、目录创建与
//! 校验由 `flick-data::DataDirectories` 实现，宿主在启动时注入。
//! 与 [`crate::settings::SettingSecretReader`] 一样放在 domain，
//! 是为了让 [`crate::PluginContext`] 能持有它而不反向依赖存储 crate。

use std::path::PathBuf;

/// 按需解析某个插件的数据目录。
///
/// 方法带 `plugin_id` 是因为实现是宿主级的共享服务；插件侧通过
/// [`crate::PluginContext`] 的 `data_dir()` / `cache_dir()` / `tmp_dir()`
/// 访问，这些方法会自行填入当前调用的插件 id。
pub trait PluginPaths: Send + Sync {
    /// 持久数据目录（逻辑根 data）。
    fn data_dir(&self, plugin_id: &str) -> Option<PathBuf>;

    /// 缓存目录（逻辑根 cache）。
    fn cache_dir(&self, plugin_id: &str) -> Option<PathBuf>;

    /// 临时目录（逻辑根 runtime）。
    fn tmp_dir(&self, plugin_id: &str) -> Option<PathBuf>;
}

/// 测试与占位用的空实现：所有路径查询都返回 `None`。
///
/// 用于让 [`crate::PluginContext`] 在单测里无需真实宿主也能构造；
/// 生产环境应注入 `flick-data::DataDirectories`。
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopPaths;

impl PluginPaths for NoopPaths {
    fn data_dir(&self, _plugin_id: &str) -> Option<PathBuf> {
        None
    }

    fn cache_dir(&self, _plugin_id: &str) -> Option<PathBuf> {
        None
    }

    fn tmp_dir(&self, _plugin_id: &str) -> Option<PathBuf> {
        None
    }
}
