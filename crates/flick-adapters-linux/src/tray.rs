//! 系统托盘（freedesktop StatusNotifierItem）适配。
//!
//! 只负责把 DBus 托盘事件翻译成 [`TrayEvent`]，不认识 daemon 的协调器与
//! 生命周期类型；图标复用内置 SVG（见 [`crate::flick_icon`]）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, Tray, TrayMethods};
use tokio::sync::mpsc::UnboundedSender;

/// 托盘图标在 DBus 上的稳定标识。
pub const TRAY_ID: &str = "flick";
/// 托盘图标的展示名称。
pub const TRAY_TITLE: &str = "Flick";
/// 托盘图标边长（像素）。
pub const ICON_SIZE: u32 = 32;
/// 应用版本号（取自 Cargo 包版本，用于「关于」菜单项展示）。
pub const TRAY_VERSION: &str = env!("CARGO_PKG_VERSION");

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
    /// 菜单项文本（部分项含动态内容，例如带版本号的「关于」）。
    pub label: String,
}

/// 返回托盘菜单结构（与设计文档一致）。
pub fn menu_spec() -> Vec<MenuItemSpec> {
    vec![
        // 有些桌面（如部分 GNOME 配置）不响应 SNI 左键点击，提供一个显式入口。
        MenuItemSpec {
            id: Some("show"),
            label: "显示".to_string(),
        },
        MenuItemSpec {
            id: None,
            label: String::new(),
        },
        MenuItemSpec {
            id: Some("home"),
            label: "主页".to_string(),
        },
        MenuItemSpec {
            id: Some("settings"),
            label: "设置".to_string(),
        },
        MenuItemSpec {
            id: Some("about"),
            label: format!("关于（v{TRAY_VERSION}）"),
        },
        MenuItemSpec {
            id: None,
            label: String::new(),
        },
        MenuItemSpec {
            id: Some("quit"),
            label: "退出".to_string(),
        },
    ]
}

/// 把菜单 id 映射成托盘事件；未知 id（含分隔线）返回 `None`。
pub fn map_menu_click(id: &str) -> Option<TrayEvent> {
    match id {
        "show" => Some(TrayEvent::Activate),
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

/// 渲染托盘图标（32px，ARGB32），结果进程内缓存。
pub fn render_tray_icon() -> Icon {
    static CACHE: OnceLock<Icon> = OnceLock::new();
    CACHE.get_or_init(render_tray_icon_uncached).clone()
}

/// 真正的光栅化：复用 `portal_identity` 相同的 resvg 渲染路径。
fn render_tray_icon_uncached() -> Icon {
    let size = ICON_SIZE;
    let blank = || Icon {
        width: size as i32,
        height: size as i32,
        data: Vec::new(),
    };

    let options = resvg::usvg::Options::default();
    let tree = match resvg::usvg::Tree::from_str(crate::flick_icon::FLICK_ICON_SVG, &options) {
        Ok(tree) => tree,
        Err(error) => {
            tracing::warn!(%error, "托盘图标解析失败，使用无图标");
            return blank();
        }
    };
    let mut pixmap = match resvg::tiny_skia::Pixmap::new(size, size) {
        Some(pixmap) => pixmap,
        None => {
            tracing::warn!("托盘图标画布创建失败");
            return blank();
        }
    };
    // 内置 SVG 的画布是 1024×1024，但内容（圆角方块）只占 112..912，
    // 直接等比缩放到 32px 会留下约 11% 的透明边，视觉上比别的托盘图标小一圈。
    // 因此按内容包围盒裁剪并居中，只保留 1px 呼吸位。
    let bbox = tree.root().abs_bounding_box();
    let pad = 1.0_f32;
    let inner = (size as f32 - 2.0 * pad).max(1.0);
    let (bw, bh) = (bbox.width(), bbox.height());
    let transform = if bw > 0.0 && bh > 0.0 && bw.is_finite() && bh.is_finite() {
        let scale = (inner / bw).min(inner / bh);
        let tx = pad + (inner - bw * scale) / 2.0 - bbox.x() * scale;
        let ty = pad + (inner - bh * scale) / 2.0 - bbox.y() * scale;
        resvg::tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, tx, ty)
    } else {
        resvg::tiny_skia::Transform::from_scale(size as f32 / 1024.0, size as f32 / 1024.0)
    };
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Icon {
        width: size as i32,
        height: size as i32,
        data: argb_from_rgba(size, size, pixmap.data()),
    }
}

/// 托盘启动失败的原因。
#[derive(Debug, thiserror::Error)]
pub enum TrayError {
    /// 通过 `FLICK_DISABLE_TRAY` 显式禁用。
    #[error("托盘已禁用（FLICK_DISABLE_TRAY）")]
    Disabled,
}

/// 是否允许启动托盘：`FLICK_DISABLE_TRAY` 未设置时启用。
pub fn tray_enabled() -> bool {
    std::env::var_os("FLICK_DISABLE_TRAY").is_none()
}

/// 托盘实现：把 ksni 回调翻译成 [`TrayEvent`] 发送给 daemon。
pub struct FlickTray {
    /// 事件出口（daemon 侧消费）。
    events: UnboundedSender<TrayEvent>,
    /// 保证「无 SNI 宿主」的告警只出现一次。
    warned: Arc<AtomicBool>,
}

impl Tray for FlickTray {
    fn id(&self) -> String {
        TRAY_ID.to_string()
    }

    fn title(&self) -> String {
        TRAY_TITLE.to_string()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![render_tray_icon()]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.events.send(TrayEvent::Activate);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        menu_spec()
            .into_iter()
            .map(|spec| match spec.id {
                None => MenuItem::Separator,
                Some(id) => {
                    let event = map_menu_click(id);
                    let label = spec.label;
                    MenuItem::from(StandardItem {
                        label,
                        enabled: true,
                        visible: true,
                        activate: Box::new(move |tray: &mut Self| {
                            if let Some(event) = event {
                                let _ = tray.events.send(event);
                            }
                        }),
                        ..StandardItem::default()
                    })
                }
            })
            .collect()
    }

    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        if !self.warned.swap(true, Ordering::SeqCst) {
            tracing::warn!(
                ?reason,
                "系统托盘不可用：GNOME 默认不显示 StatusNotifierItem，请安装 AppIndicator 扩展"
            );
        }
        // 返回 true 表示托盘服务继续等待宿主上线（不退出 daemon）。
        true
    }
}

/// 在当前 tokio 运行时上启动托盘。
///
/// - 返回 `Err(TrayError::Disabled)` 表示被环境变量禁用（调用方无需告警）。
/// - DBus 连接失败只在任务内 warn，不影响 daemon 其余功能。
pub fn spawn_tray(events: UnboundedSender<TrayEvent>) -> Result<(), TrayError> {
    if !tray_enabled() {
        return Err(TrayError::Disabled);
    }
    let tray = FlickTray {
        events,
        warned: Arc::new(AtomicBool::new(false)),
    };
    tokio::spawn(async move {
        // `assume_sni_available(true)` 把「无 StatusNotifierWatcher」降级成
        // `watcher_offline` 回调，避免 GNOME 未装扩展时 spawn 直接失败。
        match tray.assume_sni_available(true).spawn().await {
            Ok(handle) => {
                tracing::info!("系统托盘已启动");
                // 持有 handle 让托盘服务活到进程退出。
                let _keep_alive = handle;
                std::future::pending::<()>().await;
            }
            Err(error) => tracing::warn!(%error, "系统托盘启动失败，继续无托盘运行"),
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_icon_content_fills_most_of_canvas() {
        let icon = render_tray_icon();
        let side = icon.width as usize;
        let mut min_x = side;
        let mut min_y = side;
        let mut max_x = 0usize;
        let mut max_y = 0usize;
        for y in 0..side {
            for x in 0..side {
                let alpha = icon.data[(y * side + x) * 4];
                if alpha > 8 {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        let coverage_x = (max_x - min_x + 1) as f32 / side as f32;
        let coverage_y = (max_y - min_y + 1) as f32 / side as f32;
        // 不裁剪时只有约 0.81（SVG 画布自带 10.9% 透明边），裁剪后应达到 ~0.94。
        assert!(
            coverage_x >= 0.9 && coverage_y >= 0.9,
            "图标内容应基本铺满画布，实际 coverage_x={coverage_x}, coverage_y={coverage_y}"
        );
    }

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
        assert_eq!(spec.len(), 7, "5 个菜单项 + 2 条分隔线");
        let ids: Vec<Option<&str>> = spec.iter().map(|item| item.id).collect();
        assert_eq!(
            ids,
            vec![
                Some("show"),
                None,
                Some("home"),
                Some("settings"),
                Some("about"),
                None,
                Some("quit")
            ]
        );
        assert_eq!(spec[1].id, None, "「显示」下方必须是分隔线");
        assert_eq!(spec[5].id, None, "「退出」上方必须是分隔线");
    }

    #[test]
    fn map_menu_click_maps_known_ids() {
        assert_eq!(map_menu_click("show"), Some(TrayEvent::Activate));
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

    #[test]
    fn render_tray_icon_produces_argb_pixels() {
        let icon = render_tray_icon();
        assert_eq!(icon.width, ICON_SIZE as i32);
        assert_eq!(icon.height, ICON_SIZE as i32);
        assert_eq!(icon.data.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        assert!(icon.data.iter().any(|byte| *byte != 0), "图标不能是全透明");
    }

    #[test]
    fn render_tray_icon_is_deterministic() {
        let first = render_tray_icon();
        let second = render_tray_icon();
        assert_eq!(first.width, second.width);
        assert_eq!(first.height, second.height);
        assert_eq!(first.data, second.data, "同一 SVG 渲染结果必须一致");
    }

    #[test]
    fn disabled_env_switch_blocks_spawn() {
        // edition 2024：`set_var`/`remove_var` 是 unsafe；本用例独占该变量。
        unsafe { std::env::set_var("FLICK_DISABLE_TRAY", "1") };
        assert!(!tray_enabled());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let result = spawn_tray(tx);
        unsafe { std::env::remove_var("FLICK_DISABLE_TRAY") };
        assert!(matches!(result, Err(TrayError::Disabled)));
        assert!(tray_enabled());
    }

    #[test]
    fn activate_sends_event_through_channel() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tray = FlickTray {
            events: tx,
            warned: Arc::new(AtomicBool::new(false)),
        };
        tray.activate(0, 0);
        assert_eq!(rx.try_recv(), Ok(TrayEvent::Activate));
    }

    #[test]
    fn menu_items_carry_labels_and_dispatch_events() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tray = FlickTray {
            events: tx,
            warned: Arc::new(AtomicBool::new(false)),
        };
        let menu = tray.menu();
        assert_eq!(menu.len(), 7);
        let mut fired = Vec::new();
        for item in menu {
            if let ksni::MenuItem::Standard(item) = item {
                (item.activate)(&mut tray);
                if let Ok(event) = rx.try_recv() {
                    fired.push(event);
                }
            }
        }
        assert_eq!(
            fired,
            vec![
                TrayEvent::Activate,
                TrayEvent::Home,
                TrayEvent::Settings,
                TrayEvent::About,
                TrayEvent::Quit
            ]
        );
    }

    #[test]
    fn about_label_carries_version() {
        let about = menu_spec()
            .into_iter()
            .find(|spec| spec.id == Some("about"))
            .expect("「关于」项必须存在");
        assert_eq!(about.label, format!("关于（v{TRAY_VERSION}）"));
        assert!(about.label.contains(env!("CARGO_PKG_VERSION")));
    }
}
