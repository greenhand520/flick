//! 显示器布局探测：为外壳创建「铺满鼠标所在屏」的透明面板窗口提供精确的
//! **逻辑像素** 矩形。
//!
//! 为什么需要探测：gpui 的 Wayland 后端用 `wl_output` 的**整数**缩放因子换算
//! `Display::bounds()`，在分数缩放（如 2880×1800 @1.6667）下会把逻辑尺寸算小
//! （得到 1440×900，实际应为 1728×1080）。因此不能直接拿 `Display::bounds()` 当
//! 窗口尺寸，改为向合成器（GNOME/Mutter 的 `DisplayConfig`）查询真实逻辑布局。
//!
//! 多显示器策略（全部进程内完成，不 fork 任何外部命令）：
//! - 用 `x11rb` 的 `QueryPointer` 读 XWayland 根窗口指针坐标，再用 `x11rb` 的
//!   RandR `GetMonitors` 把坐标映射到连接器名（`eDP-2`、`HDMI-1` ……）；
//! - 用 `zbus` 调 Mutter 的 `DisplayConfig.GetCurrentState`，按连接器取该屏的
//!   逻辑原点与缩放比例，结合当前模式的物理宽高换算出逻辑宽高；
//! - 混合 DPI、负坐标、旋转（90°/270° 交换宽高）都在逻辑显示器这一层统一处理。
//!
//! 指针仍要借 XWayland：Wayland 协议本身不提供全局指针查询。任一环节失败都
//! 返回 `None`，由调用方回退到「最大化」建窗方案。


/// 一块显示器在逻辑坐标系中的矩形（单位：逻辑像素）。
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorRect {
    /// 连接器名，例如 `eDP-2`、`HDMI-1`。
    pub name: String,
    /// 逻辑原点的 X 坐标（可能为负值）。
    pub x: f64,
    /// 逻辑原点的 Y 坐标（可能为负值）。
    pub y: f64,
    /// 逻辑宽度。
    pub width: f64,
    /// 逻辑高度。
    pub height: f64,
}

/// 查询「鼠标所在屏」的逻辑矩形；探测失败返回 `None`。
pub fn pointer_monitor() -> Option<MonitorRect> {
    let (x, y) = pointer_position()?;
    let monitors = mutter_monitors();
    if !monitors.is_empty() {
        // 先按连接器名对齐：X 坐标 → 连接器 → Mutter 逻辑矩形。
        let x11 = x11_monitors();
        let connector = pick_monitor(&x11, x, y).map(|m| m.name.clone());
        if let Some(connector) = connector {
            if let Some(monitor) = monitors.iter().find(|m| m.name == connector) {
                return Some(monitor.clone());
            }
        }
        // 连接器对不上时，退化为「逻辑坐标包含指针」的那块屏。
        if let Some(monitor) = monitors.iter().find(|m| contains(m, x, y)) {
            return Some(monitor.clone());
        }
        return monitors.into_iter().next();
    }
    // 没有 Mutter（非 GNOME）时，只能返回 X 坐标系的矩形。
    pick_monitor(&x11_monitors(), x, y).cloned()
}

/// 判断逻辑矩形是否包含给定坐标（右/下边界开区间）。
fn contains(rect: &MonitorRect, x: f64, y: f64) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

/// 在显示器列表里挑出包含指针的那块。
fn pick_monitor(monitors: &[MonitorRect], x: f64, y: f64) -> Option<&MonitorRect> {
    monitors.iter().find(|m| contains(m, x, y))
}

/// 读取 X 指针位置（XWayland 根窗口坐标）。
///
/// 直接用 `x11rb` 在进程内查询，不再 fork `xdotool`：Wayland 协议本身不提供
/// 全局指针查询，只能借 XWayland（GNOME 为 X11 应用起的兼容 X server）拿坐标。
fn pointer_position() -> Option<(f64, f64)> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::ConnectionExt as _;

    let (connection, screen_num) = x11rb::connect(None).ok()?;
    let root = connection.setup().roots.get(screen_num)?.root;
    let pointer = connection.query_pointer(root).ok()?.reply().ok()?;
    Some((f64::from(pointer.root_x), f64::from(pointer.root_y)))
}

/// 读取 XRandR 的显示器列表（XWayland 坐标系，仅用于把指针映射到连接器名）。
fn x11_monitors() -> Vec<MonitorRect> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::ConnectionExt as _;

    let Ok((connection, screen_num)) = x11rb::connect(None) else {
        return Vec::new();
    };
    let Some(root) = connection.setup().roots.get(screen_num).map(|s| s.root) else {
        return Vec::new();
    };
    let Ok(cookie) = connection.randr_get_monitors(root, true) else {
        return Vec::new();
    };
    let Ok(reply) = cookie.reply() else {
        return Vec::new();
    };
    reply
        .monitors
        .iter()
        .map(|monitor| MonitorRect {
            name: connection
                .get_atom_name(monitor.name)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .map(|reply| String::from_utf8_lossy(&reply.name).into_owned())
                .unwrap_or_default(),
            x: f64::from(monitor.x),
            y: f64::from(monitor.y),
            width: f64::from(monitor.width),
            height: f64::from(monitor.height),
        })
        .collect()
}

/// 查询 Mutter 的逻辑显示器列表（逻辑像素）；不可用时返回空列表。
///
/// 走 zbus 直接调用 `org.gnome.Mutter.DisplayConfig.GetCurrentState`，不 fork
/// `gdbus`。返回值的签名是
/// `(u32, a((ssss)a(siiddada{sv})a{sv}), a(iiduba(ssss)a{sv}), a{sv})`，
/// 下面按字段顺序定义对应结构体。
fn mutter_monitors() -> Vec<MonitorRect> {
    let Ok(connection) = zbus::blocking::Connection::session() else {
        return Vec::new();
    };
    let Ok(reply) = connection.call_method(
        Some("org.gnome.Mutter.DisplayConfig"),
        "/org/gnome/Mutter/DisplayConfig",
        Some("org.gnome.Mutter.DisplayConfig"),
        "GetCurrentState",
        &(),
    ) else {
        return Vec::new();
    };
    let body = reply.body();
    let Ok(state) = body.deserialize::<MutterState>() else {
        return Vec::new();
    };
    state_to_monitors(&state)
}

/// `GetCurrentState` 的返回结构。
/// 该结构体字段逐一对应 DBus 返回元组，部分字段只用于对齐签名。
#[allow(dead_code)]
#[derive(serde::Deserialize, zbus::zvariant::Type)]
struct MutterState(
    u32,
    Vec<MutterMonitorSpec>,
    Vec<MutterLogicalMonitor>,
    std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
);

/// 单块显示器的规格：身份四元组、支持的模式列表、属性。
/// 该结构体字段逐一对应 DBus 返回元组，部分字段只用于对齐签名。
#[allow(dead_code)]
#[derive(serde::Deserialize, zbus::zvariant::Type)]
struct MutterMonitorSpec(
    (String, String, String, String),
    Vec<MutterMode>,
    std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
);

/// 单个显示模式：id、物理宽高、刷新率、首选缩放、支持的缩放列表、属性。
/// 该结构体字段逐一对应 DBus 返回元组，部分字段只用于对齐签名。
#[allow(dead_code)]
#[derive(serde::Deserialize, zbus::zvariant::Type)]
struct MutterMode(
    String,
    i32,
    i32,
    f64,
    f64,
    Vec<f64>,
    std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
);

/// 逻辑显示器：原点、缩放、旋转、是否主屏、所辖显示器身份、属性。
/// 该结构体字段逐一对应 DBus 返回元组，部分字段只用于对齐签名。
#[allow(dead_code)]
#[derive(serde::Deserialize, zbus::zvariant::Type)]
struct MutterLogicalMonitor(
    i32,
    i32,
    f64,
    u32,
    bool,
    Vec<(String, String, String, String)>,
    std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
);

/// 把结构化结果换算成逻辑矩形（物理宽高 / 缩放，旋转 90°/270° 时交换）。
fn state_to_monitors(state: &MutterState) -> Vec<MonitorRect> {
    let sizes: Vec<(String, f64, f64)> = state
        .1
        .iter()
        .filter_map(|spec| {
            let connector = spec.0 .0.clone();
            let mode = spec
                .1
                .iter()
                .find(|mode| is_current(&mode.6))
                .or_else(|| spec.1.first())?;
            Some((connector, f64::from(mode.1), f64::from(mode.2)))
        })
        .collect();

    let mut monitors = Vec::new();
    for logical in &state.2 {
        let Some(connector) = logical.5.first().map(|identity| identity.0.clone()) else {
            continue;
        };
        let Some((_, width, height)) = sizes.iter().find(|(name, _, _)| *name == connector) else {
            continue;
        };
        if logical.2 <= 0.0 {
            continue;
        }
        let (width, height) = logical_size(*width, *height, logical.2, logical.3);
        monitors.push(MonitorRect {
            name: connector,
            x: f64::from(logical.0),
            y: f64::from(logical.1),
            width,
            height,
        });
    }
    monitors
}

/// 属性字典里是否有 `is-current: true`（标记该屏当前使用的模式）。
fn is_current(properties: &std::collections::HashMap<String, zbus::zvariant::OwnedValue>) -> bool {
    properties
        .get("is-current")
        .and_then(|value| bool::try_from(value).ok())
        .unwrap_or(false)
}

/// 由物理宽高、缩放比例与旋转（1..=8）算出逻辑宽高。
fn logical_size(width: f64, height: f64, scale: f64, transform: u32) -> (f64, f64) {
    // 缩放换算会带出 `1728.0000411` 这类浮点噪声，取整到逻辑像素。
    let (width, height) = ((width / scale).round(), (height / scale).round());
    // 2/4/6/8 表示旋转 90°/270°（含镜像），此时宽高互换。
    if matches!(transform, 2 | 4 | 6 | 8) {
        (height, width)
    } else {
        (width, height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_monitor_containing_pointer_with_negative_origin() {
        let monitors = vec![
            MonitorRect {
                name: "left".into(),
                x: -1920.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
            MonitorRect {
                name: "main".into(),
                x: 0.0,
                y: 0.0,
                width: 2560.0,
                height: 1440.0,
            },
        ];
        assert_eq!(pick_monitor(&monitors, -10.0, 100.0).unwrap().name, "left");
        assert_eq!(pick_monitor(&monitors, 2559.0, 1439.0).unwrap().name, "main");
        assert!(pick_monitor(&monitors, 2560.0, 0.0).is_none());
    }

    #[test]
    fn logical_size_divides_by_scale_and_swaps_on_rotation() {
        assert_eq!(logical_size(2880.0, 1800.0, 1.6666666, 1), (1728.0000, 1080.0000));
        let (w, h) = logical_size(2880.0, 1800.0, 2.0, 2);
        assert!((w - 900.0).abs() < 0.001 && (h - 1440.0).abs() < 0.001);
    }
}
