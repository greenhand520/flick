//! 渲染层：唯一接触 gpui/gpui-kit 的模块。
//!
//! 形态参考真实 gpui launcher（`gpui-shell` / `awari`）：窗口本体是**面板尺寸的
//! 小窗**（约 666 宽，顶部居中，透明、可拖动），搜索面板就是绘制在窗口里的圆角卡片。
//! 卡片高度随结果条数变化时，由 `AppView::dispatch` 调 `window.resize` 同步窗口尺寸。
//! 早期曾用「全屏透明 overlay + 卡片」来绕开 GNOME/Mutter（无 wlr-layer-shell）下
//! 小窗按内容 resize 不可靠的问题，现改为小窗 + 显式 resize。

use std::time::Duration;

use flick_domain::{ActionOutcome, HostToUi, UiToHost};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::{Icon, IconName, Sizable};
use gpui_kit::*;
use tokio::sync::mpsc;

use crate::app::{desired_size, AppState, Effect, UiEvent, DEBOUNCE_MILLIS};
use crate::theme::{self, Metrics, ThemeMode};

actions!(flick_shell, [HidePanel, SelectNext, SelectPrev, ToggleThemeAction]);

/// 便于把 (r,g,b,a) 转成 gpui 颜色。
fn col((r, g, b, a): (u8, u8, u8, u8)) -> Rgba {
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: a as f32 / 255.0,
    }
}

/// 搜索面板视图，持有纯状态 + 输入实体 + 焦点 + 通道。
pub struct AppView {
    state: AppState,
    metrics: Metrics,
    input: Entity<InputState>,
    focus: FocusHandle,
    host_tx: mpsc::UnboundedSender<UiToHost>,
    close_on_hide: bool,
    _subscriptions: Vec<Subscription>,
    _host_pump: Task<()>,
    _debounce: Option<Task<()>>,
    /// 上次已应用的窗口尺寸 (宽,高)；仅变化时才调用 window.resize，避免抖动。
    last_applied_size: Option<(f32, f32)>,
}

impl AppView {
    pub fn new(
        host_tx: mpsc::UnboundedSender<UiToHost>,
        mut host_rx: mpsc::UnboundedReceiver<HostToUi>,
        initial: ThemeMode,
        close_on_hide: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索"));
        let focus = cx.focus_handle();
        // 输入框自身的焦点句柄：既用于「打开即聚焦」，也用于失焦隐藏（blur→hide）。
        let input_focus = input.focus_handle(cx);

        let input_sub = cx.subscribe_in(&input, window, |this, state, event, window, cx| {
            match event {
                InputEvent::Change => {
                    let text = state.read(cx).value().to_string();
                    this.dispatch(UiEvent::InputChanged(text), window, cx);
                }
                InputEvent::PressEnter { .. } => {
                    this.dispatch(UiEvent::Submitted, window, cx);
                }
                _ => {}
            }
        });

        // 失焦隐藏挂在「输入框」的焦点句柄上（焦点实际在输入框），而非面板根句柄。
        let blur_sub = cx.on_focus_out(&input_focus, window, |this, _event, window, cx| {
            if this.state.panel.visible {
                this.dispatch(UiEvent::WindowClosed, window, cx);
            }
        });

        let pump = cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = host_rx.recv().await {
                let ok = this.update_in(cx, |this, window, cx| {
                    this.dispatch(UiEvent::Host(event), window, cx);
                });
                if ok.is_err() {
                    break;
                }
            }
        });

        let mut view = Self {
            state: AppState::new(initial),
            metrics: Metrics::default(),
            input,
            focus,
            host_tx,
            close_on_hide,
            _subscriptions: vec![input_sub, blur_sub],
            _host_pump: pump,
            _debounce: None,
            last_applied_size: None,
        };
        // 打开即聚焦输入框，用户可直接输入（修复 iced 缺陷：打开后不自动聚焦）。
        // 面板根节点仍 track_focus + key_context("FlickPanel")，键盘上下文沿祖先链生效，
        // 故 Esc / ↑ / ↓ 的 action 依然能命中。
        window.focus(&input_focus, cx);
        // 演示/自检钩子：设置 FLICK_GPUI_DEMO_QUERY 时启动即模拟一次输入，便于截图结果态。
        if let Ok(q) = std::env::var("FLICK_GPUI_DEMO_QUERY") {
            if !q.is_empty() {
                view.dispatch(UiEvent::InputChanged(q), window, cx);
            }
        }
        view
    }

    fn dispatch(&mut self, event: UiEvent, window: &mut Window, cx: &mut Context<Self>) {
        let effects = self.state.update(event);
        self.apply(effects, window, cx);
        self.sync_window_size(window);
        cx.notify();
    }

    /// 面板尺寸的小窗口：结果增减时把窗口 resize 到「卡片高度 + 边框」，
    /// 避免整屏透明区域，也避免结果被裁。仅在尺寸变化时调用。
    fn sync_window_size(&mut self, window: &mut Window) {
        if !self.state.panel.visible {
            return;
        }
        let (w, h) = desired_size(&self.state.panel, &self.metrics);
        let target = (w, h);
        if self.last_applied_size != Some(target) {
            window.resize(Size::new(px(target.0), px(target.1)));
            self.last_applied_size = Some(target);
        }
    }

    fn apply(&mut self, effects: Vec<Effect>, window: &mut Window, cx: &mut Context<Self>) {
        for effect in effects {
            match effect {
                Effect::Send(message) => {
                    let _ = self.host_tx.send(message);
                }
                Effect::StartDebounce { revision } => {
                    let task = cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(DEBOUNCE_MILLIS))
                            .await;
                        let _ = this.update_in(cx, |this, window, cx| {
                            this.dispatch(UiEvent::DebounceElapsed { revision }, window, cx);
                        });
                    });
                    self._debounce = Some(task); // 旧任务 drop 即取消
                }
                Effect::Quit => {
                    cx.quit();
                }
            }
        }
        // 全屏 overlay 不做窗口 resize：面板高度在 `render` 里按内容计算。
        if !self.state.panel.visible && self.close_on_hide {
            window.remove_window();
        }
    }

    fn on_hide_panel(&mut self, _: &HidePanel, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(UiEvent::EscapePressed, window, cx);
    }

    fn on_select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(UiEvent::SelectionDown, window, cx);
    }

    fn on_select_prev(&mut self, _: &SelectPrev, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(UiEvent::SelectionUp, window, cx);
    }

    fn on_toggle_theme(
        &mut self,
        _: &ToggleThemeAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dispatch(UiEvent::ToggleTheme, window, cx);
        // 同步切换 gpui-kit 组件的官方主题（Input 等颜色全取自该主题）。
        let kit_mode = match self.state.theme {
            ThemeMode::Dark => gpui_kit::component::ThemeMode::Dark,
            ThemeMode::Light => gpui_kit::component::ThemeMode::Light,
        };
        gpui_kit::component::Theme::change(kit_mode, Some(window), cx);
        // Theme::change 会重置 window_radius/window_border；恢复窗口级圆角 26（卡片用一半 13，
        // 见 theme::Metrics::radius_panel）并隐藏 1px 窗口边框。
        gpui_kit::component::Theme::update(cx, |theme| {
            theme.window_radius = px(26.0);
            theme.colors.window_border.a = 0.0;
        });
        // Theme::change 会重置 tokens/window_radius，这里恢复全屏透明根 surface。
        persist_theme(self.state.theme);
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 可见透明 overlay 根：无背景，桌面透出；面板按需挂载。
        let mut root = div()
            .id("flick-root")
            .size_full()
            .flex()
            .flex_col()
            .items_center();

        let (ui, _metrics) = theme::resolve(self.state.theme);
        let (_, target_h) = desired_size(&self.state.panel, &self.metrics);
        // 窗口与面板等大（不再留 1px 透明边），避免出现一圈「透明边框」。
        let panel_h = target_h;

        let mut panel = div()
            .id("flick-panel")
            .flex()
            .flex_col()
            .w(px(self.metrics.width))
            .h(px(panel_h))
            .flex_none()
            .overflow_hidden()
            .track_focus(&self.focus)
            .key_context("FlickPanel")
            .on_action(cx.listener(Self::on_hide_panel))
            .on_action(cx.listener(Self::on_select_next))
            .on_action(cx.listener(Self::on_select_prev))
            .on_action(cx.listener(Self::on_toggle_theme))
            // 按住面板空白/边缘即可拖动整个窗口（gpui Wayland 的 start_window_move）。
            // 输入区与结果区会 stop_propagation，避免在它们上面误触发拖动。
            .on_mouse_down(MouseButton::Left, |_ev, window, _cx| {
                window.start_window_move()
            })
            .bg(col(ui.panel_bg))
            .text_color(col(ui.text))
            .rounded(px(self.metrics.radius_panel))
            .px(px(self.metrics.padding_h));

        // 搜索行：占满条高、垂直居中，大号字号（参考 gpui-shell/awari 的 query row）。
        panel = panel.child(
            div()
                .h(px(self.metrics.input_height))
                .w_full()
                .flex()
                .items_center()
                .child(
                    div()
                        .w_full()
                        // 让输入区吞掉鼠标按下，避免点击输入框时触发窗口拖动。
                        .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| {
                            cx.stop_propagation()
                        }))
                        .child(
                            Input::new(&self.input)
                                .appearance(false)
                                .cleanable(true)
                                .with_size(gpui_kit::component::Size::Large)
                                .w_full()
                                .prefix(Icon::new(IconName::Search)),
                        ),
                ),
        );

        // 结果区 + 结果/通知行：整体限制在 results_max_height 内并可纵向滚动，
        // 避免行数超过 ~7 时溢出/裁切。
        let has_results =
            !self.state.panel.routes.is_empty() || self.state.panel.last_outcome.is_some();
        if has_results {
            let mut results = div()
                .id("flick-results")
                .flex()
                .flex_col()
                .gap(px(self.metrics.row_gap))
                .w_full()
                .max_h(px(self.metrics.results_max_height))
                .overflow_y_scroll()
                .pt_2()
                // 结果区吞掉鼠标按下，避免点结果行时误触发窗口拖动。
                .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| {
                    cx.stop_propagation()
                }));

            for (index, item) in self.state.panel.routes.iter().enumerate() {
                let selected = index == self.state.panel.selection.index;
                let (bg, fg) = if selected {
                    (ui.selected_bg, ui.selected_fg)
                } else {
                    (ui.surface, ui.text)
                };
                results = results.child(
                    div()
                        .h(px(self.metrics.row_height))
                        .w_full()
                        .flex()
                        .items_center()
                        .px_3()
                        .rounded(px(self.metrics.radius_row))
                        .bg(col(bg))
                        .text_color(col(fg))
                        .child(Label::new(item.title.clone())),
                );
            }

            // 最近一次调用的结果 / 通知 / 错误行（与 iced 的渲染保持一致）。
            if let Some(outcome) = &self.state.panel.last_outcome {
                let text = match outcome {
                    ActionOutcome::Data(value) => format!("Result: {value}"),
                    ActionOutcome::Notice { message } => format!("Notice: {message}"),
                    ActionOutcome::Error(error) => format!("Error: {error}"),
                };
                results = results.child(
                    div()
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded(px(self.metrics.radius_row))
                        .bg(col(ui.surface))
                        .text_color(col(ui.text_muted))
                        .child(Label::new(text)),
                );
            }

            panel = panel.child(results);
        }

        root = root.child(panel);
        root.into_any_element()
    }
}

/// 尽力持久化主题覆盖；失败静默（$HOME 只读时预期失败）。
/// 与 `detect_initial_theme` 共用同一路径与 `mode = "dark"|"light"` 键。
fn persist_theme(mode: ThemeMode) {
    let path = crate::theme_config_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let value = match mode {
        ThemeMode::Dark => "dark",
        ThemeMode::Light => "light",
    };
    let _ = std::fs::write(path, format!("mode = \"{value}\"\n"));
}
