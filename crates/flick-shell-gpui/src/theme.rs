//! 主题与尺寸 token（纯数据，不依赖 gpui）。

/// 面板与独立 Surface 窗口共用的逻辑宽度（单一来源）。
///
/// `Metrics::default().width` 与 `detached::DETACHED_WIDTH`
/// 全部派生自这里，避免三处字面量各写一份 666.0 而漂移。
pub const PANEL_WIDTH: f32 = 666.0;

/// 暗/亮主题。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
}

impl ThemeMode {
    /// 返回相反的模式。
    pub fn toggled(self) -> ThemeMode {
        match self {
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
        }
    }
}

/// 面板尺寸（逻辑像素），来源见设计 §4.1。
///
/// 注意：这里全部是“面板卡片内部”的尺寸；面板距屏幕顶部的位置由
/// `lib.rs::PANEL_TOP_MARGIN` 控制，不在这里。
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    /// 面板总宽度；结果行、输入行都按这个宽度布局。
    pub width: f32,
    /// 搜索输入行的高度（只是这一行的高度，不是距屏幕顶部的距离）。
    /// 空态时整个面板高度就等于它。
    pub input_height: f32,
    /// 单条结果行的高度。
    pub row_height: f32,
    /// 相邻两条结果行之间的垂直间距。
    pub row_gap: f32,
    /// 结果区 / Surface 区的最大高度；超过后内容区滚动。
    pub results_max_height: f32,
    /// 整个面板卡片的最大高度（输入行 + 间距 + 结果区，再受此值钳制）。
    pub panel_max_height: f32,
    /// 面板卡片圆角；视觉上取窗口圆角（26）的一半，故为 13。
    pub radius_panel: f32,
    /// 单条结果行的圆角。
    pub radius_row: f32,
    /// 面板左右内边距；控制内容（包括搜索图标）距离卡片左右边缘的距离。
    /// 放大镜到左边距离 = 此值 + `input_padding_h`（已覆盖 gpui 默认内边距）。
    pub padding_h: f32,
    /// 图标尺寸预留 token；当前查询行图标使用 gpui-kit 默认尺寸，尚未直接使用此值。
    pub icon_size: f32,
    /// 搜索行与下方结果区 / Surface 区之间的垂直间距。
    pub content_gap: f32,
    /// 搜索输入框自身（gpui Input）的左右内边距；用于覆盖 gpui 默认的 Large=12px，
    /// 在不改变字体大小的前提下把放大镜/文字拉近卡片边缘。
    pub input_padding_h: f32,
    /// 面板底部内边距；有结果 / Surface 时加在内容区下方，避免最后一行贴住卡片底边。
    pub padding_bottom: f32,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            width: PANEL_WIDTH,
            input_height: 64.0,
            row_height: 56.0,
            row_gap: 12.0,
            // 398 = panel_max_height 480 - input_height 64 - content_gap 8 - padding_bottom 10，
            // 使结果区上限与面板总高自洽（见 desired_size 的 clamp）。
            results_max_height: 398.0,
            panel_max_height: 480.0,
            // 要比启动的时候的26圆角小不少
            radius_panel: 16.0,
            radius_row: 14.0,
            padding_h: 12.0,
            icon_size: 24.0,
            content_gap: 8.0,
            input_padding_h: 4.0,
            padding_bottom: 12.0,
        }
    }
}

/// 一套主题调色板（RGBA，0-255）。
#[derive(Debug, Clone, Copy)]
pub struct UiTheme {
    /// 面板卡片主背景色；alpha 近 255 表示近不透明，不做真实背景模糊。
    pub panel_bg: (u8, u8, u8, u8),
    /// 结果行 / Surface 块的次要表面色。
    pub surface: (u8, u8, u8, u8),
    /// 主要文字颜色（搜索词、结果标题）。
    pub text: (u8, u8, u8, u8),
    /// 次要文字颜色（占位、辅助信息）。
    pub text_muted: (u8, u8, u8, u8),
    /// 选中结果行的背景色。
    pub selected_bg: (u8, u8, u8, u8),
    /// 选中结果行的文字颜色。
    pub selected_fg: (u8, u8, u8, u8),
    /// 鼠标悬停时的背景色。
    pub hover_bg: (u8, u8, u8, u8),
    /// 边框颜色；当前面板不画 border，保留给后续样式使用。
    pub border: (u8, u8, u8, u8),
    /// 阴影颜色；当前面板不画 shadow，保留给后续样式使用。
    pub shadow: (u8, u8, u8, u8),
}

/// 解析某模式的调色板与尺寸。
pub fn resolve(mode: ThemeMode) -> (UiTheme, Metrics) {
    let metrics = Metrics::default();
    let theme = match mode {
        ThemeMode::Dark => UiTheme {
            panel_bg: (20, 20, 24, 250),      // 近不透明深色（圆角矩形，无模糊）
            surface: (255, 255, 255, 10),     // rgba(255,255,255,.04)
            text: (230, 225, 229, 255),       // #E6E1E5
            text_muted: (154, 154, 160, 255), // #9A9AA0
            selected_bg: (208, 188, 255, 71), // rgba(208,188,255,.28)
            selected_fg: (234, 221, 255, 255),// #EADDFF
            hover_bg: (255, 255, 255, 20),    // rgba(255,255,255,.08)
            border: (255, 255, 255, 26),      // rgba(255,255,255,.10)
            shadow: (0, 0, 0, 87),            // rgba(0,0,0,.34)
        },
        ThemeMode::Light => UiTheme {
            panel_bg: (250, 250, 252, 250),   // 近不透明浅色
            surface: (255, 255, 255, 140),    // rgba(255,255,255,.55)
            text: (28, 27, 31, 255),          // #1C1B1F
            text_muted: (90, 90, 102, 255),   // #5A5A66
            selected_bg: (103, 80, 164, 41),  // rgba(103,80,164,.16)
            selected_fg: (33, 0, 93, 255),    // #21005D
            hover_bg: (0, 0, 0, 13),          // rgba(0,0,0,.05)
            border: (0, 0, 0, 26),            // rgba(0,0,0,.10)
            shadow: (0, 0, 0, 51),            // rgba(0,0,0,.20)
        },
    };
    (theme, metrics)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_is_default_and_toggles() {
        assert_eq!(ThemeMode::Dark.toggled(), ThemeMode::Light);
        assert_eq!(ThemeMode::Light.toggled(), ThemeMode::Dark);
    }

    #[test]
    fn resolve_returns_mode_specific_palette() {
        let (dark, _) = resolve(ThemeMode::Dark);
        let (light, _) = resolve(ThemeMode::Light);
        assert_eq!(dark.panel_bg, (20, 20, 24, 250));
        assert_eq!(light.panel_bg, (250, 250, 252, 250));
    }
}
