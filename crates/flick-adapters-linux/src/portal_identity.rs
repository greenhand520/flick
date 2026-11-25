//! Portal host-app 身份。
//!
//! GNOME/Mutter 的 `org.freedesktop.portal.GlobalShortcuts` 要求调用进程先通过
//! `org.freedesktop.host.portal.Registry.Register` 绑定一个 app id；并且该 app id
//! 必须对应一个 GIO 能解析的 `<app_id>.desktop`（其 `Exec` 指向一个**存在**的可执行
//! 文件），否则 portal 会返回：
//! - `Could not register app ID: App info not found for '<app_id>'`
//! - `org.freedesktop.portal.Error.NotAllowed: An app id is required`
//!
//! `flick-daemon` 作为主机（非沙箱）应用，既没有随包安装的 `.desktop`，也不调用
//! `Register`，因此在 GNOME 上 `CreateSession` 直接失败、快捷键永远不会注册。
//! 本模块在启动时写出（或刷新）用户级 `.desktop` 入口，并完成 `Register`。

use std::{
    env, fs, io,
    path::{Path, PathBuf},
    str::FromStr,
};

use ashpd::AppID;

use crate::session::ShortcutError;

/// 固定的 app id；必须与写出的 `<app_id>.desktop` 文件名一致。
/// 直接使用代码常量（与 `flick_domain::APP_ID` 同源），不读取环境变量。
pub const DEFAULT_APP_ID: &str = flick_domain::APP_ID;

/// 返回固定的 app id（直接使用代码中的 [`DEFAULT_APP_ID`]，不读取环境变量）。
pub fn resolve_app_id() -> String {
    DEFAULT_APP_ID.to_owned()
}

fn data_home() -> PathBuf {
    if let Some(value) = env::var_os("XDG_DATA_HOME") {
        let path = PathBuf::from(value);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }
    PathBuf::from(env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".local/share")
}

/// 安装到 hicolor 主题的图标尺寸（像素）。
pub const ICON_SIZES: &[u32] = &[16, 22, 24, 32, 48, 64, 96, 128, 256, 512];

/// 把内置 SVG 光栅化为多尺寸 PNG，写入
/// `$XDG_DATA_HOME/icons/hicolor/<N>x<N>/apps/<app_id>.png`。
///
/// 返回写出的文件路径列表；任一尺寸失败即返回错误（由调用方告警后继续）。
pub fn ensure_icons(app_id: &str) -> io::Result<Vec<PathBuf>> {
    ensure_icons_into(&data_home(), app_id)
}

fn ensure_icons_into(data_home: &Path, app_id: &str) -> io::Result<Vec<PathBuf>> {
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_str(crate::flick_icon::FLICK_ICON_SVG, &options)
        .map_err(|error| io::Error::other(format!("解析内置 Flick 图标 SVG 失败: {error}")))?;
    let source = tree.size();
    if source.width() <= 0.0 || source.height() <= 0.0 {
        return Err(io::Error::other("内置 Flick 图标尺寸无效"));
    }

    let hicolor = data_home.join("icons").join("hicolor");
    let mut written = Vec::with_capacity(ICON_SIZES.len());

    for &size in ICON_SIZES {
        let dir = hicolor.join(format!("{size}x{size}")).join("apps");
        fs::create_dir_all(&dir)?;

        let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)
            .ok_or_else(|| io::Error::other(format!("创建 {size}x{size} 图标位图失败")))?;
        let scale_x = size as f32 / source.width();
        let scale_y = size as f32 / source.height();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale_x, scale_y),
            &mut pixmap.as_mut(),
        );

        let path = dir.join(format!("{app_id}.png"));
        pixmap.save_png(&path).map_err(|error| {
            io::Error::other(format!("写出 {} 失败: {error}", path.display()))
        })?;
        written.push(path);
    }

    Ok(written)
}

/// 写出（或刷新）`<app_id>.desktop`，`Exec` 指向当前可执行文件。
///
/// 使用运行时绝对路径（`std::env::current_exe`）而不是依赖 PATH，这样即使
/// 二进制被移动，GIO 也能解析该入口，`Register` 才不会报 "App info not found"。
pub fn ensure_desktop_entry(app_id: &str) -> std::io::Result<PathBuf> {
    let dir = data_home().join("applications");
    fs::create_dir_all(&dir)?;
    let exec = env::current_exe()?.display().to_string();
    let content = format!(
        "[Desktop Entry]\nType=Application\nName=Flick\nComment=Flick launcher daemon\n\
         Icon={app_id}\nExec={exec} --foreground\nTerminal=false\nNoDisplay=true\nCategories=Utility;\n"
    );
    let path = dir.join(format!("{app_id}.desktop"));
    fs::write(&path, content)?;
    Ok(path)
}

/// 写出图标与 `.desktop`，返回解析后的 app id 字符串。
fn prepare_identity_files() -> Result<String, ShortcutError> {
    let app_id_str = resolve_app_id();

    match ensure_icons(&app_id_str) {
        Ok(paths) => tracing::info!(count = paths.len(), "已安装 Flick 图标到 hicolor 主题"),
        Err(error) => tracing::warn!(%error, "安装 Flick 图标失败，将使用系统默认图标"),
    }

    if let Err(error) = ensure_desktop_entry(&app_id_str) {
        tracing::warn!(%error, app_id = %app_id_str, "无法写入 portal app-id 的 .desktop 入口");
    }

    Ok(app_id_str)
}

fn parse_app_id(app_id_str: &str) -> Result<AppID, ShortcutError> {
    AppID::from_str(app_id_str).map_err(|error| ShortcutError::Internal {
        message: format!("无效的 portal app id: {error}"),
    })
}

/// 把注册结果规范化：`already associated` 视为成功。
fn map_register_result(
    app_id_str: String,
    result: Result<(), ashpd::Error>,
) -> Result<(), ShortcutError> {
    match result {
        Ok(()) => Ok(()),
        // 该 D-Bus 连接此前已经注册过 app id（例如启动竞态下的重试、或同一连接的
        // 重复初始化）。xdg-desktop-portal 对此返回 "Connection already associated
        // with an application ID"，但这等价于「app id 已就绪」，属于良性情况，
        // 不应视为失败。
        Err(error) if is_already_associated(&error) => {
            tracing::debug!(
                app_id = %app_id_str,
                "portal 主机应用连接已关联 app id，跳过重复注册"
            );
            Ok(())
        }
        Err(error) => Err(ShortcutError::Internal {
            message: format!("register_host_app 失败: {error}"),
        }),
    }
}

/// 在 ashpd 的全局共享连接上完成 host-app 注册（沿用旧行为，供独立探针使用）。
///
/// 注意：daemon 不应使用本函数——进程内其它组件（gpui 经 ashpd 读取 portal
/// Settings 等）可能已经先初始化了该全局连接，使其被 portal 以「空 app id」关联，
/// 从而令后续 `CreateSession` 报 `An app id is required`。daemon 必须改用
/// [`register_portal_app_id_with_connection`] 绑定一条专用连接。
pub async fn register_portal_app_id() -> Result<(), ShortcutError> {
    let app_id_str = prepare_identity_files()?;
    let app_id = parse_app_id(&app_id_str)?;
    map_register_result(app_id_str, ashpd::register_host_app(app_id).await)
}

/// 在**调用方指定的独立 D-Bus 连接**上完成 host-app 注册。
///
/// 保证「注册」与随后的 `GlobalShortcuts::with_connection(同一连接)`/
/// `CreateSession` 落在同一条、且未被进程内其它 ashpd 使用者污染的连接上。
pub async fn register_portal_app_id_with_connection(
    connection: zbus::Connection,
) -> Result<(), ShortcutError> {
    let app_id_str = prepare_identity_files()?;
    let app_id = parse_app_id(&app_id_str)?;
    map_register_result(
        app_id_str,
        ashpd::register_host_app_with_connection(connection, app_id).await,
    )
}

/// 判断 ashpd 错误是否为「连接已关联 app id」（良性、可忽略）。
fn is_already_associated(error: &ashpd::Error) -> bool {
    error
        .to_string()
        .contains("Connection already associated with an application ID")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_app_id_is_reverse_dns() {
        assert!(DEFAULT_APP_ID.contains('.'));
        assert!(AppID::from_str(DEFAULT_APP_ID).is_ok());
    }

    #[test]
    fn embedded_icon_parses() {
        let tree = resvg::usvg::Tree::from_str(
            crate::flick_icon::FLICK_ICON_SVG,
            &resvg::usvg::Options::default(),
        )
        .expect("内置 SVG 应可解析");
        assert!(tree.size().width() > 0.0);
        assert_eq!(tree.size().width(), tree.size().height());
    }

    #[test]
    fn ensure_icons_writes_pngs_for_all_sizes() {
        let base = std::env::temp_dir().join(format!("flick-icon-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);

        let paths = ensure_icons_into(&base, "io.github.flick.Flick").expect("生成图标应成功");
        assert_eq!(paths.len(), ICON_SIZES.len());

        const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        for (&size, path) in ICON_SIZES.iter().zip(paths.iter()) {
            assert!(path.exists(), "缺失 {}", path.display());
            assert_eq!(
                path,
                &base
                    .join("icons/hicolor")
                    .join(format!("{size}x{size}"))
                    .join("apps")
                    .join("io.github.flick.Flick.png")
            );
            let bytes = fs::read(path).expect("读取 PNG");
            assert!(bytes.len() > 8);
            assert_eq!(&bytes[..8], &PNG_SIG, "{} 不是有效 PNG", path.display());

            // 背景必须透明：四角 alpha=0，中心不透明。
            let png = resvg::tiny_skia::Pixmap::load_png(path).expect("解码 PNG");
            assert_eq!(
                png.pixel(0, 0).expect("corner pixel").alpha(),
                0,
                "{} 左上角应透明",
                path.display()
            );
            assert!(
                png.pixel(size / 2, size / 2)
                    .expect("center pixel")
                    .alpha()
                    > 0,
                "{} 中心应不透明",
                path.display()
            );
        }

        let _ = fs::remove_dir_all(&base);
    }
}
