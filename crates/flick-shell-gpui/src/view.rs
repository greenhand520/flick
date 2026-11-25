//! 渲染层：唯一接触 gpui/gpui-kit 的模块。
//!
//! 窗口本体是**面板尺寸的小窗**（约 666 宽，顶部居中，透明、可拖动），搜索面板就是
//! 绘制在窗口里的圆角卡片。卡片高度随结果条数变化时，由 `AppView::dispatch` 调
//! `window.resize` 同步窗口尺寸。
//!
//! 生命周期由上层（`lib.rs` 的宿主事件泵）驱动：每次收到 `HostToUi::ShowSearchPanel`
//! 才创建新窗口，收到 `HostToUi::HideSearchPanel` 或用户按 Esc / 失焦时 `remove_window`
//! 销毁窗口。因此隐藏时没有窗口、不会在任务栏留图标；再次唤起是全新映射的窗口，GNOME
//! 会把新窗口带到前台，从而规避「后台应用无法抢焦点」的问题。

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
    /// 输入框自身的焦点句柄（打开即聚焦 / 失焦隐藏 / 重新显示时再聚焦）。
    input_focus: FocusHandle,
    host_tx: mpsc::UnboundedSender<UiToHost>,
    /// 请求应用级宿主泵销毁本窗口（隐藏时用；避免在窗口自身的焦点/更新帧内直接
    /// `remove_window`，否则会触发 "scheduling follow-up frame ... window not found"）。
    hide_tx: mpsc::UnboundedSender<()>,
    _subscriptions: Vec<Subscription>,
    _debounce: Option<Task<()>>,
    /// 上次已应用的窗口尺寸 (宽,高)；仅变化时才调用 window.resize，避免抖动。
    last_applied_size: Option<(f32, f32)>,
    /// 上一轮 `panel.visible`，用于检测「可见 -> 隐藏」跃迁（只发一次销毁请求）。
    was_visible: bool,
}

impl AppView {
    /// 创建窗口内容。宿主事件不再由本视图订阅通道，而是由 `lib.rs` 的事件泵通过
    /// [`AppView::on_host`] 转发（这样窗口销毁后事件泵仍存活，可再开新窗口）。
    pub fn new(
        host_tx: mpsc::UnboundedSender<UiToHost>,
        hide_tx: mpsc::UnboundedSender<()>,
        initial: ThemeMode,
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
        // 触发后会隐藏面板，并在 `apply` 里 `remove_window` 销毁窗口。
        let blur_sub = cx.on_focus_out(&input_focus, window, |this, _event, window, cx| {
            if this.state.panel.visible {
                this.dispatch(UiEvent::WindowClosed, window, cx);
            }
        });

        let mut view = Self {
            state: AppState::new(initial),
            metrics: Metrics::default(),
            input,
            focus,
            input_focus: input_focus.clone(),
            host_tx,
            hide_tx,
            _subscriptions: vec![input_sub, blur_sub],
            _debounce: None,
            last_applied_size: None,
            was_visible: false,
        };
        // 打开即聚焦输入框，用户可直接输入。
        window.focus(&input_focus, cx);
        // 演示/自检钩子：设置 FLICK_GPUI_DEMO_QUERY 时启动即模拟一次输入，便于截图结果态。
        if let Ok(q) = std::env::var("FLICK_GPUI_DEMO_QUERY") {
            if !q.is_empty() {
                view.dispatch(UiEvent::InputChanged(q), window, cx);
            }
        }
        view
    }

    /// 把焦点交给搜索输入框（被宿主事件泵在窗口显示后调用）。
    pub fn focus_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.input_focus, cx);
    }

    /// 接收一条宿主事件（由 `lib.rs` 的事件泵转发），进入正常 dispatch 流程。
    pub fn on_host(&mut self, event: HostToUi, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(UiEvent::Host(event), window, cx);
    }

    fn dispatch(&mut self, event: UiEvent, window: &mut Window, cx: &mut Context<Self>) {
        let effects = self.state.update(event);
        self.apply(effects, window, cx);
        // 面板已隐藏时窗口即将被应用级泵销毁，不再为它 resize/调度下一帧，
        // 避免对「已删除窗口」调度帧引发 "window not found"。
        if self.state.panel.visible {
            self.sync_window_size(window);
            cx.notify();
        }
    }

    /// 面板尺寸的小窗口：结果增减时把窗口 resize 到「卡片高度」，
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
        // 「隐藏即销毁窗口」模型：但销毁动作不能在本窗口的更新帧里同步执行——失焦等场景下
        // 同步 `remove_window` 会让 gpui 继续为已删除窗口调度下一帧，报
        // "scheduling follow-up frame ... window not found"。因此这里只发一次“请销毁”信号，
        // 由应用级宿主泵在窗口更新之外统一销毁（与快捷键/esc 隐藏走同一条路径）。
        let now_visible = self.state.panel.visible;
        if self.was_visible && !now_visible {
            let _ = self.hide_tx.send(());
        }
        self.was_visible = now_visible;
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
            // Theme::change 后同样恢复真实字体族（避免回退到不存在的 `.SystemUIFont`）。
            theme.font_family = crate::ui_font_family().into();
            theme.mono_font_family = crate::ui_mono_font_family().into();
        });
        persist_theme(self.state.theme);
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 隐藏态：不渲染任何东西（此时窗口通常已被 remove_window 销毁）。
        if !self.state.panel.visible {
            return div().id("flick-root").size_full().into_any_element();
        }
        // 可见透明窗口根：无背景，桌面透出；面板按需挂载。
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
