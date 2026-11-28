//! `flick-shell-gpui` — 基于 gpui-kit 的 Flick 搜索面板外壳。

pub mod app;
pub(crate) mod clipboard_watch;
pub mod detached;
pub(crate) mod input_capture;
pub mod theme;
pub mod view;

pub use app::{AppState, Effect, UiEvent, desired_size};
pub use detached::DetachedSurfaceView;
pub use flick_application::presenter::{PanelAction, PanelEvent, PanelState, SelectionState};
pub use theme::{Metrics, ThemeMode, UiTheme};
pub use view::AppView;

use crate::app::DetachPayload;
use flick_domain::{HostToUi, SurfaceSessionId, UiToHost};
use gpui_kit::layer_shell::{
    Anchor, KeyboardInteractivity, Layer, LayerShellNotSupportedError, LayerShellOptions,
};
use gpui_kit::*;
use tokio::sync::mpsc;

/// 在应用线程上销毁窗口（忽略句柄已失效的情况）。
fn close_window(cx: &mut AsyncApp, handle: AnyWindowHandle) {
    let _ = cx.update(|app| {
        let _ = handle.update(app, |_, window, _| window.remove_window());
    });
}

/// 向面板视图投递一条宿主事件（面板已关闭或视图类型不符时静默忽略）。
fn dispatch_panel_event(cx: &mut AsyncApp, handle: AnyWindowHandle, ev: HostToUi) {
    let _ = cx.update(|app| {
        let _ = handle.update(app, |anyview, window, app2| {
            if let Ok(view) = anyview.downcast::<AppView>() {
                view.update(app2, |v, cx2| v.on_host(ev, window, cx2));
            }
        });
    });
}

/// 空态（仅搜索行）高度，与 `theme::Metrics.input_height` 一致。
pub(crate) const PANEL_INPUT_HEIGHT: f32 = 64.0;
/// 面板卡片顶部距屏幕顶部的基准偏移（逻辑像素）。
/// 增大此值 = 搜索框整体下移；它不影响搜索行高度 `theme::Metrics::input_height`。
pub(crate) const PANEL_TOP_MARGIN: f32 = 300.0;

/// 解析一个 fontconfig 族的首选名称（`fc-match` 输出的第一个族名）。
fn fc_family(query: &str) -> Option<String> {
    let output = std::process::Command::new("fc-match")
        .args(["-f", "%{family}", query])
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    text.split(',')
        .next()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

/// 该族是否**精确安装**（`fc-match` 未回退到别的族名）。
fn font_installed(family: &str) -> bool {
    fc_family(family)
        .map(|resolved| resolved.eq_ignore_ascii_case(family))
        .unwrap_or(false)
}

fn resolve_font(candidates: &[&str], generic: &str) -> String {
    candidates
        .iter()
        .find(|family| font_installed(family))
        .map(|family| (*family).to_string())
        .or_else(|| fc_family(generic))
        .unwrap_or_else(|| generic.to_string())
}

/// UI 无衬线字体族。gpui-kit 默认的 `.SystemUIFont` 是 macOS 别名，Linux 上不存在，
/// 会触发 `gpui::text_system: failed to load font ...: no font found` 告警；这里探测
/// 一个真实安装的族（优先含 CJK 的 Noto Sans CJK，避免中文缺字）。
pub fn ui_font_family() -> &'static str {
    static FONT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FONT.get_or_init(|| {
        resolve_font(
            &[
                "Noto Sans CJK SC",
                "Noto Sans",
                "Adwaita Sans",
                "DejaVu Sans",
                "Liberation Sans",
            ],
            "sans-serif",
        )
    })
}

/// UI 等宽字体族（同样避免 `.SystemUIFont` 之类不存在的别名）。
pub fn ui_mono_font_family() -> &'static str {
    static FONT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FONT.get_or_init(|| {
        resolve_font(
            &[
                "Noto Sans Mono CJK SC",
                "Noto Sans Mono",
                "Adwaita Mono",
                "DejaVu Sans Mono",
                "Liberation Mono",
            ],
            "monospace",
        )
    })
}
/// 探测初始主题：env 覆盖 > 系统颜色方案（gsettings）> 暗色回退。
///
/// 主题的权威来源是设置数据库（`core.theme.mode`，由守护进程经
/// [`ShellConfig`] 注入）；本函数只在该来源不可用时兜底。
///
/// 不使用 ashpd 的 portal —— 其 `tokio` feature 与 gpui 的 `async-io` feature 冲突，
/// 会触发 compile_error。这里用 `gsettings` 做尽力而为的探测。
pub fn detect_initial_theme() -> ThemeMode {
    // 1) 显式环境变量覆盖。
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

/// 打开面板窗口：**固定全屏透明 overlay**，内部只绘制顶部面板卡片。
///
/// 参考 `awari` 的做法：窗口尺寸固定为显示器大小，结果增减只改变内部卡片高度，
/// 从不 `window.resize`（Wayland 下 resize 会引发输入法/键盘焦点抖动，表现为
/// Delete/Backspace 误隐藏）。Wayland 优先用 layer-shell overlay；不支持时回退
/// 普通全屏透明窗口（仅 Wayland/X11 后端，不做特殊平台补丁）。
fn open_panel_window(
    app: &mut App,
    host_tx: mpsc::UnboundedSender<UiToHost>,
    hide_tx: mpsc::UnboundedSender<()>,
    detach_tx: mpsc::UnboundedSender<(DetachPayload, ThemeMode)>,
    theme_tx: mpsc::UnboundedSender<ThemeMode>,
    clipboard_watch: clipboard_watch::ClipboardWatch,
    initial: ThemeMode,
    result_limit: usize,
) -> Result<AnyWindowHandle, String> {
    // GNOME/Wayland 下 `primary_display()` 在窗口创建前可能返回 None；此时退回
    // `displays()` 的第一块显示器，否则窗口会退化成 1280×800，四周露出一大片
    // 透明区域（用户看到的“大的透明背景/边框”）。
    let display = app
        .primary_display()
        .or_else(|| app.displays().into_iter().next());
    let bounds = match display {
        Some(display) => display.bounds(),
        None => Bounds::new(point(px(0.0), px(0.0)), size(px(1280.0), px(800.0))),
    };
    tracing::debug!(
        x = f32::from(bounds.origin.x),
        y = f32::from(bounds.origin.y),
        width = f32::from(bounds.size.width),
        height = f32::from(bounds.size.height),
        displays = app.displays().len(),
        "打开搜索面板窗口"
    );

    let layer_options = LayerShellOptions {
        namespace: "flick:launcher".into(),
        layer: Layer::Overlay,
        anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
        exclusive_zone: Some(px(0.0)),
        exclusive_edge: None,
        margin: None,
        keyboard_interactivity: KeyboardInteractivity::Exclusive,
    };
    let options = WindowOptions {
        titlebar: None,
        is_resizable: false,
        is_movable: false,
        kind: WindowKind::LayerShell(layer_options),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // 整窗透明：只有面板卡片绘制底色；无窗口装饰/边框/阴影。
        window_background: WindowBackgroundAppearance::Transparent,
        window_decorations: Some(WindowDecorations::Client),
        focus: true,
        app_id: Some(flick_domain::APP_ID.into()),
        ..Default::default()
    };

    let host_tx1 = host_tx.clone();
    let hide_tx1 = hide_tx.clone();
    let detach_tx1 = detach_tx.clone();
    let theme_tx1 = theme_tx.clone();
    let clipboard_watch1 = clipboard_watch.clone();
    let result = app.open_window(options, move |window, cx| {
        cx.new(|cx| {
            AppView::new(
                host_tx1,
                hide_tx1,
                detach_tx1,
                theme_tx1,
                clipboard_watch1,
                initial,
                result_limit,
                window,
                cx,
            )
        })
    });

    // GNOME/Mutter 等无 `wlr-layer-shell` 的合成器：回退到普通全屏透明窗口。
    let result = match result {
        Ok(handle) => return Ok(handle.into()),
        Err(err) if err.downcast_ref::<LayerShellNotSupportedError>().is_some() => {
            tracing::warn!(
                error = %err,
                "wlr-layer-shell is not supported by the compositor; falling back to a normal maximized transparent window"
            );
            let fallback = WindowOptions {
                titlebar: None,
                is_resizable: false,
                is_movable: false,
                kind: WindowKind::Normal,
                // 直接以「最大化」状态建窗：
                // - `Display::bounds()` 在分数缩放下会算小（2880×1800@1.6667 得到
                //   1440×900），只有合成器下发的 configure 尺寸才准确，最大化/全屏
                //   才会触发这种 configure。
                // - 不能请求全屏：Mutter 会把全屏 toplevel 直接扫描输出、跳过 alpha
                //   混合，透明区域变黑并盖住壁纸。
                // - 最大化窗口仍由合成器混合 alpha，且最大化/平铺窗口不绘制窗口
                //   阴影，正好避免「阴影边框」。
                window_bounds: Some(fallback_panel_window_bounds(bounds)),
                window_background: WindowBackgroundAppearance::Transparent,
                window_decorations: Some(WindowDecorations::Client),
                focus: true,
                app_id: Some(flick_domain::APP_ID.into()),
                ..Default::default()
            };
            app.open_window(fallback, move |window, cx| {
                cx.new(|cx| {
                    AppView::new(
                        host_tx,
                        hide_tx,
                        detach_tx,
                        theme_tx,
                        clipboard_watch,
                        initial,
                        result_limit,
                        window,
                        cx,
                    )
                })
            })
        }
        Err(err) => Err(err),
    };
    match result {
        Ok(handle) => Ok(handle.into()),
        Err(err) => Err(err.to_string()),
    }
}

/// 无 layer-shell 回退窗口的尺寸：优先「鼠标所在屏」的精确逻辑矩形，
/// 探测失败时退化为最大化（落在工作区）。探测要读 Mutter `DisplayConfig`
/// 与 XWayland 指针，失败一律回退，不影响开窗。
fn fallback_panel_window_bounds(bounds: Bounds<Pixels>) -> WindowBounds {
    match flick_adapters_linux::screen::pointer_monitor() {
        Some(monitor) => {
            tracing::debug!(
                name = %monitor.name,
                x = monitor.x,
                y = monitor.y,
                width = monitor.width,
                height = monitor.height,
                "面板窗口按鼠标所在屏建窗"
            );
            WindowBounds::Windowed(Bounds::new(
                point(px(monitor.x as f32), px(monitor.y as f32)),
                size(px(monitor.width as f32), px(monitor.height as f32)),
            ))
        }
        None => {
            tracing::debug!("显示器探测不可用，回退最大化建窗");
            WindowBounds::Maximized(bounds)
        }
    }
}

/// 打开一个独立 Surface 窗口（快照渲染 + 会话交互）。
fn open_detached_window(
    app: &mut App,
    payload: DetachPayload,
    theme: ThemeMode,
    close_tx: mpsc::UnboundedSender<AnyWindowHandle>,
    host_tx: mpsc::UnboundedSender<UiToHost>,
    index: usize,
) -> Result<AnyWindowHandle, String> {
    let DetachPayload {
        session_id,
        content,
        needs_search,
    } = payload;
    // 位置与尺寸统一由纯函数 `detached_bounds` 计算：基准居中 + 级联，并夹进显示器
    // （越界时回落到规范位置），避免高窗口底边/级联把窗推到屏幕外。
    let (x, y, w, h) = match app.primary_display() {
        Some(display) => {
            let db = display.bounds();
            detached::detached_bounds(
                &content,
                theme,
                (
                    f32::from(db.origin.x),
                    f32::from(db.origin.y),
                    f32::from(db.size.width),
                    f32::from(db.size.height),
                ),
                index,
                needs_search,
            )
        }
        None => {
            // 无显示器信息：退化为默认位置 + 内容尺寸（不做 clamp）。
            let style = detached::surface_style(theme);
            let (w, h) = detached::detached_size(&content, &style, needs_search);
            (0.0, PANEL_TOP_MARGIN + PANEL_INPUT_HEIGHT + 24.0, w, h)
        }
    };
    let bounds = Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
    let options = WindowOptions {
        titlebar: None,
        is_resizable: true,
        is_movable: true,
        kind: WindowKind::Normal,
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_background: WindowBackgroundAppearance::Transparent,
        window_decorations: Some(WindowDecorations::Client),
        focus: true,
        app_id: Some(flick_domain::APP_ID.into()),
        ..Default::default()
    };
    let title = content.title.clone().unwrap_or_else(|| "Flick".to_string());
    let handle = app
        .open_window(options, move |window, cx| {
            window.set_window_title(&title);
            cx.new(|cx| {
                DetachedSurfaceView::new(
                    content,
                    session_id,
                    needs_search,
                    theme,
                    close_tx,
                    host_tx,
                    window,
                    cx,
                )
            })
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
    run_with_config(
        host_tx,
        host_rx,
        ShellConfig {
            theme: initial,
            ..ShellConfig::default()
        },
    );
}

/// 外壳启动配置（由守护进程从设置系统读出后传入）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellConfig {
    /// 初始主题模式。
    pub theme: ThemeMode,
    /// 结果区一次展示的最大条数（`core.search.result_limit`）。
    pub result_limit: usize,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Dark,
            result_limit: flick_application::presenter::DEFAULT_RESULT_LIMIT,
        }
    }
}

/// 用完整配置运行 GPUI 面板（阻塞直到应用退出）。
pub fn run_with_config(
    host_tx: mpsc::UnboundedSender<UiToHost>,
    host_rx: mpsc::UnboundedReceiver<HostToUi>,
    config: ShellConfig,
) {
    let initial = config.theme;
    let result_limit = config.result_limit;
    application()
        .with_assets(assets::Assets)
        // 隐藏时会销毁唯一的窗口；用 Explicit 防止「最后一个窗口关闭 -> 进程退出」。
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx| {
            init(cx);
            // 应用级启动剪贴板轮询；窗口销毁/重建时通过克隆句柄继续复用同一观察状态。
            let clipboard_watch = clipboard_watch::ClipboardWatch::start(cx);
            // 让 gpui-kit 组件（Input/Label 等）使用与面板一致的暗/亮主题。
            component::Theme::change(
                match initial {
                    ThemeMode::Dark => component::ThemeMode::Dark,
                    ThemeMode::Light => component::ThemeMode::Light,
                },
                None,
                cx,
            );
            // gpui-kit 的「窗口级」圆角来自 theme.window_radius，且它按物理像素应用
            // （≈ UI 逻辑像素的 2 倍）。窗口保持 26，搜索框卡片用一半 13
            // （见 theme::Metrics::radius_panel），两者视觉才一致；同时把 1px 窗口边框设为透明。
            component::Theme::update(cx, |theme| {
                theme.window_radius = px(26.0);
                theme.colors.window_border.a = 0.0;
                // 用真实安装的字体族替换 gpui-kit 默认的 `.SystemUIFont`（Linux 上不存在）。
                theme.font_family = ui_font_family().into();
                theme.mono_font_family = ui_mono_font_family().into();
            });
            cx.bind_keys([
                KeyBinding::new("escape", view::HidePanel, Some("FlickPanel")),
                KeyBinding::new("up", view::SelectPrev, Some("FlickPanel")),
                KeyBinding::new("down", view::SelectNext, Some("FlickPanel")),
                KeyBinding::new("ctrl-shift-l", view::ToggleThemeAction, Some("FlickPanel")),
                KeyBinding::new("ctrl-d", view::DetachSurface, Some("FlickPanel")),
                KeyBinding::new("escape", view::CloseDetached, Some("FlickDetached")),
            ]);

            let host_tx_open = host_tx.clone();
            // 视图失焦自发隐藏时，通过该通道请求应用级泵销毁窗口（在窗口更新帧之外执行）。
            let (hide_tx, mut hide_rx) = mpsc::unbounded_channel::<()>();
            let hide_tx_open = hide_tx.clone();
            let (detach_tx, mut detach_rx) = mpsc::unbounded_channel::<(DetachPayload, ThemeMode)>();
            let (close_tx, mut close_rx) = mpsc::unbounded_channel::<AnyWindowHandle>();
            let detach_tx_open = detach_tx.clone();
            let close_tx_open = close_tx.clone();
            let (theme_tx, mut theme_rx) = mpsc::unbounded_channel::<ThemeMode>();
            let theme_tx_open = theme_tx.clone();
            // 新开窗口（面板/独立窗口）都用当前主题；面板内切换后更新。
            let mut current_theme = initial;
            // 结果条数上限可被设置系统热更新；新窗口也用最新值。
            let mut current_limit = result_limit;

            // 应用级宿主事件泵（与窗口解耦）：窗口销毁后它仍存活，可再创建新窗口。
            cx.spawn(async move |cx: &mut AsyncApp| {
                let mut rx = host_rx;
                // 当前面板窗口句柄（窗口被销毁后会失效，用前需校验）。
                let mut handle: Option<AnyWindowHandle> = None;

                let mut detached: Vec<(SurfaceSessionId, AnyWindowHandle)> = Vec::new();
                let mut detach_seq: usize = 0;
                // 保持发送端存活，避免通道关闭后 select 分支忙循环。
                let _detach_tx_guard = detach_tx_open.clone();
                let _close_tx_guard = close_tx_open.clone();
                let _theme_tx_guard = theme_tx_open.clone();

                loop {
                    tokio::select! {
                        maybe = rx.recv() => {
                            let Some(event) = maybe else { break; };
                            match event {
                                HostToUi::Shutdown => {
                                    for (_, h) in detached.drain(..) {
                                        close_window(cx, h);
                                    }
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
                                                detach_tx_open.clone(),
                                                theme_tx_open.clone(),
                                                clipboard_watch.clone(),
                                                current_theme,
                                                current_limit,
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
                                                let viewport = window.viewport_size();
                                                let bounds = window.bounds();
                                                tracing::debug!(
                                                    viewport_width = f32::from(viewport.width),
                                                    viewport_height = f32::from(viewport.height),
                                                    bounds_width = f32::from(bounds.size.width),
                                                    bounds_height = f32::from(bounds.size.height),
                                                    scale = window.scale_factor(),
                                                    "面板窗口尺寸"
                                                );
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
                                        close_window(cx, h);
                                    }
                                }
                                HostToUi::SurfaceUpdated { session_id, content } => {
                                    // 面板未持有该会话时，把更新路由到匹配的独立窗口。
                                    let mut routed = false;
                                    for (sid, h) in detached.iter() {
                                        if *sid == session_id {
                                            routed = true;
                                            let h = *h;
                                            let c = content.clone();
                                            let _ = cx.update(|app| {
                                                let _ = h.update(app, |anyview, window, app2| {
                                                    if let Ok(view) = anyview
                                                        .downcast::<DetachedSurfaceView>(
                                                    ) {
                                                        view.update(app2, |v, cx2| {
                                                            v.set_content(c, window, cx2)
                                                        });
                                                    }
                                                });
                                            });
                                        }
                                    }
                                    if !routed {
                                        if let Some(h) = handle {
                                            let ev = HostToUi::SurfaceUpdated {
                                                session_id,
                                                content,
                                            };
                                            dispatch_panel_event(cx, h, ev);
                                        }
                                    }
                                }
                                HostToUi::SurfaceFailed { session_id, message } => {
                                    let mut routed = false;
                                    for (sid, h) in detached.iter() {
                                        if *sid == session_id {
                                            routed = true;
                                            let h = *h;
                                            let m = message.clone();
                                            let _ = cx.update(|app| {
                                                let _ = h.update(app, |anyview, _window, app2| {
                                                    if let Ok(view) = anyview
                                                        .downcast::<DetachedSurfaceView>(
                                                    ) {
                                                        view.update(app2, |v, cx2| {
                                                            v.set_error(m, cx2)
                                                        });
                                                    }
                                                });
                                            });
                                        }
                                    }
                                    if !routed {
                                        if let Some(h) = handle {
                                            let ev = HostToUi::SurfaceFailed {
                                                session_id,
                                                message,
                                            };
                                            dispatch_panel_event(cx, h, ev);
                                        }
                                    }
                                }
                                // 设置热更新：主题走既有的 theme 通道（同步 gpui-kit 主题、
                                // 面板与所有独立窗口）；结果上限直接应用到面板视图。
                                HostToUi::ThemeChanged { mode } => {
                                    let _ = theme_tx_open.send(mode.into());
                                }
                                HostToUi::ResultLimitChanged { limit } => {
                                    current_limit = limit;
                                    if let Some(h) = handle {
                                        let _ = cx.update(|app| {
                                            let _ = h.update(app, |anyview, _, app2| {
                                                if let Ok(view) = anyview.downcast::<AppView>() {
                                                    view.update(app2, |v, cx2| {
                                                        v.set_result_limit(limit, cx2)
                                                    });
                                                }
                                            });
                                        });
                                    }
                                }
                                other => {
                                    if let Some(h) = handle {
                                        dispatch_panel_event(cx, h, other.clone());
                                    }
                                }
                            }
                        }
                        maybe = detach_rx.recv() => {
                            if let Some((payload, theme)) = maybe {
                                current_theme = theme;
                                let session_id = payload.session_id;
                                match cx.update(|app| {
                                    open_detached_window(
                                        app,
                                        payload,
                                        theme,
                                        close_tx.clone(),
                                        host_tx_open.clone(),
                                        detach_seq,
                                    )
                                }) {
                                    Ok(h) => {
                                        detached.push((session_id, h));
                                        detach_seq += 1;
                                    }
                                    Err(err) => {
                                        eprintln!("flick: failed to open detached window: {err}");
                                    }
                                }
                            }
                        }
                        maybe = close_rx.recv() => {
                            if let Some(handle) = maybe {
                                detached.retain(|(_, h)| *h != handle);
                                close_window(cx, handle);
                            }
                        }
                        maybe = theme_rx.recv() => {
                            if let Some(mode) = maybe {
                                current_theme = mode;
                                if let Some(h) = handle {
                                    let _ = cx.update(|app| {
                                        let _ = h.update(app, |anyview, _, app2| {
                                            if let Ok(view) = anyview.downcast::<AppView>() {
                                                view.update(app2, |v, cx2| v.set_theme(mode, cx2));
                                            }
                                        });
                                    });
                                }
                                // 广播到所有独立窗口；顺带剪除已失效句柄（例如被 WM 以
                                // Alt+F4 销毁的窗口），避免 `detached` 永久累积死句柄。
                                let mut live = Vec::with_capacity(detached.len());
                                for (sid, h) in detached.drain(..) {
                                    let ok = cx.update(|app| {
                                        h.update(app, |anyview, _, app2| {
                                            if let Ok(view) =
                                                anyview.downcast::<DetachedSurfaceView>()
                                            {
                                                view.update(app2, |v, cx2| v.set_theme(mode, cx2));
                                            }
                                        })
                                        .is_ok()
                                    });
                                    if ok {
                                        live.push((sid, h));
                                    }
                                }
                                detached = live;
                            }
                        }
                        _ = hide_rx.recv() => {
                            // 失焦等视图自发隐藏：在窗口更新帧之外统一销毁窗口。
                            if let Some(h) = handle.take() {
                                close_window(cx, h);
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
