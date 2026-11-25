//! Ctrl+D 分离出来的 Surface 独立窗口视图。
//!
//! 内容为分离那一刻的 `SurfaceContent` 快照；不订阅 daemon 事件、不跟随后续搜索。
//! 窗口的创建与销毁由 `lib.rs` 的应用级事件泵统一管理：本视图只负责渲染，并在
//! Esc 时通过 `close_tx` 请求泵在窗口更新帧之外 `remove_window`。

use flick_domain::SurfaceContent;
use flick_ui::render::{col, render_surface};
use flick_ui::style::{estimate_height, SurfaceStyle};
use gpui_kit::*;
use tokio::sync::mpsc;

use crate::theme::{self, ThemeMode};
use crate::view::CloseDetached;

/// 独立窗口逻辑宽度（与搜索面板一致）。
pub const DETACHED_WIDTH: f32 = 666.0;
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

/// 由 Surface 内容计算独立窗口尺寸（宽, 高）。
pub fn detached_size(content: &SurfaceContent) -> (f32, f32) {
    let content_h = estimate_height(content, &SurfaceStyle::default());
    let h = (content_h + DETACHED_PAD_V * 2.0).clamp(DETACHED_MIN_HEIGHT, DETACHED_MAX_HEIGHT);
    (DETACHED_WIDTH, h)
}

/// 第 `index` 个独立窗口相对基准位置的级联偏移。
pub fn cascade_offset(index: usize) -> (f32, f32) {
    let n = (index % DETACHED_CASCADE_WRAP) as f32 * DETACHED_CASCADE_STEP;
    (n, n)
}

/// 独立 Surface 窗口视图。
pub struct DetachedSurfaceView {
    content: SurfaceContent,
    theme: ThemeMode,
    focus: FocusHandle,
    close_tx: mpsc::UnboundedSender<AnyWindowHandle>,
}

impl DetachedSurfaceView {
    pub fn new(
        content: SurfaceContent,
        theme: ThemeMode,
        close_tx: mpsc::UnboundedSender<AnyWindowHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            content,
            theme,
            focus,
            close_tx,
        }
    }

    /// 主题广播：更新配色并请求重绘。
    pub fn set_theme(&mut self, theme: ThemeMode, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    fn on_close(&mut self, _: &CloseDetached, window: &mut Window, _cx: &mut Context<Self>) {
        // 交给应用级泵在窗口更新帧之外销毁，避免 "window not found"。
        let _ = self.close_tx.send(window.window_handle());
    }
}

impl Render for DetachedSurfaceView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let (ui, metrics) = theme::resolve(self.theme);
        let style = SurfaceStyle {
            panel_bg: ui.panel_bg,
            surface: ui.surface,
            text: ui.text,
            text_muted: ui.text_muted,
            border: ui.border,
            radius_row: metrics.radius_row,
            ..SurfaceStyle::default()
        };

        div()
            .id("flick-detached-root")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .track_focus(&self.focus)
            .key_context("FlickDetached")
            .on_action(_cx.listener(Self::on_close))
            .child(
                div()
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
                    })
                    .child(
                        div()
                            .id("flick-detached-surface")
                            .w_full()
                            .flex_1()
                            .overflow_y_scroll()
                            // 内容区吞掉鼠标按下，避免在文字上拖动窗口。
                            .on_mouse_down(
                                MouseButton::Left,
                                _cx.listener(|_, _, _, cx| cx.stop_propagation()),
                            )
                            .child(render_surface(&self.content, &style)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    // 注意：不要 `use super::*`——父模块的 `use gpui_kit::*` 会把 gpui 的 `test` 宏带进
    // 作用域，令内置 `#[test]` 解析到 gpui 宏而递归爆栈（与 crate 根 `theme_config_tests`
    // 同因）。只显式引入被测项。
    use super::{cascade_offset, detached_size, DETACHED_MAX_HEIGHT, DETACHED_MIN_HEIGHT, DETACHED_WIDTH};
    use flick_domain::{SurfaceContent, SurfaceNode, SURFACE_SCHEMA_VERSION};

    fn content(nodes: Vec<SurfaceNode>) -> SurfaceContent {
        SurfaceContent {
            schema_version: SURFACE_SCHEMA_VERSION,
            title: None,
            body: nodes,
        }
    }

    #[test]
    fn detached_size_clamps_to_min_and_max() {
        let (w, h) = detached_size(&content(vec![]));
        assert_eq!(w, DETACHED_WIDTH);
        assert_eq!(h, DETACHED_MIN_HEIGHT);

        let many: Vec<SurfaceNode> = (0..200)
            .map(|i| SurfaceNode::Text {
                id: None,
                text: format!("line {i}"),
                muted: false,
            })
            .collect();
        let (_w, h2) = detached_size(&content(many));
        assert_eq!(h2, DETACHED_MAX_HEIGHT);
    }

    #[test]
    fn cascade_offset_wraps() {
        assert_eq!(cascade_offset(0), (0.0, 0.0));
        assert_eq!(cascade_offset(3), (72.0, 72.0));
        assert_eq!(cascade_offset(8), (0.0, 0.0));
    }
}
