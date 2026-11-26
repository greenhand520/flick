//! 渲染层：唯一接触 gpui/gpui-kit 的模块。
//!
//! 窗口本体是**全屏透明 overlay**（参考 `awari`），搜索面板只是绘制在顶部的圆角卡片。
//! 窗口尺寸固定为显示器大小，结果增减只改变卡片高度、不触发 `window.resize`，
//! 从而避免 Wayland 下 resize 引发的键盘/输入法焦点抖动。卡片可在 overlay 内拖动。
//!
//! 生命周期由上层（`lib.rs` 的宿主事件泵）驱动：每次收到 `HostToUi::ShowSearchPanel`
//! 才创建新窗口，收到 `HostToUi::HideSearchPanel` 或用户按 Esc / 点击面板外时
//! `remove_window` 销毁窗口。因此隐藏时没有窗口、不会在任务栏留图标；再次唤起是全新映射的窗口，GNOME
//! 会把新窗口带到前台，从而规避「后台应用无法抢焦点」的问题。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use flick_domain::{ActionOutcome, HostToUi, SurfaceEventKind, SurfaceNode, UiToHost};
use flick_ui::render::{col, render_surface_with_inputs, InputWidgetSpec};
use flick_ui::style::SurfaceStyle;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::{Icon, IconName, Sizable};
use gpui_kit::*;
use tokio::sync::mpsc;

use crate::app::{
    desired_size, AppState, DetachPayload, Effect, UiEvent, DEBOUNCE_MILLIS,
};
use crate::theme::{self, Metrics, ThemeMode};

actions!(flick_shell, [
    HidePanel,
    SelectNext,
    SelectPrev,
    ToggleThemeAction,
    DetachSurface,
    CloseDetached
]);

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
    /// 请求应用级泵把 Surface 分离成独立窗口：携带内容快照、会话与当前主题。
    detach_tx: mpsc::UnboundedSender<(DetachPayload, ThemeMode)>,
    /// 主题变更广播：通知应用级泵同步所有独立窗口。
    theme_tx: mpsc::UnboundedSender<ThemeMode>,
    _subscriptions: Vec<Subscription>,
    /// Surface 内可编辑 Input/TextArea 的控件缓存，按节点 id 复用。
    widgets: HashMap<String, Entity<InputState>>,
    /// 各控件订阅的句柄，随视图存活。
    _widget_subs: Vec<Subscription>,
    _debounce: Option<Task<()>>,
    /// 上一轮 `panel.visible`，用于检测「可见 -> 隐藏」跃迁（只发一次销毁请求）。
    was_visible: bool,
    /// 分离窗口进行中：抑制面板隐藏时发给宿主的 `SurfaceClosed`
    /// （会话已转交给独立窗口，不能被当作已关闭）。
    suppress_surface_closed: bool,
    /// 面板拖动起点（overlay 内坐标）。
    drag_origin: Option<(f32, f32)>,
    /// 面板相对默认位置的偏移（overlay 内拖动）。
    panel_offset: (f32, f32),
}

impl AppView {
    /// 创建窗口内容。宿主事件不再由本视图订阅通道，而是由 `lib.rs` 的事件泵通过
    /// [`AppView::on_host`] 转发（这样窗口销毁后事件泵仍存活，可再开新窗口）。
    pub fn new(
        host_tx: mpsc::UnboundedSender<UiToHost>,
        hide_tx: mpsc::UnboundedSender<()>,
        detach_tx: mpsc::UnboundedSender<(DetachPayload, ThemeMode)>,
        theme_tx: mpsc::UnboundedSender<ThemeMode>,
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

        let mut view = Self {
            state: AppState::new(initial),
            metrics: Metrics::default(),
            input,
            focus,
            input_focus: input_focus.clone(),
            host_tx,
            hide_tx,
            detach_tx,
            theme_tx,
            _subscriptions: vec![input_sub],
            widgets: HashMap::new(),
            _widget_subs: Vec::new(),
            _debounce: None,
            was_visible: false,
            suppress_surface_closed: false,
            drag_origin: None,
            panel_offset: (0.0, 0.0),
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

    /// 主题广播入口：更新本地主题并重绘（由应用级泵调用）。
    pub fn set_theme(&mut self, theme: ThemeMode, cx: &mut Context<Self>) {
        self.state.theme = theme;
        cx.notify();
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
            self.sync_surface_widgets(window, cx);
            cx.notify();
        }
    }

    /// 为当前 Surface 的 Input/TextArea 建立（或复用）可编辑控件，并在其未聚焦时
    /// 用内容里的值同步显示。控件事件翻译为 `UiToHost::SurfaceEvent` 上报宿主。
    fn sync_surface_widgets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(content) = crate::app::active_surface(&self.state.panel).cloned() else {
            return;
        };
        let mut specs = Vec::new();
        collect_inputs(&content.body, &mut specs);
        for spec in specs {
            if !self.widgets.contains_key(&spec.id) {
                let placeholder = spec.placeholder.clone();
                let entity = cx.new(|cx| {
                    let mut state = InputState::new(window, cx);
                    if let Some(p) = &placeholder {
                        state = state.placeholder(p.clone());
                    }
                    state
                });
                self.widgets.insert(spec.id.clone(), entity.clone());
                let node_id = spec.id.clone();
                let sub = cx.subscribe_in(
                    &entity,
                    window,
                    move |this, state, event, _window, cx| match event {
                        InputEvent::Change => {
                            let text = state.read(cx).value().to_string();
                            if let Some(session_id) = this.state.panel.surface_session() {
                                let _ = this.host_tx.send(UiToHost::SurfaceEvent {
                                    session_id,
                                    kind: SurfaceEventKind::TextChanged {
                                        node_id: node_id.clone(),
                                        text,
                                    },
                                });
                            }
                        }
                        InputEvent::PressEnter { .. } => {
                            let text = state.read(cx).value().to_string();
                            if let Some(session_id) = this.state.panel.surface_session() {
                                let _ = this.host_tx.send(UiToHost::SurfaceEvent {
                                    session_id,
                                    kind: SurfaceEventKind::Submitted {
                                        node_id: node_id.clone(),
                                        text,
                                    },
                                });
                            }
                        }
                        _ => {}
                    },
                );
                self._widget_subs.push(sub);
            }
            if let Some(entity) = self.widgets.get(&spec.id).cloned() {
                let desired = spec.value.clone();
                let w = &mut *window;
                entity.update(cx, |state, cx| {
                    if !state.focus_handle(cx).is_focused(w) {
                        state.set_value(desired.clone(), w, cx);
                    }
                });
            }
        }
    }

    fn apply(&mut self, effects: Vec<Effect>, window: &mut Window, cx: &mut Context<Self>) {
        for effect in effects {
            match effect {
                Effect::Send(message) => {
                    // 分离时面板隐藏会由 presenter 追加 SurfaceClosed；但会话已转交
                    // 独立窗口，这里抑制该消息，避免运行时误关会话。
                    if self.suppress_surface_closed
                        && matches!(&message, UiToHost::SurfaceClosed { .. })
                    {
                        continue;
                    }
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
                Effect::Detach(payload) => {
                    let _ = self.detach_tx.send((payload, self.state.theme));
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

    fn on_overlay_mouse_move(
        &mut self,
        ev: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((ox, oy)) = self.drag_origin else {
            return;
        };
        let x: f32 = ev.position.x.into();
        let y: f32 = ev.position.y.into();
        self.panel_offset.0 += x - ox;
        self.panel_offset.1 += y - oy;
        self.drag_origin = Some((x, y));
        cx.notify();
    }

    fn on_select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(UiEvent::SelectionDown, window, cx);
    }

    fn on_select_prev(&mut self, _: &SelectPrev, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(UiEvent::SelectionUp, window, cx);
    }

    fn on_detach_surface(
        &mut self,
        _: &DetachSurface,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let effects = self.state.update(UiEvent::DetachRequested);
        let should_detach = effects.iter().any(|e| matches!(e, Effect::Detach(_)));
        self.apply(effects, window, cx);
        if should_detach {
            // 面板隐藏 + 通知 daemon（与 Esc 相同路径），但抑制 SurfaceClosed：
            // 会话转交独立窗口，不能当作已关闭。
            self.suppress_surface_closed = true;
            self.dispatch(UiEvent::EscapePressed, window, cx);
            self.suppress_surface_closed = false;
        }
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
        let _ = self.theme_tx.send(self.state.theme);
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
            .items_center()
            // 点击面板外的透明区域 = 关闭面板（layer-shell 全屏 overlay 需要自己处理点击外部）。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseDownEvent, window, cx| {
                    this.dispatch(UiEvent::WindowClosed, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(Self::on_overlay_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseUpEvent, _window, _cx| {
                    this.drag_origin = None;
                }),
            );

        let (ui, _metrics) = theme::resolve(self.state.theme);
        let (_, target_h) = desired_size(&self.state.panel, &self.metrics);
        // 窗口与面板等大（不再留 1px 透明边），避免出现一圈「透明边框」。
        let panel_h = target_h;
        let has_body = crate::app::active_surface(&self.state.panel).is_some()
            || !self.state.panel.routes.is_empty()
            || self.state.panel.last_outcome.is_some();

        let mut panel = div()
            .id("flick-panel")
            .flex()
            .flex_col()
            .w(px(self.metrics.width))
            .h(px(panel_h))
            .mt(px(crate::PANEL_TOP_MARGIN + self.panel_offset.1))
            .ml(px(self.panel_offset.0))
            .flex_none()
            .overflow_hidden()
            .track_focus(&self.focus)
            .key_context("FlickPanel")
            .on_action(cx.listener(Self::on_hide_panel))
            .on_action(cx.listener(Self::on_select_next))
            .on_action(cx.listener(Self::on_select_prev))
            .on_action(cx.listener(Self::on_toggle_theme))
            // ctrl-d 用捕获阶段绑定：先于聚焦的 Input 处理，避免输入框吞掉该快捷键。
            .capture_action(cx.listener(Self::on_detach_surface))
            // overlay 内拖动面板（layer-shell 全屏窗口不能移动，记录偏移即可）。
            // 输入区与结果区会 stop_propagation，避免在它们上面误触发拖动。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _window, cx| {
                    this.drag_origin = Some((ev.position.x.into(), ev.position.y.into()));
                    cx.stop_propagation();
                }),
            )
            .bg(col(ui.panel_bg))
            .text_color(col(ui.text))
            .rounded(px(self.metrics.radius_panel))
            .px(px(self.metrics.padding_h));
        if has_body {
            panel = panel.pb(px(self.metrics.padding_bottom));
        }

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
                                // 覆盖 gpui Input Large 默认的 12px 左右内边距，
                                // 保留字号，只把图标/文字拉近卡片边缘。
                                .px(px(self.metrics.input_padding_h))
                                .prefix(Icon::new(IconName::Search)),
                        ),
                ),
        );

        // 仅覆盖主题颜色与行圆角；高度相关字段（pad/row_gap/line_height/
        // section_gap）保持 `SurfaceStyle::default()`，与 `app.rs` 中
        // `estimate_height(..., &SurfaceStyle::default())` 的估算口径一致。
        let surface_style = SurfaceStyle {
            panel_bg: ui.panel_bg,
            surface: ui.surface,
            text: ui.text,
            text_muted: ui.text_muted,
            border: ui.border,
            primary: ui.selected_bg,
            primary_fg: ui.selected_fg,
            radius_row: self.metrics.radius_row,
            ..SurfaceStyle::default()
        };

        // 结果区 + 结果/通知行：整体限制在 results_max_height 内并可纵向滚动，
        // 避免行数超过 ~7 时溢出/裁切。若存在活动 Surface，则内联渲染该 Surface 取代结果列表。
        if let Some(surface) = crate::app::active_surface(&self.state.panel).cloned() {
            // 交互失败的错误提示（红色）显示在 Surface 顶部。
            if let Some(err) = self.state.surface_error().map(str::to_owned) {
                panel = panel.child(
                    div()
                        .w_full()
                        .pt(px(self.metrics.content_gap))
                        .text_color(col((220, 53, 69, 255)))
                        .child(Label::new(err)),
                );
            }
            let session = self.state.panel.surface_session();
            let host_tx = self.host_tx.clone();
            let events: Arc<dyn Fn(SurfaceEventKind) + Send + Sync> = Arc::new(move |kind| {
                if let Some(session_id) = session {
                    let _ = host_tx.send(UiToHost::SurfaceEvent { session_id, kind });
                }
            });
            // 可编辑控件来自缓存；缺失时 render 层退回只读样式。
            let widgets = &self.widgets;
            let input_widget = |spec: &InputWidgetSpec| {
                widgets.get(spec.id).map(|entity| {
                    Input::new(entity)
                        .appearance(false)
                        .cleanable(true)
                        .with_size(gpui_kit::component::Size::Large)
                        .w_full()
                        .into_any_element()
                })
            };
            let rendered =
                render_surface_with_inputs(&surface, &surface_style, events, &input_widget);
            panel = panel.child(
                div()
                    .id("flick-surface")
                    .w_full()
                    .max_h(px(self.metrics.results_max_height))
                    .overflow_y_scroll()
                    .pt(px(self.metrics.content_gap))
                    .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| {
                        cx.stop_propagation()
                    }))
                    .child(rendered),
            );
        } else {
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
                    .pt(px(self.metrics.content_gap))
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
                    let mut row = div()
                        .h(px(self.metrics.row_height))
                        .w_full()
                        .flex()
                        .items_center()
                        .px_3()
                        .rounded(px(self.metrics.radius_row))
                        .bg(col(bg))
                        .text_color(col(fg))
                        // 鼠标点击等同 Enter：先选中该行，再走统一的提交路径。
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.state.panel.selection.index = index;
                                this.dispatch(UiEvent::Submitted, window, cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child(Label::new(item.title.clone()));
                    if let Some(subtitle) = &item.subtitle {
                        row = row.child(
                            div()
                                .ml(px(8.0))
                                .text_color(col(ui.text_muted))
                                .child(Label::new(subtitle.clone())),
                        );
                    }
                    if let Some(badge) = item.execution.as_ref().and_then(|e| e.badge.as_ref()) {
                        row = row.child(
                            div()
                                .ml(px(8.0))
                                .rounded_full()
                                .px_2()
                                .bg(col(ui.selected_bg))
                                .text_color(col(ui.selected_fg))
                                .child(Label::new(badge.clone())),
                        );
                    }
                    results = results.child(row);
                }

                if let Some(outcome) = &self.state.panel.last_outcome {
                    let row = div()
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded(px(self.metrics.radius_row))
                        .bg(col(ui.surface))
                        .text_color(col(ui.text_muted));
                    match outcome {
                        // 兜底：`OpenSurface` 但 body 为空（故未走 Surface 渲染）时，
                        // 不再输出 "Surface: {id}" 纯文本；按 spec §4.4 用
                        // Badge("Surface") + Text(title 或 surface_id) 组合渲染。
                        ActionOutcome::OpenSurface { surface_id, content, .. } => {
                            let label = content.title.clone().unwrap_or_else(|| surface_id.clone());
                            results = results.child(
                                row.flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .rounded_full()
                                            .px_2()
                                            .py_1()
                                            .bg(col(ui.text_muted))
                                            .text_color(col(ui.panel_bg))
                                            .child(Label::new("Surface")),
                                    )
                                    .child(div().child(Label::new(label))),
                            );
                        }
                        other => {
                            let text = match other {
                                ActionOutcome::Data(value) => format!("Result: {value}"),
                                ActionOutcome::Notice { message } => format!("Notice: {message}"),
                                ActionOutcome::Error(error) => format!("Error: {error}"),
                                ActionOutcome::Text(text) => text.clone(),
                                ActionOutcome::Url(url) => url.clone(),
                                ActionOutcome::File(file) => file.display_name.clone(),
                                ActionOutcome::OpenSurface { .. } => {
                                    unreachable!("OpenSurface handled above")
                                }
                            };
                            results = results.child(row.child(Label::new(text)));
                        }
                    }
                }

                panel = panel.child(results);
            }
        }

        root = root.child(panel);
        root.into_any_element()
    }
}

/// Surface 内一个可编辑文本节点的值快照（用于建立/同步控件）。
pub(crate) struct InputSpec {
    pub id: String,
    pub value: String,
    pub placeholder: Option<String>,
}

/// 递归收集 Surface 节点中的 Input/TextArea（含 Section/Row/Column 子节点）。
pub(crate) fn collect_inputs(nodes: &[SurfaceNode], out: &mut Vec<InputSpec>) {
    for node in nodes {
        match node {
            SurfaceNode::Input {
                id,
                value,
                placeholder,
            } => out.push(InputSpec {
                id: id.clone(),
                value: value.clone(),
                placeholder: placeholder.clone(),
            }),
            SurfaceNode::TextArea {
                id,
                value,
                placeholder,
                ..
            } => out.push(InputSpec {
                id: id.clone(),
                value: value.clone(),
                placeholder: placeholder.clone(),
            }),
            SurfaceNode::Section { children, .. }
            | SurfaceNode::Row { children, .. }
            | SurfaceNode::Column { children, .. } => collect_inputs(children, out),
            _ => {}
        }
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
