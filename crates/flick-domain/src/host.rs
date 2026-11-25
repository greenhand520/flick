//! 宿主基础信息（由平台适配器在启动时采集并注入运行时）。

use serde::{Deserialize, Serialize};

use crate::plugin::{Architecture, OperatingSystem};

/// 宿主环境的基础信息（OS 与架构为强类型，便于与插件 manifest 做平台匹配）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HostInfo {
    pub os: OperatingSystem,
    pub arch: Architecture,
    pub kernel: String,
    pub locale: String,
    pub shell: String,
    pub lan_ip: Option<String>,
}
