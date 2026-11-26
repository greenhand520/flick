//! 应用插件：启动时扫描 `.desktop`，搜索阶段返回应用 Execution 项。

// TODO(Task 5): AppsPlugin 接入后移除该 allow；当前扫描层仅被 Task 5 与测试使用。
#![allow(dead_code)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// 一个可启动的桌面应用条目（扫描 `.desktop` 得到）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopApp {
    /// 稳定标识，形如 `app:<desktop 文件名去扩展名>`。
    pub id: String,
    /// 展示名称（`Name` 字段）。
    pub name: String,
    /// 已去除 field codes（如 `%u`/`%f`）的启动命令。
    pub exec: String,
    /// 图标名（`Icon` 字段，可选）。
    pub icon: Option<String>,
    /// 搜索关键词（`Keywords` 字段按 `;` 拆分）。
    pub keywords: Vec<String>,
}

/// 根据环境变量推断应扫描的 applications 目录。
///
/// 优先使用 `FLICK_APPS_DIRS`（以 `:` 分隔，便于测试注入）；否则按 XDG 规范组合
/// `XDG_DATA_HOME`/`HOME` 与 `XDG_DATA_DIRS`。
pub(crate) fn desktop_dirs_from_env() -> Vec<PathBuf> {
    if let Ok(raw) = std::env::var("FLICK_APPS_DIRS") {
        return raw.split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    }
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(home).join("applications"));
    } else if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(
        data_dirs
            .split(':')
            .filter(|s| !s.is_empty())
            .map(|d| PathBuf::from(d).join("applications")),
    );
    dirs
}

/// 扫描给定目录（递归）下的 `.desktop` 文件，返回可见应用列表。
///
/// 按扫描顺序（即 `dirs` 顺序，对应 XDG 优先级）按 `id` 去重，**先出现的胜出**；
/// 随后按名称（忽略大小写）稳定排序。
pub(crate) fn scan_apps_from_dirs(dirs: &[PathBuf]) -> Vec<DesktopApp> {
    let mut apps = Vec::new();
    for dir in dirs {
        scan_dir(dir, &mut apps);
    }
    let mut seen = HashSet::new();
    apps.retain(|app| seen.insert(app.id.clone()));
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps
}

/// 递归扫描单个目录并收集应用。
fn scan_dir(dir: &Path, out: &mut Vec<DesktopApp>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("desktop") {
            if let Some(app) = parse_desktop(&path) {
                out.push(app);
            }
        }
    }
}

/// 解析单个 `.desktop` 文件；跳过非 Application、`NoDisplay=true`、`Hidden=true` 的条目。
///
/// 只读取 `[Desktop Entry]` 组内的键，忽略 `[Desktop Action …]` 等其它组，
/// 避免子动作覆盖主条目的 Name/Exec。
fn parse_desktop(path: &Path) -> Option<DesktopApp> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut name = None;
    let mut exec = None;
    let mut icon = None;
    let mut keywords = Vec::new();
    let mut no_display = false;
    let mut hidden = false;
    let mut is_application = false;
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // 组标题行，如 `[Desktop Entry]` / `[Desktop Action new-window]`。
        if line.starts_with('[') && line.ends_with(']') {
            in_entry = &line[1..line.len() - 1] == "Desktop Entry";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        match key.trim() {
            "Type" => is_application = value.trim() == "Application",
            "Name" => name = Some(value.trim().to_string()),
            "Exec" => exec = Some(strip_field_codes(value.trim())),
            "Icon" => icon = Some(value.trim().to_string()),
            "Keywords" => {
                keywords = value
                    .split(';')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            }
            "NoDisplay" => no_display = value.trim().eq_ignore_ascii_case("true"),
            "Hidden" => hidden = value.trim().eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
    if !is_application || no_display || hidden {
        return None;
    }
    let name = name?;
    let exec = exec?;
    if exec.is_empty() {
        return None;
    }
    let stem = path.file_stem()?.to_string_lossy();
    let id = format!("app:{stem}");
    Some(DesktopApp { id, name, exec, icon, keywords })
}

/// 去掉 Exec 中的 field codes（以 `%` 开头的 token，如 `%u`、`%f`、`%F`）。
fn strip_field_codes(exec: &str) -> String {
    exec.split_whitespace()
        .filter(|token| !token.starts_with('%'))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_desktop(dir: &std::path::Path, file: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(file), body).unwrap();
    }

    #[test]
    fn scans_visible_application_entries() {
        let dir = tempfile::tempdir().unwrap();
        let apps_dir = dir.path().join("applications");
        write_desktop(&apps_dir, "firefox.desktop", "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox %u\nIcon=firefox\nKeywords=web;browser;\n");
        write_desktop(&apps_dir, "hidden.desktop", "[Desktop Entry]\nType=Application\nName=Hidden\nExec=hidden\nNoDisplay=true\n");
        let apps = scan_apps_from_dirs(&[apps_dir]);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "Firefox");
        assert_eq!(apps[0].exec, "firefox");
        assert_eq!(apps[0].keywords, vec!["web", "browser"]);
    }

    /// 回归：`[Desktop Action …]` 组不得覆盖 `[Desktop Entry]` 的 Name/Exec。
    #[test]
    fn ignores_desktop_action_groups() {
        let dir = tempfile::tempdir().unwrap();
        let apps_dir = dir.path().join("applications");
        write_desktop(
            &apps_dir,
            "firefox.desktop",
            "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox %u\nIcon=firefox\n\n[Desktop Action new-window]\nName=Open a New Window\nExec=firefox --new-window %u\n",
        );
        let apps = scan_apps_from_dirs(&[apps_dir]);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "Firefox");
        assert_eq!(apps[0].exec, "firefox");
    }

    /// 跨目录按 id 去重：先扫描的目录（XDG 高优先级）胜出。
    #[test]
    fn dedups_across_dirs_first_wins() {
        let dir = tempfile::tempdir().unwrap();
        let high = dir.path().join("high/applications");
        let low = dir.path().join("low/applications");
        // 名称故意与目录顺序相反：若「先排序后 dedup」则低优先级会胜出，本测试可捕获。
        write_desktop(&high, "aaa.desktop", "[Desktop Entry]\nType=Application\nName=Zeta\nExec=first\n");
        write_desktop(&low, "aaa.desktop", "[Desktop Entry]\nType=Application\nName=Alpha\nExec=second\n");
        write_desktop(&low, "bbb.desktop", "[Desktop Entry]\nType=Application\nName=Bbb\nExec=bbb\n");
        let apps = scan_apps_from_dirs(&[high, low]);
        assert_eq!(apps.len(), 2);
        let aaa = apps.iter().find(|a| a.id == "app:aaa").unwrap();
        assert_eq!(aaa.name, "Zeta");
        assert_eq!(aaa.exec, "first");
        assert!(apps.iter().any(|a| a.id == "app:bbb"));
    }

    /// `Hidden=true` 与非 Application 条目应被跳过。
    #[test]
    fn skips_hidden_and_non_application() {
        let dir = tempfile::tempdir().unwrap();
        let apps_dir = dir.path().join("applications");
        write_desktop(&apps_dir, "visible.desktop", "[Desktop Entry]\nType=Application\nName=Visible\nExec=visible\n");
        write_desktop(&apps_dir, "hidden.desktop", "[Desktop Entry]\nType=Application\nName=Hidden\nExec=hidden\nHidden=true\n");
        write_desktop(&apps_dir, "link.desktop", "[Desktop Entry]\nType=Link\nName=Link\nExec=link\n");
        let apps = scan_apps_from_dirs(&[apps_dir]);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "Visible");
    }
}
