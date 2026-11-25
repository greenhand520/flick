//! Ctrl+D 分离出来的 Surface 独立窗口视图。
//!
//! 内容为分离那一刻的 `SurfaceContent` 快照；不订阅 daemon 事件、不跟随后续搜索。
//! 窗口的创建与销毁由 `lib.rs` 的应用级事件泵统一管理：本视图只负责渲染，并在
//! Esc 时通过 `close_tx` 请求泵在窗口更新帧之外 `remove_window`。

use std::collections::HashMap;
use std::sync::Arc;

use flick_domain::{SurfaceContent, SurfaceEventKind, SurfaceSessionId, UiToHost};
use flick_ui::render::{col, render_surface_with_inputs, InputWidgetSpec};
use flick_ui::style::{estimate_height, SurfaceStyle};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::Sizable;
use gpui_kit::*;
use tokio::sync::mpsc;

use crate::theme::{self, ThemeMode};
use crate::view::{collect_inputs, CloseDetached};

/// 独立窗口逻辑宽度（与搜索面板一致，统一取 `theme::PANEL_WIDTH`）。
pub const DETACHED_WIDTH: f32 = theme::PANEL_WIDTH;
/// 独立窗口最小高度。
pub const DETACHED_MIN_HEIGHT: f32 = 160.0;
/// 独立窗口最大高度（超出内容滚动）。
pub const DETACHED_MAX_HEIGHT: f32 = 720.0;
/// 卡片上下内边距。
pub const DETACHED_PAD_V: f32 = 16.0;
/// 级联错开步长。
pub const DETACHED_CASCADE_STEP: f32 = 24.0;
/// 级联错开回绕数量。
pub const DETACHED_CASCADE_WRAP: usize = 8;

/// 由主题构造 Surface 渲染样式。
///
/// 独立窗口的**尺寸估算**与**渲染**共用这一个构造入口，使两者的布局字段
/// （pad/row_gap/line_height/section_gap 等）不可能漂移——这也是 `detached_size`
/// 接收 `&SurfaceStyle` 的原因（见 Minor #4）。
pub(crate) fn surface_style(theme: ThemeMode) -> SurfaceStyle {
    let (ui, metrics) = theme::resolve(theme);
    SurfaceStyle {
        panel_bg: ui.panel_bg,
        surface: ui.surface,
        text: ui.text,
        text_muted: ui.text_muted,
        border: ui.border,
        primary: ui.selected_bg,
        primary_fg: ui.selected_fg,
        radius_row: metrics.radius_row,
        ..SurfaceStyle::default()
    }
}

/// 由 Surface 内容与样式计算独立窗口尺寸（宽, 高）。
///
/// `needs_search` 为真时在内容上方保留一条搜索框，因此高度额外加上
/// `crate::PANEL_INPUT_HEIGHT`（与面板搜索行同高）。
pub fn detached_size(
    content: &SurfaceContent,
    style: &SurfaceStyle,
    needs_search: bool,
) -> (f32, f32) {
    let mut content_h = estimate_height(content, style);
    if needs_search {
        content_h += crate::PANEL_INPUT_HEIGHT;
    }
    let h = (content_h + DETACHED_PAD_V * 2.0).clamp(DETACHED_MIN_HEIGHT, DETACHED_MAX_HEIGHT);
    (DETACHED_WIDTH, h)
}

/// 计算独立窗口在给定显示器内的最终 bounds `(x, y, w, h)`。
///
/// 基准位置与搜索面板同源（水平居中；`y = origin_y + PANEL_TOP_MARGIN +
/// PANEL_INPUT_HEIGHT + 24`），叠加 `cascade_offset(index)`。随后把窗口**完整**夹进
/// 显示器：高度先按可用高度截断，x 夹到 `[ox, ox+screen_w-w]`、y 夹到
/// `[oy, oy+screen_h-h]`。若「基准 + 尺寸」在夹取前就已经越界，按设计规范回落到
/// `(ox+40, oy+120)` 再夹一次。纯函数，便于单测。
pub fn detached_bounds(
    content: &SurfaceContent,
    theme: ThemeMode,
    display: (f32, f32, f32, f32),
    index: usize,
    needs_search: bool,
) -> (f32, f32, f32, f32) {
    let (ox, oy, screen_w, screen_h) = display;
    let style = surface_style(theme);
    let (w, h) = detached_size(content, &style, needs_search);
    let (dx, dy) = cascade_offset(index);

    let base_x = ox + (screen_w - w) / 2.0;
    let base_y = oy + crate::PANEL_TOP_MARGIN + crate::PANEL_INPUT_HEIGHT + 24.0;

    // 高度不得超过显示器可用高度（保证窗口完整可见）。
    let h = h.min(screen_h);

    let x_min = ox;
    let x_max = ox + screen_w - w;
    let y_min = oy;
    let y_max = oy + screen_h - h;

    // 夹取到 `[lo, hi]`；若可用区间为负（显示器比窗口还小），退化为贴住 lo。
    let clamp = |v: f32, lo: f32, hi: f32| if hi < lo { lo } else { v.max(lo).min(hi) };

    let unclamped_x = base_x + dx;
    let unclamped_y = base_y + dy;

    // 基准 + 尺寸越界 → 回落到规范位置；否则沿用基准。之后统一夹一次。
    let overflows = unclamped_x < x_min
        || unclamped_x + w > ox + screen_w
        || unclamped_y < y_min
        || unclamped_y + h > oy + screen_h;
    let (candidate_x, candidate_y) = if overflows {
        (ox + 40.0, oy + 120.0)
    } else {
        (unclamped_x, unclamped_y)
    };

    let x = clamp(candidate_x, x_min, x_max);
    let y = clamp(candidate_y, y_min, y_max);
    (x, y, w, h)
}

/// 第 `index` 个独立窗口相对基准位置的级联偏移。
pub fn cascade_offset(index: usize) -> (f32, f32) {
    let n = (index % DETACHED_CASCADE_WRAP) as f32 * DETACHED_CASCADE_STEP;
    (n, n)
}

/// 独立 Surface 窗口视图。
pub struct DetachedSurfaceView {
    content: SurfaceContent,
    session_id: SurfaceSessionId,
    needs_search: bool,
    theme: ThemeMode,
    focus: FocusHandle,
    /// 搜索框（仅 `needs_search` 时建立），其 Change → SearchChanged，
    /// Enter → Submitted{node_id:"__search__"}。
    search_input: Option<Entity<InputState>>,
    /// 回送控件/搜索/关闭事件到宿主运行时。
    surface_tx: mpsc::UnboundedSender<UiToHost>,
    /// 请求应用级泵销毁本窗口。
    close_tx: mpsc::UnboundedSender<AnyWindowHandle>,
    /// Surface 内可编辑 Input/TextArea 的控件缓存，按节点 id 复用。
    widgets: HashMap<String, Entity<InputState>>,
    _widget_subs: Vec<Subscription>,
    /// 交互失败的错误提示（红色文本）。
    error: Option<String>,
}

impl DetachedSurfaceView {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        content: SurfaceContent,
        session_id: SurfaceSessionId,
        needs_search: bool,
        theme: ThemeMode,
        close_tx: mpsc::UnboundedSender<AnyWindowHandle>,
        surface_tx: mpsc::UnboundedSender<UiToHost>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let mut widget_subs: Vec<Subscription> = Vec::new();
        let search_input = if needs_search {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索"));
            let sub = cx.subscribe_in(
                &input,
                window,
                |this, state, event, _window, cx| {
                    let text = state.read(cx).value().to_string();
                    let kind = match event {
                        InputEvent::Change => {
                            Some(SurfaceEventKind::SearchChanged { text: text.clone() })
                        }
                        InputEvent::PressEnter { .. } => Some(SurfaceEventKind::Submitted {
                            node_id: "__search__".into(),
                            text: text.clone(),
                        }),
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        let _ = this.surface_tx.send(UiToHost::SurfaceEvent {
                            session_id: this.session_id,
                            kind,
                        });
                    }
                },
            );
            widget_subs.push(sub);
            Some(input)
        } else {
            None
        };

        let mut view = Self {
            content,
            session_id,
            needs_search,
            theme,
            focus,
            search_input,
            surface_tx,
            close_tx,
            widgets: HashMap::new(),
            _widget_subs: widget_subs,
            error: None,
        };
        view.sync_widgets(window, cx);
        if let Some(input) = view.search_input.clone() {
            window.focus(&input.focus_handle(cx), cx);
        } else {
            window.focus(&view.focus, cx);
        }
        view
    }

    /// 为当前 Surface 的 Input/TextArea 建立（或复用）可编辑控件并同步值。
    fn sync_widgets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut specs = Vec::new();
        collect_inputs(&self.content.body, &mut specs);
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
                    move |this, state, event, _window, cx| {
                        let text = state.read(cx).value().to_string();
                        let kind = match event {
                            InputEvent::Change => Some(SurfaceEventKind::TextChanged {
                                node_id: node_id.clone(),
                                text: text.clone(),
                            }),
                            InputEvent::PressEnter { .. } => Some(SurfaceEventKind::Submitted {
                                node_id: node_id.clone(),
                                text: text.clone(),
                            }),
                            _ => None,
                        };
                        if let Some(kind) = kind {
                            let _ = this.surface_tx.send(UiToHost::SurfaceEvent {
                                session_id: this.session_id,
                                kind,
                            });
                        }
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

    /// 宿主推送的新内容：更新快照、清除错误、重建/同步可编辑控件并重绘。
    pub fn set_content(
        &mut self,
        content: SurfaceContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.content = content;
        self.error = None;
        self.sync_widgets(window, cx);
        cx.notify();
    }

    /// 宿主报告的交互失败：记录错误信息并重绘（内容保持不变）。
    pub fn set_error(&mut self, message: String, cx: &mut Context<Self>) {
        self.error = Some(message);
        cx.notify();
    }

    /// 主题广播：更新配色并请求重绘。
    pub fn set_theme(&mut self, theme: ThemeMode, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    fn on_close(&mut self, _: &CloseDetached, window: &mut Window, _cx: &mut Context<Self>) {
        // 先告知宿主会话已关闭（会话生命周期由运行时负责），
        // 再交给应用级泵在窗口更新帧之外销毁，避免 "window not found"。
        let _ = self.surface_tx.send(UiToHost::SurfaceClosed {
            session_id: self.session_id,
        });
        let _ = self.close_tx.send(window.window_handle());
    }
}

impl Render for DetachedSurfaceView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (ui, metrics) = theme::resolve(self.theme);
        let style = surface_style(self.theme);

        // 搜索行（仅插件声明 needs_search 时）。
        let search_row = if self.needs_search {
            self.search_input.clone().map(|input| {
                div()
                    .h(px(crate::PANEL_INPUT_HEIGHT))
                    .w_full()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .w_full()
                            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| {
                                cx.stop_propagation()
                            }))
                            .child(
                                Input::new(&input)
                                    .appearance(false)
                                    .cleanable(true)
                                    .with_size(gpui_kit::component::Size::Large)
                                    .w_full(),
                            ),
                    )
            })
        } else {
            None
        };

        let error_row = self.error.clone().map(|message| {
            div()
                .w_full()
                .text_color(col((220, 53, 69, 255)))
                .child(Label::new(message))
        });

        // Surface 交互节点的事件回送（session_id 固定为本窗口会话）。
        let session = self.session_id;
        let surface_tx = self.surface_tx.clone();
        let events: Arc<dyn Fn(SurfaceEventKind) + Send + Sync> = Arc::new(move |kind| {
            let _ = surface_tx.send(UiToHost::SurfaceEvent {
                session_id: session,
                kind,
            });
        });
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
        let rendered = render_surface_with_inputs(&self.content, &style, events, &input_widget);

        let mut panel = div()
            .id("flick-detached-panel")
            .w(px(DETACHED_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(metrics.radius_panel))
            .bg(col(ui.panel_bg))
            .text_color(col(ui.text))
            .p(px(DETACHED_PAD_V))
            // 卡片空白处拖动整个窗口。
            .on_mouse_down(MouseButton::Left, |_ev, window, _cx| {
                window.start_window_move()
            });
        if let Some(row) = search_row {
            panel = panel.child(row);
        }
        if let Some(row) = error_row {
            panel = panel.child(row);
        }
        panel = panel.child(
            div()
                .id("flick-detached-surface")
                .w_full()
                .flex_1()
                .overflow_y_scroll()
                // 内容区吞掉鼠标按下，避免在文字上拖动窗口。
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                )
                .child(rendered),
        );

        div()
            .id("flick-detached-root")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .track_focus(&self.focus)
            .key_context("FlickDetached")
            .on_action(cx.listener(Self::on_close))
            .child(panel)
    }
}

#[cfg(test)]
mod tests {
    // 注意：不要 `use super::*`——父模块的 `use gpui_kit::*` 会把 gpui 的 `test` 宏带进
    // 作用域，令内置 `#[test]` 解析到 gpui 宏而递归爆栈（与 crate 根 `theme_config_tests`
    // 同因）。只显式引入被测项。
    use super::{
        cascade_offset, detached_bounds, detached_size, DETACHED_MAX_HEIGHT, DETACHED_MIN_HEIGHT,
        DETACHED_WIDTH,
    };
    use crate::theme::ThemeMode;
    use flick_domain::{SurfaceContent, SurfaceNode, SURFACE_SCHEMA_VERSION};
    use flick_ui::style::SurfaceStyle;

    /// 1366x768 显示器：`(x, y, w, h)`。
    const DISPLAY_1366X768: (f32, f32, f32, f32) = (0.0, 0.0, 1366.0, 768.0);

    fn content(nodes: Vec<SurfaceNode>) -> SurfaceContent {
        SurfaceContent {
            schema_version: SURFACE_SCHEMA_VERSION,
            title: None,
            body: nodes,
        }
    }

    #[test]
    fn detached_size_clamps_to_min_and_max() {
        let style = SurfaceStyle::default();
        let (w, h) = detached_size(&content(vec![]), &style, false);
        assert_eq!(w, DETACHED_WIDTH);
        assert_eq!(h, DETACHED_MIN_HEIGHT);

        let many: Vec<SurfaceNode> = (0..200)
            .map(|i| SurfaceNode::Text {
                id: None,
                text: format!("line {i}"),
                muted: false,
            })
            .collect();
        let (_w, h2) = detached_size(&content(many), &style, false);
        assert_eq!(h2, DETACHED_MAX_HEIGHT);
    }

    fn medium_content() -> SurfaceContent {
        let nodes: Vec<SurfaceNode> = (0..10)
            .map(|i| SurfaceNode::Text {
                id: None,
                text: format!("line {i}"),
                muted: false,
            })
            .collect();
        content(nodes)
    }

    #[test]
    fn detached_size_adds_search_row_height() {
        let style = SurfaceStyle::default();
        let c = medium_content();
        let (_, without) = detached_size(&c, &style, false);
        let (_, with) = detached_size(&c, &style, true);
        assert!(without > DETACHED_MIN_HEIGHT && without < DETACHED_MAX_HEIGHT);
        assert_eq!(with, without + crate::PANEL_INPUT_HEIGHT);
    }

    /// 正常（小）内容 + index 0：窗口居中、`y = oy + 208`，不应被改动。
    #[test]
    fn detached_bounds_normal_case_stays_put() {
        let one = vec![SurfaceNode::Heading {
            id: None,
            text: "系统信息".into(),
        }];
        let (x, y, w, h) = detached_bounds(&content(one), ThemeMode::Dark, DISPLAY_1366X768, 0, false);
        assert_eq!(w, DETACHED_WIDTH);
        assert_eq!(h, DETACHED_MIN_HEIGHT); // 小内容被夹到最小高度
        // 水平居中：(1366 - 666) / 2 = 350。
        assert_eq!(x, (1366.0 - DETACHED_WIDTH) / 2.0);
        // y = 0 + 120 + 64 + 24 = 208，未越界，保留基准。
        assert_eq!(y, 208.0);
    }

    /// 超高内容在 1366x768 上：高度与 y 被夹取，窗口整体落在屏内（y + h <= 屏高）。
    #[test]
    fn detached_bounds_tall_content_clamps_y_and_height_to_fit() {
        let many: Vec<SurfaceNode> = (0..200)
            .map(|i| SurfaceNode::Text {
                id: None,
                text: format!("line {i}"),
                muted: false,
            })
            .collect();
        let (x, y, w, h) = detached_bounds(&content(many), ThemeMode::Dark, DISPLAY_1366X768, 0, false);
        assert_eq!(w, DETACHED_WIDTH);
        assert!(h <= DETACHED_MAX_HEIGHT);
        assert!(h <= 768.0);
        assert!(y >= 0.0);
        assert!(y + h <= 768.0, "window bottom {} exceeds screen", y + h);
        assert!(x >= 0.0 && x + w <= 1366.0);
    }

    /// 级联偏移把窗口推向右缘时：x 被夹回，窗口完整落在屏内。
    #[test]
    fn detached_bounds_cascade_near_right_edge_clamps_x() {
        // 800 宽：居中基准 x=(800-666)/2=67，index 7 级联 +168 => 越右缘。
        let display = (0.0f32, 0.0f32, 800.0f32, 600.0f32);
        let (x, y, w, h) = detached_bounds(&content(vec![]), ThemeMode::Dark, display, 7, false);
        assert_eq!(w, DETACHED_WIDTH);
        assert!(x >= 0.0, "x {x} < left edge");
        assert!(
            x + w <= 800.0,
            "window right {} exceeds 800",
            x + w
        );
        assert!(y >= 0.0 && y + h <= 600.0);
    }

    #[test]
    fn cascade_offset_wraps() {
        assert_eq!(cascade_offset(0), (0.0, 0.0));
        assert_eq!(cascade_offset(3), (72.0, 72.0));
        assert_eq!(cascade_offset(8), (0.0, 0.0));
    }

    #[test]
    fn detached_bounds_needs_search_adds_input_height() {
        let without = detached_bounds(
            &medium_content(),
            ThemeMode::Dark,
            DISPLAY_1366X768,
            0,
            false,
        );
        let with = detached_bounds(
            &medium_content(),
            ThemeMode::Dark,
            DISPLAY_1366X768,
            0,
            true,
        );
        assert_eq!(with.3, without.3 + crate::PANEL_INPUT_HEIGHT);
        assert_eq!(with.2, DETACHED_WIDTH);
    }
}
