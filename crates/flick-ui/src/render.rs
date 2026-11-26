//! Surface 的 gpui 渲染器（仅 `feature = "gpui"`）。

use std::sync::Arc;

use flick_domain::{BadgeTone, SurfaceContent, SurfaceEventKind, SurfaceNode};
use gpui_kit::component::label::Label;
use gpui_kit::*;

use crate::style::{SurfaceStyle, truncate_chars};

/// 普通文本（正文 / value / subtitle）的字符上限。
const TEXT_MAX_CHARS: usize = 200;
/// 标题 / 键 / 徽标等短文本的字符上限。
const TITLE_MAX_CHARS: usize = 80;

/// 把 `(r,g,b,a)` 四元组转换为 gpui `Rgba`（供渲染层共用）。
pub fn col((r, g, b, a): (u8, u8, u8, u8)) -> Rgba {
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: a as f32 / 255.0,
    }
}

fn tone_color(style: &SurfaceStyle, tone: BadgeTone) -> (u8, u8, u8, u8) {
    match tone {
        BadgeTone::Neutral => style.badge_neutral,
        BadgeTone::Info => style.badge_info,
        BadgeTone::Success => style.badge_success,
        BadgeTone::Warning => style.badge_warning,
        BadgeTone::Danger => style.badge_danger,
    }
}

/// 徽标前景色：按底色亮度选深/浅文字，浅底用深色以保证对比度。
fn badge_fg(style: &SurfaceStyle, tone: BadgeTone) -> (u8, u8, u8, u8) {
    let (r, g, b, _) = tone_color(style, tone);
    let luma = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    if luma > 140.0 {
        style.panel_bg
    } else {
        style.text
    }
}

/// 渲染一个徽标（Badge 节点与 List item 共用）。
fn render_badge(text: String, tone: BadgeTone, style: &SurfaceStyle) -> AnyElement {
    div()
        .flex()
        .child(
            div()
                .rounded_full()
                .px_2()
                .py_1()
                .bg(col(tone_color(style, tone)))
                .text_color(col(badge_fg(style, tone)))
                .child(Label::new(truncate_chars(&text, TITLE_MAX_CHARS))),
        )
        .into_any_element()
}

/// 一个可交互文本节点（Input / TextArea）的元信息，供外壳注入真实可编辑控件。
///
/// `render_surface_with_inputs` 会为每个 Input/TextArea 节点调用
/// `input_widget`，外壳据此返回一个可编辑的 `Input` 元素；返回 `None`
/// 时退回只读渲染（例如该节点尚未在控件缓存中建立）。
#[derive(Debug, Clone, Copy)]
pub struct InputWidgetSpec<'a> {
    pub id: &'a str,
    pub value: &'a str,
    pub placeholder: Option<&'a str>,
    pub multiline: bool,
    pub rows: Option<u16>,
}

/// 把一个 Surface 渲染为只读 gpui 元素树（旧 API，等价于无事件、无输入控件）。
pub fn render_surface(content: &SurfaceContent, style: &SurfaceStyle) -> AnyElement {
    render_surface_with_events(content, style, Arc::new(|_| {}))
}

/// 渲染 Surface，并把交互控件（Button / Checkbox / ListItem 行）的点击事件
/// 通过 `events` 回调上报；Input/TextArea 用只读样式渲染。
pub fn render_surface_with_events(
    content: &SurfaceContent,
    style: &SurfaceStyle,
    events: Arc<dyn Fn(SurfaceEventKind) + Send + Sync>,
) -> AnyElement {
    render_surface_with_inputs(content, style, events, &|_| None)
}

/// 渲染 Surface，既上报交互事件，又把 Input/TextArea 交给 `input_widget` 注入
/// 真实可编辑控件（外壳的 gpui Input）。`input_widget` 返回 `None` 时退回只读样式。
pub fn render_surface_with_inputs(
    content: &SurfaceContent,
    style: &SurfaceStyle,
    events: Arc<dyn Fn(SurfaceEventKind) + Send + Sync>,
    input_widget: &dyn Fn(&InputWidgetSpec) -> Option<AnyElement>,
) -> AnyElement {
    let mut root = div().flex().flex_col().w_full().gap(px(style.section_gap));
    if let Some(title) = &content.title {
        root = root.child(
            div()
                .w_full()
                .text_color(col(style.text))
                .child(Label::new(truncate_chars(title, TITLE_MAX_CHARS))),
        );
    }
    for node in &content.body {
        root = root.child(render_node_with(node, style, &events, input_widget));
    }
    root.into_any_element()
}

fn render_node(node: &SurfaceNode, style: &SurfaceStyle) -> AnyElement {
    match node {
        SurfaceNode::Heading { text, .. } => div()
            .w_full()
            .text_color(col(style.text))
            .child(Label::new(truncate_chars(text, TITLE_MAX_CHARS)))
            .into_any_element(),
        SurfaceNode::Text { text, muted, .. } => {
            let color = if *muted { style.text_muted } else { style.text };
            div()
                .w_full()
                .text_color(col(color))
                .child(Label::new(truncate_chars(text, TEXT_MAX_CHARS)))
                .into_any_element()
        }
        SurfaceNode::KeyValue { key, value, .. } => div()
            .flex()
            .w_full()
            .justify_between()
            .gap(px(style.row_gap))
            .child(
                div()
                    .text_color(col(style.text_muted))
                    .child(Label::new(truncate_chars(key, TITLE_MAX_CHARS))),
            )
            .child(
                div()
                    .text_color(col(style.text))
                    .child(Label::new(truncate_chars(value, TEXT_MAX_CHARS))),
            )
            .into_any_element(),
        SurfaceNode::Badge { text, tone, .. } => render_badge(text.clone(), *tone, style),
        SurfaceNode::List { items, .. } => {
            let mut el = div().flex().flex_col().w_full().gap(px(style.row_gap));
            for item in items {
                let mut entry = div().flex().flex_col().w_full();
                let mut top = div().flex().w_full().justify_between().child(
                    div()
                        .text_color(col(style.text))
                        .child(Label::new(truncate_chars(&item.title, TITLE_MAX_CHARS))),
                );
                if let Some(v) = &item.value {
                    top = top.child(
                        div()
                            .text_color(col(style.text_muted))
                            .child(Label::new(truncate_chars(v, TEXT_MAX_CHARS))),
                    );
                }
                entry = entry.child(top);
                if let Some(sub) = &item.subtitle {
                    entry = entry.child(
                        div()
                            .text_sm()
                            .text_color(col(style.text_muted))
                            .child(Label::new(truncate_chars(sub, TEXT_MAX_CHARS))),
                    );
                }
                if let Some(badge) = &item.badge {
                    entry = entry.child(render_badge(badge.clone(), BadgeTone::Neutral, style));
                }
                el = el.child(entry);
            }
            el.into_any_element()
        }
        SurfaceNode::Divider { .. } => div()
            .h(px(1.0))
            .w_full()
            .bg(col(style.border))
            .into_any_element(),
        SurfaceNode::Spacer { size, .. } => {
            let h = match size {
                flick_domain::SpacerSize::Sm => 6.0,
                flick_domain::SpacerSize::Md => 12.0,
                flick_domain::SpacerSize::Lg => 24.0,
            };
            div().h(px(h)).w_full().into_any_element()
        }
        SurfaceNode::Section {
            title, children, ..
        } => {
            let mut el = div().flex().flex_col().w_full().gap(px(style.section_gap));
            if let Some(t) = title {
                el = el.child(
                    div()
                        .text_color(col(style.text))
                        .child(Label::new(truncate_chars(t, TITLE_MAX_CHARS))),
                );
            }
            for c in children {
                el = el.child(render_node(c, style));
            }
            el.into_any_element()
        }
        SurfaceNode::Row { children, .. } => {
            let mut el = div().flex().w_full().gap(px(style.row_gap));
            for c in children {
                el = el.child(render_node(c, style));
            }
            el.into_any_element()
        }
        SurfaceNode::Column { children, .. } => {
            let mut el = div().flex().flex_col().w_full().gap(px(style.section_gap));
            for c in children {
                el = el.child(render_node(c, style));
            }
            el.into_any_element()
        }
        SurfaceNode::Input {
            value, placeholder, ..
        } => {
            let empty = value.is_empty();
            let shown = if empty {
                placeholder.clone().unwrap_or_default()
            } else {
                value.clone()
            };
            let color = if empty { style.text_muted } else { style.text };
            div()
                .flex()
                .items_center()
                .w_full()
                .h(px(40.0))
                .px(px(style.pad))
                .rounded(px(style.radius_row))
                .border_1()
                .border_color(col(style.border))
                .bg(col(style.surface))
                .text_color(col(color))
                .child(Label::new(truncate_chars(&shown, TEXT_MAX_CHARS)))
                .into_any_element()
        }
        SurfaceNode::TextArea {
            value,
            placeholder,
            rows,
            ..
        } => {
            let empty = value.is_empty();
            let shown = if empty {
                placeholder.clone().unwrap_or_default()
            } else {
                value.clone()
            };
            let color = if empty { style.text_muted } else { style.text };
            let row_count = (*rows).unwrap_or(3).max(1) as usize;
            let mut el = div()
                .flex()
                .flex_col()
                .w_full()
                .h(px(style.line_height * row_count as f32))
                .px(px(style.pad))
                .py(px(4.0))
                .rounded(px(style.radius_row))
                .border_1()
                .border_color(col(style.border))
                .bg(col(style.surface))
                .text_color(col(color))
                .overflow_hidden();
            for line in shown.split('\n') {
                el = el.child(Label::new(truncate_chars(line, TEXT_MAX_CHARS)));
            }
            el.into_any_element()
        }
        SurfaceNode::Button { label, tone, .. } => {
            let (bg, fg) = style.button_colors(*tone);
            div()
                .flex()
                .items_center()
                .justify_center()
                .w_full()
                .h(px(40.0))
                .px(px(style.pad))
                .rounded(px(style.radius_row))
                .bg(col(bg))
                .text_color(col(fg))
                .child(Label::new(truncate_chars(label, TITLE_MAX_CHARS)))
                .into_any_element()
        }
        SurfaceNode::Checkbox { label, checked, .. } => {
            let mark = if *checked { "☑" } else { "☐" };
            div()
                .flex()
                .items_center()
                .gap(px(style.row_gap))
                .w_full()
                .h(px(40.0))
                .text_color(col(style.text))
                .child(div().text_color(col(style.text)).child(Label::new(mark)))
                .child(
                    div()
                        .text_color(col(style.text))
                        .child(Label::new(truncate_chars(label, TEXT_MAX_CHARS))),
                )
                .into_any_element()
        }
    }
}

/// 与 [`render_node`] 相同，但交互节点会挂上事件回调，Input/TextArea 交由
/// `input_widget` 注入真实控件（返回 None 时退回 [`render_node`] 的只读渲染）。
fn render_node_with(
    node: &SurfaceNode,
    style: &SurfaceStyle,
    events: &Arc<dyn Fn(SurfaceEventKind) + Send + Sync>,
    input_widget: &dyn Fn(&InputWidgetSpec) -> Option<AnyElement>,
) -> AnyElement {
    match node {
        SurfaceNode::Input {
            id,
            value,
            placeholder,
        } => {
            let spec = InputWidgetSpec {
                id,
                value,
                placeholder: placeholder.as_deref(),
                multiline: false,
                rows: None,
            };
            match input_widget(&spec) {
                Some(el) => el,
                None => render_node(node, style),
            }
        }
        SurfaceNode::TextArea {
            id,
            value,
            placeholder,
            rows,
        } => {
            let spec = InputWidgetSpec {
                id,
                value,
                placeholder: placeholder.as_deref(),
                multiline: true,
                rows: *rows,
            };
            match input_widget(&spec) {
                Some(el) => el,
                None => render_node(node, style),
            }
        }
        SurfaceNode::Button { id, label, tone } => {
            let (bg, fg) = style.button_colors(*tone);
            let events = events.clone();
            let node_id = id.clone();
            div()
                .flex()
                .items_center()
                .justify_center()
                .w_full()
                .h(px(40.0))
                .px(px(style.pad))
                .rounded(px(style.radius_row))
                .bg(col(bg))
                .text_color(col(fg))
                .child(Label::new(truncate_chars(label, TITLE_MAX_CHARS)))
                .on_mouse_down(MouseButton::Left, move |_ev, _window, _cx| {
                    events(SurfaceEventKind::Click {
                        node_id: node_id.clone(),
                    });
                })
                .into_any_element()
        }
        SurfaceNode::Checkbox { id, label, checked } => {
            let mark = if *checked { "☑" } else { "☐" };
            let events = events.clone();
            let node_id = id.clone();
            let next = !*checked;
            div()
                .flex()
                .items_center()
                .gap(px(style.row_gap))
                .w_full()
                .h(px(40.0))
                .text_color(col(style.text))
                .child(div().text_color(col(style.text)).child(Label::new(mark)))
                .child(
                    div()
                        .text_color(col(style.text))
                        .child(Label::new(truncate_chars(label, TEXT_MAX_CHARS))),
                )
                .on_mouse_down(MouseButton::Left, move |_ev, _window, _cx| {
                    events(SurfaceEventKind::Toggled {
                        node_id: node_id.clone(),
                        checked: next,
                    });
                })
                .into_any_element()
        }
        SurfaceNode::List { items, .. } => {
            let mut el = div().flex().flex_col().w_full().gap(px(style.row_gap));
            for item in items {
                let mut entry = div().flex().flex_col().w_full();
                let mut top = div().flex().w_full().justify_between().child(
                    div()
                        .text_color(col(style.text))
                        .child(Label::new(truncate_chars(&item.title, TITLE_MAX_CHARS))),
                );
                if let Some(v) = &item.value {
                    top = top.child(
                        div()
                            .text_color(col(style.text_muted))
                            .child(Label::new(truncate_chars(v, TEXT_MAX_CHARS))),
                    );
                }
                entry = entry.child(top);
                if let Some(sub) = &item.subtitle {
                    entry = entry.child(
                        div()
                            .text_sm()
                            .text_color(col(style.text_muted))
                            .child(Label::new(truncate_chars(sub, TEXT_MAX_CHARS))),
                    );
                }
                if let Some(badge) = &item.badge {
                    entry = entry.child(render_badge(badge.clone(), BadgeTone::Neutral, style));
                }
                if let Some(id) = &item.id {
                    let events = events.clone();
                    let node_id = id.clone();
                    entry = entry.on_mouse_down(MouseButton::Left, move |_ev, _window, _cx| {
                        events(SurfaceEventKind::Click {
                            node_id: node_id.clone(),
                        });
                    });
                }
                el = el.child(entry);
            }
            el.into_any_element()
        }
        SurfaceNode::Section {
            title, children, ..
        } => {
            let mut el = div().flex().flex_col().w_full().gap(px(style.section_gap));
            if let Some(t) = title {
                el = el.child(
                    div()
                        .text_color(col(style.text))
                        .child(Label::new(truncate_chars(t, TITLE_MAX_CHARS))),
                );
            }
            for c in children {
                el = el.child(render_node_with(c, style, events, input_widget));
            }
            el.into_any_element()
        }
        SurfaceNode::Row { children, .. } => {
            let mut el = div().flex().w_full().gap(px(style.row_gap));
            for c in children {
                el = el.child(render_node_with(c, style, events, input_widget));
            }
            el.into_any_element()
        }
        SurfaceNode::Column { children, .. } => {
            let mut el = div().flex().flex_col().w_full().gap(px(style.section_gap));
            for c in children {
                el = el.child(render_node_with(c, style, events, input_widget));
            }
            el.into_any_element()
        }
        _ => render_node(node, style),
    }
}
