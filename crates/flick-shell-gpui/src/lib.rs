//! `flick-shell-gpui` — 基于 gpui-kit 的 Flick 搜索面板外壳。

pub mod app;
pub mod theme;
pub mod view;

pub use app::{desired_size, AppState, Effect, UiEvent};
pub use flick_application::presenter::{PanelAction, PanelEvent, PanelState, SelectionState};
pub use theme::{Metrics, ThemeMode, UiTheme};
pub use view::AppView;

use flick_domain::{HostToUi, UiToHost};
use gpui_kit::*;
use tokio::sync::mpsc;
use view::{HidePanel, SelectNext, SelectPrev, ToggleThemeAction};

/// 面板逻辑宽度（与 `theme::Metrics::default().width` 一致）。
const PANEL_WIDTH: f32 = 666.0;
/// 空态（仅搜索行）高度，与 `theme::Metrics.input_height` 一致。
const PANEL_INPUT_HEIGHT: f32 = 64.0;

/// 主题持久化配置路径：`$XDG_CONFIG_HOME/flick/shell-gpui.toml`
/// （未设置时回退 `$HOME/.config/flick/shell-gpui.toml`）。
pub(crate) fn theme_config_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| {
                let mut p = std::path::PathBuf::from(home);
                p.push(".config");
                p
            })
        })
        .unwrap_or_default();
    base.join("flick").join("shell-gpui.toml")
}

/// 解析主题配置文件：接受 `mode = "dark"` / `mode = "light"`（允许引号与注释）。
pub(crate) fn parse_theme_toml(text: &str) -> Option<ThemeMode> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() != "mode" {
                continue;
            }
            let value = value.trim().trim_matches('"').trim_matches('\'');
            return match value {
                "dark" => Some(ThemeMode::Dark),
                "light" => Some(ThemeMode::Light),
                _ => None,
            };
        }
    }
    None
}

/// 探测初始主题：持久化文件 > env 覆盖 > 系统颜色方案（gsettings）> 暗色回退。
///
/// 不使用 ashpd 的 portal —— 其 `tokio` feature 与 gpui 的 `async-io` feature 冲突，
/// 会触发 compile_error。这里用 `gsettings` 做尽力而为的探测。
pub fn detect_initial_theme() -> ThemeMode {
    // 1) 用户上次通过 ctrl-shift-l 持久化的选择。
    if let Ok(text) = std::fs::read_to_string(theme_config_path()) {
        if let Some(mode) = parse_theme_toml(&text) {
            return mode;
        }
    }

    // 2) 显式环境变量覆盖。
    if let Ok(v) = std::env::var("FLICK_SHELL_THEME") {
        match v.trim().to_ascii_lowercase().as_str() {
            "light" => return ThemeMode::Light,
            "dark" => return ThemeMode::Dark,
            _ => {}
        }
    }

    // 3) 系统颜色方案。
    if let Ok(output) = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("prefer-light") {
                return ThemeMode::Light;
            }
            if stdout.contains("prefer-dark") {
                return ThemeMode::Dark;
            }
        }
    }

    // 4) 暗色回退。
    ThemeMode::Dark
}

/// 用默认探测的主题运行。
pub fn run(host_tx: mpsc::UnboundedSender<UiToHost>, host_rx: mpsc::UnboundedReceiver<HostToUi>) {
    run_with_theme(host_tx, host_rx, detect_initial_theme());
}

/// 用指定主题运行 GPUI 面板（阻塞直到应用退出）。
pub fn run_with_theme(
    host_tx: mpsc::UnboundedSender<UiToHost>,
    host_rx: mpsc::UnboundedReceiver<HostToUi>,
    initial: ThemeMode,
) {
    application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            init(cx);
            // 让 gpui-kit 组件（Input/Label 等）使用与面板一致的暗/亮主题。
            gpui_kit::component::Theme::change(
                match initial {
                    ThemeMode::Dark => gpui_kit::component::ThemeMode::Dark,
                    ThemeMode::Light => gpui_kit::component::ThemeMode::Light,
                },
                None,
                cx,
            );
            // gpui-kit 的「窗口级」圆角来自 theme.window_radius，且它按物理像素应用
            // （≈ UI 逻辑像素的 2 倍）。因此窗口保持 26，而搜索框卡片用一半 13
            // （见 theme::Metrics::radius_panel），两者视觉才一致；否则四角会露出一圈
            // 半径不匹配的窗口底/边框，看起来就是「搜索框外面的透明边框」。这里同时把
            // 1px 窗口边框设为透明。
            gpui_kit::component::Theme::update(cx, |theme| {
                theme.window_radius = px(26.0);
                theme.colors.window_border.a = 0.0;
            });
            // 小窗口（面板尺寸）：根 surface 不画底色，桌面透出；
            // 圆角与底色由我们自己的面板卡片（view.rs）绘制。
            cx.bind_keys([
                KeyBinding::new("escape", HidePanel, Some("FlickPanel")),
                KeyBinding::new("up", SelectPrev, Some("FlickPanel")),
                KeyBinding::new("down", SelectNext, Some("FlickPanel")),
                KeyBinding::new("ctrl-shift-l", ToggleThemeAction, Some("FlickPanel")),
            ]);
            cx.on_window_closed(|cx, _id| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let rx = host_rx;

            // 面板尺寸的小窗口（Normal、透明、可拖动）：窗口与面板卡片等大，
            // 从根上避免出现整屏半透明底/边框；按住面板边缘可 window.start_window_move 拖动。
            // 位置：主显示器顶部居中，y = 120。结果变化时由 AppView::dispatch 调 window.resize。
            let (x, y) = match cx.primary_display() {
                Some(display) => {
                    let db = display.bounds();
                    let screen_w = f32::from(db.size.width);
                    let ox = f32::from(db.origin.x);
                    let oy = f32::from(db.origin.y);
                    (ox + (screen_w - PANEL_WIDTH) / 2.0, oy + 120.0)
                }
                None => (0.0, 120.0),
            };
            let bounds = Bounds::new(
                point(px(x), px(y)),
                size(px(PANEL_WIDTH), px(PANEL_INPUT_HEIGHT)),
            );
            let options = WindowOptions {
                titlebar: None,
                is_resizable: false,
                is_movable: true,
                kind: WindowKind::Normal,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // 透明窗口：整窗不画底色，桌面透出；面板自身画圆角底色。
                window_background: WindowBackgroundAppearance::Transparent,
                // 客户端装饰：不请求窗口管理器画标题栏/边框，得到干净的无边框浮卡。
                window_decorations: Some(WindowDecorations::Client),
                focus: true,
                app_id: Some("flick".into()),
                ..Default::default()
            };

            cx.open_window(options, move |window, cx| {
                window.activate_window();
                cx.new(|cx| AppView::new(host_tx, rx, initial, false, window, cx))
            })
            .expect("failed to open Flick GPUI panel window");
        });
}

#[cfg(test)]
mod theme_config_tests {
    // 注意：不要 `use super::*`——crate 根的 `use gpui_kit::*` 会把 gpui 的 `test` 宏
    // 带进作用域，令内置 `#[test]` 解析到 gpui 宏而递归爆栈。只显式引入所需项。
    use super::{parse_theme_toml, ThemeMode};

    #[test]
    fn parse_theme_toml_reads_mode() {
        assert_eq!(parse_theme_toml("mode = \"dark\"\n"), Some(ThemeMode::Dark));
        assert_eq!(parse_theme_toml("mode = 'light'"), Some(ThemeMode::Light));
        assert_eq!(
            parse_theme_toml("# 注释\nmode = \"light\""),
            Some(ThemeMode::Light)
        );
        // 旧键名 / 空内容 → 回退 None（再由 detect 继续 env/gsettings/默认）。
        assert_eq!(parse_theme_toml("theme = \"dark\""), None);
        assert_eq!(parse_theme_toml(""), None);
    }
}
