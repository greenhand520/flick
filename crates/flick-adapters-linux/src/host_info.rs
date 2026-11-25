//! 宿主基础信息采集（Linux）：OS / 内核 / locale / shell / LAN IP。

fn read_trim(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// 采集宿主基础信息。
pub fn collect_host_info() -> flick_domain::HostInfo {
    let os = std::env::consts::OS.to_string();
    let kernel = read_trim("/proc/sys/kernel/osrelease");
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_else(|_| "en_US.UTF-8".into());
    let shell = match std::env::var("SHELL").unwrap_or_default() {
        s if s.ends_with("/zsh") => "zsh".to_string(),
        s if s.ends_with("/bash") => "bash".to_string(),
        _ => "other".to_string(),
    };
    let lan_ip = std::process::Command::new("hostname")
        .arg("-I")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split_whitespace().next().map(|s| s.to_string()));
    flick_domain::HostInfo {
        os,
        kernel,
        locale,
        shell,
        lan_ip,
    }
}
