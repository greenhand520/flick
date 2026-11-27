//! 系统托盘（freedesktop StatusNotifierItem）适配。
//!
//! 只负责把 DBus 托盘事件翻译成 [`TrayEvent`]，不认识 daemon 的协调器与
//! 生命周期类型；图标复用内置 SVG（见 [`crate::flick_icon`]）。

/// 托盘图标在 DBus 上的稳定标识。
pub const TRAY_ID: &str = "flick";
/// 托盘图标的展示名称。
pub const TRAY_TITLE: &str = "Flick";
/// 托盘图标边长（像素）。
pub const ICON_SIZE: u32 = 32;

/// 托盘图标触发的用户意图。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// 左键单击图标：唤起搜索面板。
    Activate,
    /// 菜单「主页」（占位）。
    Home,
    /// 菜单「设置…」（占位）。
    Settings,
    /// 菜单「关于」。
    About,
    /// 菜单「退出」。
    Quit,
}

/// 托盘菜单项描述：`id` 为 `None` 时表示分隔线。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItemSpec {
    /// 菜单项 id；`None` 表示分隔线。
    pub id: Option<&'static str>,
    /// 菜单项文本。
    pub label: &'static str,
}

/// 返回托盘菜单结构（与设计文档一致）。
pub fn menu_spec() -> Vec<MenuItemSpec> {
    vec![
        MenuItemSpec {
            id: Some("home"),
            label: "主页",
        },
        MenuItemSpec {
            id: Some("settings"),
            label: "设置…",
        },
        MenuItemSpec {
            id: Some("about"),
            label: "关于",
        },
        MenuItemSpec { id: None, label: "" },
        MenuItemSpec {
            id: Some("quit"),
            label: "退出",
        },
    ]
}

/// 把菜单 id 映射成托盘事件；未知 id（含分隔线）返回 `None`。
pub fn map_menu_click(id: &str) -> Option<TrayEvent> {
    match id {
        "home" => Some(TrayEvent::Home),
        "settings" => Some(TrayEvent::Settings),
        "about" => Some(TrayEvent::About),
        "quit" => Some(TrayEvent::Quit),
        _ => None,
    }
}

/// 把 RGBA8888 像素转成 SNI 要求的 ARGB32（network byte order）。
///
/// 长度不匹配时返回空 `Vec`（调用方应视为「无图标」）。
pub fn argb_from_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let expected = width as usize * height as usize * 4;
    if width == 0 || height == 0 || rgba.len() != expected {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(expected);
    for pixel in rgba.chunks_exact(4) {
        // [r, g, b, a] -> [a, r, g, b]，即 ARGB32 network byte order。
        out.push(pixel[3]);
        out.push(pixel[0]);
        out.push(pixel[1]);
        out.push(pixel[2]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argb_from_rgba_swaps_channels() {
        assert_eq!(
            argb_from_rgba(1, 1, &[10, 20, 30, 255]),
            vec![255, 10, 20, 30]
        );
    }

    #[test]
    fn argb_from_rgba_rejects_mismatched_length() {
        assert!(argb_from_rgba(2, 2, &[1, 2, 3, 4]).is_empty());
        assert!(argb_from_rgba(0, 0, &[]).is_empty());
    }

    #[test]
    fn menu_spec_matches_design() {
        let spec = menu_spec();
        assert_eq!(spec.len(), 5, "4 个菜单项 + 1 条分隔线");
        let ids: Vec<Option<&str>> = spec.iter().map(|item| item.id).collect();
        assert_eq!(
            ids,
            vec![
                Some("home"),
                Some("settings"),
                Some("about"),
                None,
                Some("quit")
            ]
        );
        assert_eq!(spec[3].id, None, "第 4 项必须是分隔线");
    }

    #[test]
    fn map_menu_click_maps_known_ids() {
        assert_eq!(map_menu_click("home"), Some(TrayEvent::Home));
        assert_eq!(map_menu_click("settings"), Some(TrayEvent::Settings));
        assert_eq!(map_menu_click("about"), Some(TrayEvent::About));
        assert_eq!(map_menu_click("quit"), Some(TrayEvent::Quit));
    }

    #[test]
    fn map_menu_click_ignores_unknown_id() {
        assert_eq!(map_menu_click("nope"), None);
        assert_eq!(map_menu_click(""), None);
    }
}
