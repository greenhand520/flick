//! GPUI 输入适配：把路径/剪贴板条目转成框架无关的 `SearchInputSnapshot`。
//! 本模块只做纯函数，不读窗口、不启动任务；副作用集中在 `clipboard_watch.rs` 与 `view.rs`。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use flick_domain::{FileInput, ImageData, InputSource, SearchInputSnapshot};
use gpui_kit::{ClipboardEntry, ClipboardItem};

/// 为普通文件/目录构造 `FileInput`；不存在时返回 `None`。
pub(crate) fn file_input_from_path(path: &Path) -> Option<FileInput> {
    let metadata = std::fs::metadata(path).ok()?;
    let display_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());
    let mime = extension
        .as_deref()
        .and_then(mime_for_extension)
        .map(str::to_owned);
    Some(FileInput {
        uri: file_uri(path),
        display_name,
        extension: metadata.is_file().then(|| extension.clone()).flatten(),
        is_dir: metadata.is_dir(),
        mime,
    })
}

/// 把本地路径编码成 `file://` URI；只转义空格、`%`、`#`、`?`。
fn file_uri(path: &Path) -> String {
    let encoded = path
        .to_string_lossy()
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('#', "%23")
        .replace('?', "%3F");
    format!("file://{encoded}")
}

/// 解析 `file://` URI 为本地路径，并处理 `%20` 等 percent-encode。
fn path_from_file_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file://localhost/mnt/x` 与 `file:///mnt/x` 都应解析为 `/mnt/x`；
    // 其他 host 形式（如 `file://server/share`）在 Linux 本地文件场景不处理。
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    Some(PathBuf::from(percent_decode(rest)))
}

/// 轻量 percent-decode（仅用于 `file://` URI；非法序列原样保留）。
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = (bytes[index + 1] as char).to_digit(16);
            let low = (bytes[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                output.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(output).unwrap_or_else(|_| input.to_string())
}

/// 判断整段剪贴板文本是否是一组真实存在的文件/文件夹路径。
///
/// Linux 文件管理器复制文件时，Wayland 平台层没有把 `text/uri-list`
/// 暴露成 `ClipboardEntry::ExternalPaths`，而是作为 `text/plain` 路径返回；
/// 这里识别这种路径文本并转为文件附件，避免搜索框里出现原始 `file://` URI。
/// 支持每行一个绝对路径或 `file://` URI；任何一行不是现有路径则整体按文本处理。
fn paths_from_clipboard_text(text: &str) -> Option<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let path = if line.starts_with("file://") {
            path_from_file_uri(line)?
        } else {
            let path = PathBuf::from(line);
            if !path.is_absolute() {
                return None;
            }
            path
        };
        if !path.exists() {
            return None;
        }
        paths.push(path);
    }
    (!paths.is_empty()).then_some(paths)
}

/// 小范围扩展名 → MIME 映射；未知扩展名返回 `None`。
fn mime_for_extension(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "txt" | "md" | "rs" | "toml" | "json" => "text/plain",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => return None,
    })
}

/// 从本地路径列表构造文件快照；过滤掉已不存在的路径。
pub(crate) fn snapshot_from_paths(
    paths: &[PathBuf],
    source: InputSource,
) -> Option<SearchInputSnapshot> {
    let files: Vec<FileInput> = paths
        .iter()
        .filter_map(|p| file_input_from_path(p))
        .collect();
    (!files.is_empty()).then(|| SearchInputSnapshot::files(files, source))
}

/// 从剪贴板条目构造快照：图片优先，其次文件，最后文本。
///
/// 图片与文件不进入 Input 文本；文本返回后由 `view.rs` 的 `InputEvent::Change`
/// 以 `InputSource::Paste` 派发，保证受控输入与光标行为一致。
pub(crate) fn snapshot_from_clipboard_item(
    item: &ClipboardItem,
    source: InputSource,
) -> Option<SearchInputSnapshot> {
    for entry in &item.entries {
        if let ClipboardEntry::Image(image) = entry {
            return Some(SearchInputSnapshot::clipboard_image(
                ImageData {
                    mime: image.format.mime_type().to_string(),
                    bytes: image.bytes.clone(),
                },
                source,
            ));
        }
    }
    for entry in &item.entries {
        if let ClipboardEntry::ExternalPaths(paths) = entry {
            if let Some(snapshot) = snapshot_from_paths(paths.paths(), source) {
                return Some(snapshot);
            }
        }
    }
    if item
        .entries
        .iter()
        .any(|entry| matches!(entry, ClipboardEntry::ExternalPaths(_)))
    {
        // 有外部路径条目但路径已不存在时，不要把拼接后的路径字符串当文本插入。
        return None;
    }
    item.text().and_then(|text| {
        if let Some(paths) = paths_from_clipboard_text(&text) {
            return snapshot_from_paths(&paths, source);
        }
        (!text.is_empty()).then(|| SearchInputSnapshot::text(text, source))
    })
}

/// 计算剪贴板内容指纹，用于判定“5 秒内是否发生变化”。
pub(crate) fn clipboard_fingerprint(item: &ClipboardItem) -> u64 {
    let mut hasher = DefaultHasher::new();
    for entry in &item.entries {
        match entry {
            ClipboardEntry::String(text) => {
                0u8.hash(&mut hasher);
                text.text.hash(&mut hasher);
            }
            ClipboardEntry::Image(image) => {
                1u8.hash(&mut hasher);
                image.format.mime_type().hash(&mut hasher);
                image.bytes.hash(&mut hasher);
            }
            ClipboardEntry::ExternalPaths(paths) => {
                2u8.hash(&mut hasher);
                paths.paths().hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

/// 附件 chip 的短标签：文件名主体最多 10 个 Unicode 字符，超出用 `…` 截断；
/// 有扩展名时保留 `.ext`，文件夹不追加扩展名。
///
/// 例：`flick-搜索匹配算法.pdf` → `flick-搜索匹配….pdf`（前 10 个字符）。
pub(crate) fn display_label(file: &FileInput) -> String {
    const NAME_PREFIX_CHARS: usize = 10;

    let extension = file
        .extension
        .as_deref()
        .filter(|ext| !ext.is_empty() && !file.is_dir);
    // 文件名的展示主体不含扩展名；若 `display_name` 末尾不是期望扩展名，
    // 仍按扩展名切出可读主体，避免出现 `….pdf.pdf`。
    let stem = extension
        .and_then(|ext| {
            let suffix = format!(".{ext}");
            file.display_name.strip_suffix(&suffix).or_else(|| {
                file.display_name
                    .rsplit_once('.')
                    .filter(|(_, actual)| actual.eq_ignore_ascii_case(ext))
                    .map(|(stem, _)| stem)
            })
        })
        .unwrap_or(&file.display_name);

    let mut chars = stem.chars();
    let prefix: String = chars.by_ref().take(NAME_PREFIX_CHARS).collect();
    let mut label = prefix;
    if chars.next().is_some() {
        label.push('…');
    }
    if let Some(ext) = extension {
        label.push('.');
        label.push_str(ext);
    }
    label
}

#[cfg(test)]
mod tests {
    use super::*;
    use flick_domain::UiInputKind;
    use gpui_kit::ClipboardString;

    #[test]
    fn absolute_path_builds_file_input() {
        let dir = std::env::temp_dir().join(format!("flick-input-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hello world.txt");
        std::fs::write(&file, b"x").unwrap();

        let input = file_input_from_path(&file).unwrap();
        assert_eq!(input.display_name, "hello world.txt");
        assert_eq!(input.extension.as_deref(), Some("txt"));
        assert_eq!(input.mime.as_deref(), Some("text/plain"));
        assert!(!input.is_dir);
        assert_eq!(
            input.uri,
            format!("file://{}", file.display()).replace(' ', "%20")
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clipboard_text_becomes_text_snapshot() {
        let item = ClipboardItem {
            entries: vec![ClipboardEntry::String(ClipboardString::new("hi".into()))],
        };
        let snapshot = snapshot_from_clipboard_item(&item, InputSource::Paste).unwrap();
        assert_eq!(snapshot.kind, UiInputKind::Text);
        assert_eq!(snapshot.text, "hi");
        assert_eq!(snapshot.source, InputSource::Paste);
    }

    #[test]
    fn clipboard_text_file_path_becomes_files_snapshot() {
        let dir = std::env::temp_dir().join(format!("flick-input-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("截图 2026-08-17.png");
        std::fs::write(&file, b"x").unwrap();

        let item = ClipboardItem::new_string(file.to_string_lossy().into_owned());
        let snapshot = snapshot_from_clipboard_item(&item, InputSource::Paste).unwrap();
        assert_eq!(snapshot.kind, UiInputKind::Files);
        assert_eq!(snapshot.files.len(), 1);
        assert_eq!(snapshot.files[0].display_name, "截图 2026-08-17.png");
        assert!(!snapshot.files[0].is_dir);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clipboard_text_folder_path_is_kept_as_directory() {
        let dir = std::env::temp_dir().join(format!("flick-input-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let item = ClipboardItem::new_string(dir.to_string_lossy().into_owned());
        let snapshot = snapshot_from_clipboard_item(&item, InputSource::Paste).unwrap();
        assert_eq!(snapshot.kind, UiInputKind::Files);
        assert!(snapshot.files[0].is_dir);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clipboard_text_nonexistent_path_remains_text() {
        let item = ClipboardItem::new_string("/no/such/flick-path-xyz".into());
        let snapshot = snapshot_from_clipboard_item(&item, InputSource::Paste).unwrap();
        assert_eq!(snapshot.kind, UiInputKind::Text);
        assert_eq!(snapshot.text, "/no/such/flick-path-xyz");
    }

    #[test]
    fn file_uri_is_decoded_to_local_path() {
        let path = path_from_file_uri("file:///tmp/flick%20a%23b%3Fc%25d.png").unwrap();
        assert_eq!(path, std::path::PathBuf::from("/tmp/flick a#b?c%d.png"));
    }

    #[test]
    fn fingerprint_changes_with_content() {
        let a = ClipboardItem::new_string("a".into());
        let b = ClipboardItem::new_string("b".into());
        assert_ne!(clipboard_fingerprint(&a), clipboard_fingerprint(&b));
    }

    #[test]
    fn display_label_truncates_stem_and_keeps_extension() {
        let file = FileInput {
            uri: "file:///tmp/x".into(),
            display_name: "flick-搜索匹配算法.pdf".into(),
            extension: Some("pdf".into()),
            is_dir: false,
            mime: Some("application/pdf".into()),
        };
        assert_eq!(display_label(&file), "flick-搜索匹配….pdf");
    }

    #[test]
    fn display_label_truncates_folder_without_extension() {
        let folder = FileInput {
            uri: "file:///tmp/x".into(),
            display_name: "一个非常非常长的文件夹名字".into(),
            extension: None,
            is_dir: true,
            mime: None,
        };
        assert_eq!(display_label(&folder), "一个非常非常长的文件…");
    }
}
