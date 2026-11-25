//! Surface 样式（纯数据，不依赖 gpui）与高度估算。

use flick_domain::{SurfaceContent, SurfaceNode, SpacerSize};

/// 一套 Surface 呈现样式（颜色 + 尺寸）。默认值仅为高度估算服务时使用。
#[derive(Debug, Clone, Copy)]
pub struct SurfaceStyle {
    pub panel_bg: (u8, u8, u8, u8),
    pub surface: (u8, u8, u8, u8),
    pub text: (u8, u8, u8, u8),
    pub text_muted: (u8, u8, u8, u8),
    pub border: (u8, u8, u8, u8),
    pub badge_neutral: (u8, u8, u8, u8),
    pub badge_info: (u8, u8, u8, u8),
    pub badge_success: (u8, u8, u8, u8),
    pub badge_warning: (u8, u8, u8, u8),
    pub badge_danger: (u8, u8, u8, u8),
    pub radius_row: f32,
    pub pad: f32,
    pub row_gap: f32,
    pub line_height: f32,
    pub section_gap: f32,
}

impl Default for SurfaceStyle {
    fn default() -> Self {
        Self {
            panel_bg: (20, 20, 24, 250),
            surface: (255, 255, 255, 10),
            text: (230, 225, 229, 255),
            text_muted: (154, 154, 160, 255),
            border: (255, 255, 255, 26),
            badge_neutral: (154, 154, 160, 255),
            badge_info: (120, 180, 255, 255),
            badge_success: (120, 200, 140, 255),
            badge_warning: (240, 190, 90, 255),
            badge_danger: (240, 120, 120, 255),
            radius_row: 14.0,
            pad: 10.0,
            row_gap: 8.0,
            line_height: 22.0,
            section_gap: 10.0,
        }
    }
}

/// 按字符（而非字节）安全截断文本：超长时保留前 `max - 1` 个字符再追加 `…`，
/// 使最终长度不超过 `max` 个字符。
///
/// 仅在 UTF-8 字符边界切分，绝不会从多字节字符中间切断，也不会 panic。
/// 渲染层用它防止超长文本撑破布局。
pub fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// 估算 Surface 的原始高度（不做面板 max clamp；调用方再 clamp）。
pub fn estimate_height(content: &SurfaceContent, style: &SurfaceStyle) -> f32 {
    let mut total = 0.0;
    for (i, node) in content.body.iter().enumerate() {
        if i > 0 {
            total += style.section_gap;
        }
        total += node_height(node, style);
    }
    total
}

fn node_height(node: &SurfaceNode, style: &SurfaceStyle) -> f32 {
    match node {
        SurfaceNode::Heading { .. } => style.line_height * 1.25,
        SurfaceNode::Text { text, .. } => {
            let lines = ((text.chars().count() as f32) / 28.0).ceil().max(1.0);
            style.line_height * lines
        }
        SurfaceNode::KeyValue { .. } => style.line_height,
        SurfaceNode::Badge { .. } => style.line_height,
        SurfaceNode::List { items, .. } => {
            let rows: f32 = items
                .iter()
                .map(|it| {
                    style.line_height * if it.subtitle.is_some() { 1.6 } else { 1.0 }
                        + if it.badge.is_some() { style.line_height } else { 0.0 }
                })
                .sum();
            rows + (items.len().saturating_sub(1) as f32) * style.row_gap
        }
        SurfaceNode::Divider { .. } => 1.0,
        SurfaceNode::Spacer { size, .. } => match size {
            SpacerSize::Sm => 6.0,
            SpacerSize::Md => 12.0,
            SpacerSize::Lg => 24.0,
        },
        SurfaceNode::Section { title, children, .. } => {
            let mut h = 0.0;
            if title.is_some() {
                h += style.line_height * 1.1 + style.section_gap;
            }
            for (i, c) in children.iter().enumerate() {
                if i > 0 {
                    h += style.section_gap;
                }
                h += node_height(c, style);
            }
            h
        }
        SurfaceNode::Row { children, .. } => children
            .iter()
            .map(|c| node_height(c, style))
            .fold(0.0_f32, f32::max),
        SurfaceNode::Column { children, .. } => {
            children.iter().map(|c| node_height(c, style)).sum::<f32>()
                + (children.len().saturating_sub(1) as f32) * style.section_gap
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flick_domain::{SurfaceContent, SurfaceNode};

    fn content(body: Vec<SurfaceNode>) -> SurfaceContent {
        SurfaceContent { schema_version: 1, title: None, body }
    }

    #[test]
    fn truncate_keeps_short_text_unchanged() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("", 0), "");
    }

    #[test]
    fn truncate_at_exact_limit_is_unchanged() {
        let s = "a".repeat(10);
        assert_eq!(truncate_chars(&s, 10), s);
    }

    #[test]
    fn truncate_long_ascii_appends_ellipsis_within_limit() {
        let s = "a".repeat(50);
        let out = truncate_chars(&s, 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.ends_with('…'));
        assert_eq!(out, format!("{}…", "a".repeat(9)));
    }

    #[test]
    fn truncate_is_char_safe_for_multibyte() {
        // 每个汉字 3 字节；按字节切会 panic / 产生乱码，这里必须按字符切。
        let s = "系统信息".repeat(10); // 40 个字符
        let out = truncate_chars(&s, 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('…'));
        // 输出仍是合法 UTF-8（String 本身保证），且没有替换字符。
        assert!(!out.contains('\u{FFFD}'));
    }

    #[test]
    fn empty_content_is_zero_height() {
        assert_eq!(estimate_height(&content(vec![]), &SurfaceStyle::default()), 0.0);
    }

    #[test]
    fn more_items_are_taller() {
        let one = content(vec![SurfaceNode::Heading { id: None, text: "a".into() }]);
        let three = content(vec![
            SurfaceNode::Heading { id: None, text: "a".into() },
            SurfaceNode::KeyValue { id: None, key: "k".into(), value: "v".into() },
            SurfaceNode::Text { id: None, text: "hello".into(), muted: false },
        ]);
        let s = SurfaceStyle::default();
        assert!(estimate_height(&three, &s) > estimate_height(&one, &s));
    }

    #[test]
    fn list_grows_per_item() {
        let s = SurfaceStyle::default();
        let item = flick_domain::ListItem { title: "t".into(), subtitle: None, value: None, badge: None };
        let a = content(vec![SurfaceNode::List { id: None, items: vec![item.clone()] }]);
        let b = content(vec![SurfaceNode::List { id: None, items: vec![item.clone(), item] }]);
        assert!(estimate_height(&b, &s) > estimate_height(&a, &s));
    }
}
