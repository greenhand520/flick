//! 四个逻辑根的解析与每插件目录的创建。
//!
//! 平台差异由 `directories::ProjectDirs` 统一处理（config/data/cache）；
//! runtime 根不在 `ProjectDirs` 覆盖范围内，单独读 `XDG_RUNTIME_DIR` 并
//! 回退系统临时目录。除本模块外，任何 crate 不得自行拼接这些路径（C17）。

use std::fs;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::error::DataError;

/// 项目目录使用的应用名（与 `flick-domain::APP_ID` 的解码形式一致）。
const APP_NAME: &str = "flick";

/// 四个逻辑根：配置、数据、缓存、运行时临时文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDirectories {
    config: PathBuf,
    data: PathBuf,
    cache: PathBuf,
    runtime: PathBuf,
}

impl DataDirectories {
    /// 按平台约定发现四个逻辑根（不创建目录）。
    pub fn discover() -> Result<Self, DataError> {
        let project = ProjectDirs::from("", "", APP_NAME)
            .ok_or_else(|| DataError::NoProjectDirs)?;
        let runtime = runtime_root();
        Ok(Self {
            config: project.config_dir().to_path_buf(),
            data: project.data_dir().to_path_buf(),
            cache: project.cache_dir().to_path_buf(),
            runtime,
        })
    }

    /// 用显式路径构造（测试与容器场景）。
    pub fn with_roots(
        config: impl Into<PathBuf>,
        data: impl Into<PathBuf>,
        cache: impl Into<PathBuf>,
        runtime: impl Into<PathBuf>,
    ) -> Self {
        Self {
            config: config.into(),
            data: data.into(),
            cache: cache.into(),
            runtime: runtime.into(),
        }
    }

    /// 配置根。
    pub fn config_root(&self) -> &Path {
        &self.config
    }

    /// 数据根。
    pub fn data_root(&self) -> &Path {
        &self.data
    }

    /// 缓存根。
    pub fn cache_root(&self) -> &Path {
        &self.cache
    }

    /// 运行时临时根。
    pub fn runtime_root(&self) -> &Path {
        &self.runtime
    }

    /// 权威配置数据库路径：`<config>/core.sqlite3`。
    pub fn config_db(&self) -> PathBuf {
        self.config.join("core.sqlite3")
    }

    /// 确保配置根存在（`0700`）。
    pub fn ensure_config_root(&self) -> Result<PathBuf, DataError> {
        ensure_dir(&self.config)?;
        Ok(self.config.clone())
    }

    /// 插件数据目录：`<data>/plugins/<plugin_id>`，按需创建（`0700`）。
    pub fn plugin_dir(&self, plugin_id: &str) -> Result<PathBuf, DataError> {
        let path = self.data.join("plugins").join(validate_plugin_id(plugin_id)?);
        ensure_dir(&path)?;
        Ok(path)
    }

    /// 插件缓存目录：`<cache>/plugins/<plugin_id>`，按需创建（`0700`）。
    pub fn plugin_cache_dir(&self, plugin_id: &str) -> Result<PathBuf, DataError> {
        let path = self.cache.join("plugins").join(validate_plugin_id(plugin_id)?);
        ensure_dir(&path)?;
        Ok(path)
    }

    /// 插件临时目录：`<runtime>/plugins/<plugin_id>`，按需创建（`0700`）。
    pub fn plugin_tmp_dir(&self, plugin_id: &str) -> Result<PathBuf, DataError> {
        let path = self.runtime.join("plugins").join(validate_plugin_id(plugin_id)?);
        ensure_dir(&path)?;
        Ok(path)
    }
}

/// 把目录解析能力暴露给插件上下文。
///
/// 这里直接复用 [`DataDirectories`] 的校验与创建逻辑；错误被降级为 `None`
/// （与 `SettingSecretReader::secret` 一致），日志由调用方按需补充。
impl flick_domain::PluginPaths for DataDirectories {
    fn data_dir(&self, plugin_id: &str) -> Option<PathBuf> {
        self.plugin_dir(plugin_id).ok()
    }

    fn cache_dir(&self, plugin_id: &str) -> Option<PathBuf> {
        self.plugin_cache_dir(plugin_id).ok()
    }

    fn tmp_dir(&self, plugin_id: &str) -> Option<PathBuf> {
        self.plugin_tmp_dir(plugin_id).ok()
    }
}

/// 校验插件 id：复用 domain 的字符集规则，并额外拒绝路径分隔符。
fn validate_plugin_id(plugin_id: &str) -> Result<&str, DataError> {
    if flick_domain::settings::is_valid_plugin_id(plugin_id)
        && !plugin_id.contains('/')
        && !plugin_id.contains('\\')
    {
        Ok(plugin_id)
    } else {
        Err(DataError::InvalidPluginId(plugin_id.to_string()))
    }
}

/// 创建目录（含父目录），Unix 下权限 `0700`，并拒绝符号链接目标。
fn ensure_dir(path: &Path) -> Result<(), DataError> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(DataError::Symlink {
                path: path.display().to_string(),
            });
        }
        if meta.is_dir() {
            return Ok(());
        }
    }
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// runtime 根：优先 `XDG_RUNTIME_DIR`，否则回退系统临时目录下的 `flick`。
fn runtime_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(dir);
        if !path.as_os_str().is_empty() {
            return path.join(APP_NAME);
        }
    }
    std::env::temp_dir().join(APP_NAME)
}
