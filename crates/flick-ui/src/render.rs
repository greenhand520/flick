//! Surface 的 gpui 渲染器（仅 `feature = "gpui"`）。

use flick_domain::{BadgeTone, SurfaceContent, SurfaceNode};
use gpui_kit::component::label::Label;
use gpui_kit::*;

use crate::style::SurfaceStyle;

fn col((r, g, b, a): (u8, u8, u8, u8)) -> Rgba {
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

/// 把一个 Surface 渲染为 gpui 元素树。
pub fn render_surface(content: &SurfaceContent, style: &SurfaceStyle) -> AnyElement {
    let mut root = div().flex().flex_col().w_full().gap(px(style.section_gap));
    if let Some(title) = &content.title {
        root = root.child(
            div().w_full().text_color(col(style.text)).child(Label::new(title.clone())),
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
            .child(Label::new(text.clone()))
            .into_any_element(),
        SurfaceNode::Text { text, muted, .. } => {
            let color = if *muted { style.text_muted } else { style.text };
            div()
                .w_full()
                .text_color(col(color))
                .child(Label::new(text.clone()))
                .into_any_element()
        }
        SurfaceNode::KeyValue { key, value, .. } => div()
            .flex()
            .w_full()
            .justify_between()
            .gap(px(style.row_gap))
            .child(div().text_color(col(style.text_muted)).child(Label::new(key.clone())))
            .child(div().text_color(col(style.text)).child(Label::new(value.clone())))
            .into_any_element(),
        SurfaceNode::Badge { text, tone, .. } => div()
            .flex()
            .child(
                div()
                    .rounded_full()
                    .px_2()
                    .py_1()
                    .bg(col(tone_color(style, *tone)))
                    .text_color(col(style.text))
                    .child(Label::new(text.clone())),
            )
            .into_any_element(),
        SurfaceNode::List { items, .. } => {
            let mut el = div().flex().flex_col().w_full().gap(px(style.row_gap));
            for item in items {
                let mut row = div()
                    .flex()
                    .w_full()
                    .justify_between()
                    .child(div().text_color(col(style.text)).child(Label::new(item.title.clone())));
                if let Some(v) = &item.value {
                    row = row.child(div().text_color(col(style.text_muted)).child(Label::new(v.clone())));
                }
                el = el.child(row);
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
                el = el.child(div().text_color(col(style.text)).child(Label::new(t.clone())));
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
