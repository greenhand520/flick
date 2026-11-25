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

/// 面板逻辑宽度（与 `theme::Metrics::default().width` 一致）。
const PANEL_WIDTH: f32 = 666.0;
/// 空态（仅搜索行）高度，与 `theme::Metrics.input_height` 一致。
const PANEL_INPUT_HEIGHT: f32 = 64.0;
/// 面板距主显示器顶部的偏移。
const PANEL_TOP_MARGIN: f32 = 120.0;

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

/// 打开一个面板尺寸的小窗口，返回其类型擦除句柄。
///
/// 窗口与面板卡片等大（透明、无边框、可拖动），位置在主显示器顶部居中。
/// `app_id` 必须与 portal 写出的 `.desktop` 文件名一致，否则 GNOME 会显示 "null"。
fn open_panel_window(
    app: &mut App,
    host_tx: mpsc::UnboundedSender<UiToHost>,
    hide_tx: mpsc::UnboundedSender<()>,
    initial: ThemeMode,
) -> Result<AnyWindowHandle, String> {
    let (x, y) = match app.primary_display() {
        Some(display) => {
            let db = display.bounds();
            let screen_w = f32::from(db.size.width);
            let ox = f32::from(db.origin.x);
            let oy = f32::from(db.origin.y);
            (ox + (screen_w - PANEL_WIDTH) / 2.0, oy + PANEL_TOP_MARGIN)
        }
        None => (0.0, PANEL_TOP_MARGIN),
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
        app_id: Some(flick_domain::APP_ID.into()),
        ..Default::default()
    };

    let handle = app
        .open_window(options, move |window, cx| {
            cx.new(|cx| AppView::new(host_tx, hide_tx, initial, window, cx))
        })
        .map_err(|err| err.to_string())?;
    Ok(handle.into())
}

/// 用指定主题运行 GPUI 面板（阻塞直到应用退出）。
///
/// 应用级事件泵负责窗口生命周期：收到 `ShowSearchPanel` 就创建（或复用）窗口，收到
/// `HideSearchPanel` 就销毁窗口。采用「隐藏即销毁、唤起即新建」模型，使隐藏态不在任务栏
/// 留图标，且新映射的窗口更容易被合成器带到前台（规避后台应用抢焦点问题）。
pub fn run_with_theme(
    host_tx: mpsc::UnboundedSender<UiToHost>,
    host_rx: mpsc::UnboundedReceiver<HostToUi>,
    initial: ThemeMode,
) {
    application()
        .with_assets(gpui_kit::assets::Assets)
        // 隐藏时会销毁唯一的窗口；用 Explicit 防止「最后一个窗口关闭 -> 进程退出」。
        .with_quit_mode(QuitMode::Explicit)
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
            // （≈ UI 逻辑像素的 2 倍）。窗口保持 26，搜索框卡片用一半 13
            // （见 theme::Metrics::radius_panel），两者视觉才一致；同时把 1px 窗口边框设为透明。
            gpui_kit::component::Theme::update(cx, |theme| {
                theme.window_radius = px(26.0);
                theme.colors.window_border.a = 0.0;
            });
            cx.bind_keys([
                KeyBinding::new("escape", view::HidePanel, Some("FlickPanel")),
                KeyBinding::new("up", view::SelectPrev, Some("FlickPanel")),
                KeyBinding::new("down", view::SelectNext, Some("FlickPanel")),
                KeyBinding::new("ctrl-shift-l", view::ToggleThemeAction, Some("FlickPanel")),
            ]);

            let host_tx_open = host_tx.clone();
            let initial_for_open = initial;
            // 视图失焦自发隐藏时，通过该通道请求应用级泵销毁窗口（在窗口更新帧之外执行）。
            let (hide_tx, mut hide_rx) = mpsc::unbounded_channel::<()>();
            let hide_tx_open = hide_tx.clone();

            // 应用级宿主事件泵（与窗口解耦）：窗口销毁后它仍存活，可再创建新窗口。
            cx.spawn(async move |cx: &mut AsyncApp| {
                let mut rx = host_rx;
                // 当前面板窗口句柄（窗口被销毁后会失效，用前需校验）。
                let mut handle: Option<AnyWindowHandle> = None;

                loop {
                    tokio::select! {
                        maybe = rx.recv() => {
                            let Some(event) = maybe else { break; };
                            match event {
                                HostToUi::Shutdown => {
                                    cx.update(|app| app.quit());
                                    break;
                                }
                                HostToUi::ShowSearchPanel { activation_token } => {
                                    let alive = cx.update(|app| {
                                        handle.map_or(false, |h| h.update(app, |_, _, _| {}).is_ok())
                                    });
                                    if !alive {
                                        match cx.update(|app| {
                                            open_panel_window(
                                                app,
                                                host_tx_open.clone(),
                                                hide_tx_open.clone(),
                                                initial_for_open,
                                            )
                                        }) {
                                            Ok(h) => handle = Some(h),
                                            Err(err) => {
                                                eprintln!("flick: failed to open GPUI panel window: {err}");
                                                continue;
                                            }
                                        }
                                    }
                                    if let Some(h) = handle {
                                        let show = HostToUi::ShowSearchPanel { activation_token };
                                        let _ = cx.update(|app| {
                                            let _ = h.update(app, |anyview, window, app2| {
                                                // 新窗口映射后请求激活并聚焦输入框。
                                                window.activate_window();
                                                if let Ok(view) = anyview.downcast::<AppView>() {
                                                    view.update(app2, |v, cx2| {
                                                        v.on_host(show, window, cx2);
                                                        v.focus_input(window, cx2);
                                                    });
                                                }
                                            });
                                        });
                                    }
                                }
                                HostToUi::HideSearchPanel => {
                                    if let Some(h) = handle.take() {
                                        let _ = cx.update(|app| {
                                            let _ = h.update(app, |_, window, _| {
                                                window.remove_window();
                                            });
                                        });
                                    }
                                }
                                other => {
                                    if let Some(h) = handle {
                                        cx.update(|app| {
                                            let _ = h.update(app, |anyview, window, app2| {
                                                if let Ok(view) = anyview.downcast::<AppView>() {
                                                    view.update(app2, |v, cx2| {
                                                        v.on_host(other.clone(), window, cx2);
                                                    });
                                                }
                                            });
                                        });
                                    }
                                }
                            }
                        }
                        _ = hide_rx.recv() => {
                            // 失焦等视图自发隐藏：在窗口更新帧之外统一销毁窗口。
                            if let Some(h) = handle.take() {
                                let _ = cx.update(|app| {
                                    let _ = h.update(app, |_, window, _| {
                                        window.remove_window();
                                    });
                                });
                            }
                        }
                    }
                }

                // 通道关闭（daemon 退出）：结束应用事件循环。
                cx.update(|app| app.quit());
            })
            .detach();
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
