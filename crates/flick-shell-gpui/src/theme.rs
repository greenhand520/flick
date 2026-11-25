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
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    pub width: f32,
    pub input_height: f32,
    pub row_height: f32,
    pub row_gap: f32,
    pub results_max_height: f32,
    pub panel_max_height: f32,
    pub radius_panel: f32,
    pub radius_row: f32,
    pub padding_h: f32,
    pub icon_size: f32,
    pub content_gap: f32,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            width: PANEL_WIDTH,
            input_height: 64.0,
            row_height: 56.0,
            row_gap: 12.0,
            // 408 = panel_max_height 480 - input_height 64 - content_gap 8，
            // 使结果区上限与面板总高自洽（见 desired_size 的 clamp）。
            results_max_height: 408.0,
            panel_max_height: 480.0,
            // 要比启动的时候的26圆角小不少
            radius_panel: 13.0,
            radius_row: 14.0,
            padding_h: 22.0,
            icon_size: 24.0,
            content_gap: 8.0,
        }
    }
}

/// 一套主题调色板（RGBA，0-255）。
#[derive(Debug, Clone, Copy)]
pub struct UiTheme {
    pub panel_bg: (u8, u8, u8, u8),
    pub surface: (u8, u8, u8, u8),
    pub text: (u8, u8, u8, u8),
    pub text_muted: (u8, u8, u8, u8),
    pub selected_bg: (u8, u8, u8, u8),
    pub selected_fg: (u8, u8, u8, u8),
    pub hover_bg: (u8, u8, u8, u8),
    pub border: (u8, u8, u8, u8),
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

    #[test]
    fn metrics_match_design() {
        let m = Metrics::default();
        assert_eq!(m.width, 666.0);
        assert_eq!(m.input_height, 64.0);
        assert_eq!(m.row_height, 56.0);
        assert_eq!(m.results_max_height, 408.0);
        assert_eq!(m.panel_max_height, 480.0);
        // 搜索框卡片圆角 = 窗口圆角(26)的一半：gpui 的窗口圆角按物理像素(≈2x)应用，
        // 所以卡片用 13 才与窗口 26 视觉一致。
        assert_eq!(m.radius_panel, 13.0);
        assert_eq!(m.radius_row, 14.0);
        assert_eq!(m.padding_h, 22.0);
    }
}
