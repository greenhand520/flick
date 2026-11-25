//! 宿主基础信息（由平台适配器在启动时采集并注入运行时）。

use serde::{Deserialize, Serialize};

/// 宿主环境的基础信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HostInfo {
    pub os: String,
    pub kernel: String,
    pub locale: String,
    pub shell: String,
    pub lan_ip: Option<String>,
}
