//! 显示器布局探测：为外壳创建「铺满鼠标所在屏」的透明面板窗口提供精确的
//! **逻辑像素** 矩形。
//!
//! 为什么需要探测：gpui 的 Wayland 后端用 `wl_output` 的**整数**缩放因子换算
//! `Display::bounds()`，在分数缩放（如 2880×1800 @1.6667）下会把逻辑尺寸算小
//! （得到 1440×900，实际应为 1728×1080）。因此不能直接拿 `Display::bounds()` 当
//! 窗口尺寸，改为向合成器（GNOME/Mutter 的 `DisplayConfig`）查询真实逻辑布局。
//!
//! 多显示器策略：
//! - 用 XWayland 的指针坐标（`xdotool getmouselocation`）确定「鼠标所在屏」，
//!   借助 `xrandr --listmonitors` 把坐标映射到连接器名（`eDP-2`、`HDMI-1` ……）；
//! - 再用连接器名到 Mutter 的逻辑显示器列表里取该屏的逻辑原点与缩放比例，
//!   结合当前模式的物理宽高换算出逻辑宽高；
//! - 混合 DPI、负坐标、旋转（90°/270° 交换宽高）都在逻辑显示器这一层统一处理。
//!
//! 任一环节失败都返回 `None`，由调用方回退到「最大化」建窗方案。

use std::process::Command;

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
/// 通过 `gdbus` 读取 `org.gnome.Mutter.DisplayConfig.GetCurrentState`。这里刻意
/// 用文本输出而不是 zbus 结构化反序列化：该方法的签名里嵌了多层匿名结构，逐层
/// 定义 Rust 类型既啰嗦又容易随版本漂移，而 gdbus 的文本输出格式稳定且便于单测。
fn mutter_monitors() -> Vec<MonitorRect> {
    let output = match Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Mutter.DisplayConfig",
            "--object-path",
            "/org/gnome/Mutter/DisplayConfig",
            "--method",
            "org.gnome.Mutter.DisplayConfig.GetCurrentState",
        ])
        .output()
    {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_mutter_state(&String::from_utf8_lossy(&output.stdout))
}

/// 解析 `GetCurrentState` 的文本输出，得到每块屏的逻辑矩形。
///
/// 输入形如：
/// ```text
/// (uint32 2, [(('eDP-2', 'SDC', ...), [('2880x1800@120.001', 2880, 1800, ...
///   {'is-current': <true>, ...}), ...], {'is-builtin': <true>, ...}), ...],
///  [(0, 0, 1.6666666269302368, uint32 0, true, [('eDP-2', ...)], @a{sv} {}),
///   (1728, 0, 1.0, 0, false, [('HDMI-1', ...)], {})], {'layout-mode': <uint32 1>})
/// ```
/// 逻辑显示器段给出原点与缩放，物理宽高取自该屏「当前模式」（`is-current: true`）。
fn parse_mutter_state(text: &str) -> Vec<MonitorRect> {
    // 先按显示器块切分：每个块以 `(('连接器名'` 开始（逻辑显示器段里的连接器是单层括号）。
    let mut blocks: Vec<(usize, &str)> = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = text[cursor..].find("(('") {
        let start = cursor + offset;
        blocks.push((start, &text[start..]));
        cursor = start + 3;
    }

    // 「连接器 → 当前模式物理宽高」。
    let mut sizes: Vec<(String, f64, f64)> = Vec::new();
    for (index, (start, _block)) in blocks.iter().enumerate() {
        let end = blocks.get(index + 1).map(|(next, _)| *next).unwrap_or(text.len());
        let block = &text[*start..end];
        let _ = block;
        let Some(connector) = block.split('\'').nth(1) else {
            continue;
        };
        // 该块内 `is-current: <true>` 之前最近的模式元组就是当前模式。
        let Some(current_at) = block.find("'is-current': <true>") else {
            continue;
        };
        let prefix = &block[..current_at];
        let Some(mode_at) = prefix.rfind("('") else {
            continue;
        };
        let mut parts = prefix[mode_at..].split('\'');
        let _ = parts.next(); // "("
        let _ = parts.next(); // 模式 id
        let Some(rest) = parts.next() else {
            continue;
        };
        let numbers: Vec<&str> = rest.trim_start_matches(',').split(',').collect();
        let (Some(width), Some(height)) = (
            numbers.first().and_then(|v| v.trim().parse::<f64>().ok()),
            numbers.get(1).and_then(|v| v.trim().parse::<f64>().ok()),
        ) else {
            continue;
        };
        sizes.push((connector.to_string(), width, height));
    }

    // 再解析逻辑显示器段。
    let mut monitors = Vec::new();
    let Some(logical_start) = text.find("})], [(") else {
        return monitors;
    };
    let logical_text = &text[logical_start + 6..];
    let mut depth = 0i32;
    let mut entry_start: Option<usize> = None;
    let bytes = logical_text.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'(' => {
                if depth == 0 {
                    entry_start = Some(index);
                }
                depth += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(start) = entry_start.take() {
                        if let Some(monitor) = parse_logical_entry(&logical_text[start..=index], &sizes) {
                            monitors.push(monitor);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    monitors
}

/// 解析单个逻辑显示器元组 `(x, y, scale, transform, primary, [('连接器', ...)], props)`。
fn parse_logical_entry(entry: &str, sizes: &[(String, f64, f64)]) -> Option<MonitorRect> {
    let connector_at = entry.find("[('")?;
    let head = &entry[1..connector_at];
    let connector = entry[connector_at + 3..].split('\'').next()?.to_string();
    let tokens: Vec<&str> = head.split(',').collect();
    if tokens.len() < 4 {
        return None;
    }
    let x: f64 = tokens[0].trim().parse().ok()?;
    let y: f64 = tokens[1].trim().parse().ok()?;
    let scale: f64 = tokens[2].trim().parse().ok()?;
    let transform: u32 = tokens[3]
        .trim()
        .trim_start_matches("uint32 ")
        .parse()
        .ok()?;
    let (_, width, height) = sizes.iter().find(|(name, _, _)| *name == connector)?;
    if scale <= 0.0 {
        return None;
    }
    let (width, height) = logical_size(*width, *height, scale, transform);
    Some(MonitorRect {
        name: connector,
        x,
        y,
        width,
        height,
    })
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
    fn parses_mutter_state_with_mixed_scales() {
        let text = "(uint32 2, [(('eDP-2', 'SDC', '0x4190', '0x0'), [('2880x1800@120.001', 2880, 1800, 120.0, 1.6666666, [1.0, 1.5], {'is-current': <true>, 'is-preferred': <true>}), ('1440x900@60.0', 1440, 900, 60.0, 1.0, [1.0], {})], {'is-builtin': <true>}), (('HDMI-1', 'XMI', 'Redmi', '308'), [('3840x2160@60.0', 3840, 2160, 60.0, 2.0, [1.0, 2.0], {'is-current': <true>})], {'is-builtin': <false>})], [(0, 0, 1.6666666269302368, uint32 0, true, [('eDP-2', 'SDC', '0x4190', '0x0')], @a{sv} {}), (1728, 0, 2.0, 0, false, [('HDMI-1', 'XMI', 'Redmi', '308')], {})], {'layout-mode': <uint32 1>})";
        let monitors = parse_mutter_state(text);
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors[0].name, "eDP-2");
        assert!((monitors[0].width - 1728.0).abs() < 0.001);
        assert!((monitors[0].height - 1080.0).abs() < 0.001);
        assert_eq!(monitors[1].name, "HDMI-1");
        assert!((monitors[1].x - 1728.0).abs() < 0.001);
        assert!((monitors[1].width - 1920.0).abs() < 0.001);
        assert!((monitors[1].height - 1080.0).abs() < 0.001);
    }

    #[test]
    fn logical_size_divides_by_scale_and_swaps_on_rotation() {
        assert_eq!(logical_size(2880.0, 1800.0, 1.6666666, 1), (1728.0000, 1080.0000));
        let (w, h) = logical_size(2880.0, 1800.0, 2.0, 2);
        assert!((w - 900.0).abs() < 0.001 && (h - 1440.0).abs() < 0.001);
    }
}
