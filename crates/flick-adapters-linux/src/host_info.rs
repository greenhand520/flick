//! 宿主基础信息采集（Linux）：OS / 内核 / locale / shell / LAN IP。

fn read_trim(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// 采集宿主基础信息。
pub fn collect_host_info() -> flick_domain::HostInfo {
    let os = match std::env::consts::OS {
        "linux" => flick_domain::OperatingSystem::Linux,
        "windows" => flick_domain::OperatingSystem::Windows,
        "macos" => flick_domain::OperatingSystem::MacOs,
        _ => flick_domain::OperatingSystem::Other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => flick_domain::Architecture::X86_64,
        "aarch64" => flick_domain::Architecture::Aarch64,
        _ => flick_domain::Architecture::Other,
    };
    let kernel = read_trim("/proc/sys/kernel/osrelease");
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_else(|_| "en_US.UTF-8".into());
    let shell = match std::env::var("SHELL").unwrap_or_default() {
        s if s.ends_with("/zsh") => "zsh".to_string(),
        s if s.ends_with("/bash") => "bash".to_string(),
        _ => "other".to_string(),
    };
    let window_manager = detect_window_manager();
    let lan_ip = std::process::Command::new("hostname")
        .arg("-I")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split_whitespace().next().map(|s| s.to_string()));
    flick_domain::HostInfo {
        os,
        arch,
        kernel,
        locale,
        shell,
        window_manager,
        lan_ip,
    }
}

/// 根据桌面环境变量推断窗口管理器 / 合成器名称。
///
/// Linux 上没有统一接口查询当前合成器；按常见桌面环境映射：
/// GNOME（含 Ubuntu 的 `ubuntu:GNOME`）→ `Mutter`，KDE/Plasma → `KWin`，
/// Sway → `Sway`，Hyprland → `Hyprland`，Weston → `Weston`。无法识别返回 `None`。
fn detect_window_manager() -> Option<String> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let session = std::env::var("XDG_SESSION_DESKTOP").unwrap_or_default();
    let hay = format!("{desktop}:{session}").to_ascii_lowercase();
    let wm = if hay.contains("gnome") {
        "Mutter"
    } else if hay.contains("kde") || hay.contains("plasma") {
        "KWin"
    } else if hay.contains("hyprland") {
        "Hyprland"
    } else if hay.contains("sway") {
        "Sway"
    } else if hay.contains("weston") {
        "Weston"
    } else {
        return None;
    };
    Some(wm.to_string())
}
