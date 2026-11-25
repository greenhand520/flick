//! Surface 的 gpui 渲染器（仅 `feature = "gpui"`）。

use flick_domain::{BadgeTone, SurfaceContent, SurfaceNode};
use gpui_kit::component::label::Label;
use gpui_kit::*;

use crate::style::{truncate_chars, SurfaceStyle};

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
    if luma > 140.0 { style.panel_bg } else { style.text }
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

/// 把一个 Surface 渲染为 gpui 元素树。
pub fn render_surface(content: &SurfaceContent, style: &SurfaceStyle) -> AnyElement {
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
        root = root.child(render_node(node, style));
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
                let mut top = div()
                    .flex()
                    .w_full()
                    .justify_between()
                    .child(
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
        SurfaceNode::Section { title, children, .. } => {
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
    }
}
