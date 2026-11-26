//! 搜索输入类型：UI 采集快照，以及 Runtime 解析后的输入上下文（facet）。

use crate::action::{ActionInput, FileInput, ImageData, InputType};
use crate::id::QueryGeneration;
use serde::{Deserialize, Serialize};

/// 用户提交的搜索查询。
///
/// 这是旧的轻量协议，保留用于兼容现有调用方；新的输入链路应优先使用
/// [`SearchInputSnapshot`] + [`InputContext`]。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub raw_text: String,
    pub input_type: InputType,
}

impl SearchQuery {
    pub fn plain_text(text: impl Into<String>) -> Self {
        Self {
            raw_text: text.into(),
            input_type: InputType::PlainText,
        }
    }
}

/// 输入来源，记录 UI 是如何得到这份输入的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum InputSource {
    /// 手动键入。
    #[default]
    Manual,
    /// 显式粘贴。
    Paste,
    /// 拖拽文件或文件夹。
    DragDrop,
    /// 打开搜索框时自动读取到 5 秒内变化的剪贴板。
    ClipboardRecent,
}

/// UI 层只负责采集和展示的初始输入种类。
///
/// URL、路径、数学表达式等细分类是 Runtime 的 facet 解析结果，不是 UI 分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum UiInputKind {
    /// 文本输入（手动键入、剪贴板文本）。
    #[default]
    Text,
    /// 文件或文件夹输入（粘贴、拖拽、文件选择器）。
    Files,
    /// 未落盘的剪贴板图片。
    ClipboardImage,
}

/// UI 采集到的输入快照。
///
/// 它只描述“用户给了什么”，不包含 URL/路径/数学表达式等解析结果；这些由
/// [`InputContext::from_snapshot`] 产生。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchInputSnapshot {
    /// UI 初始输入种类。
    pub kind: UiInputKind,
    /// 输入来源。
    pub source: InputSource,
    /// 文本输入；文件输入时通常是首个 `file://` URI 或空串。
    pub text: String,
    /// 文件/文件夹列表；非文件输入为空。
    #[serde(default)]
    pub files: Vec<FileInput>,
    /// 剪贴板图片；仅 `ClipboardImage` 使用。
    #[serde(default)]
    pub image: Option<ImageData>,
}

impl SearchInputSnapshot {
    /// 构造一个纯文本快照。
    pub fn text(text: impl Into<String>, source: InputSource) -> Self {
        Self {
            kind: UiInputKind::Text,
            source,
            text: text.into(),
            files: Vec::new(),
            image: None,
        }
    }

    /// 构造一个文件列表快照；`text` 默认为首个 `file://` URI。
    pub fn files(files: Vec<FileInput>, source: InputSource) -> Self {
        let text = files
            .first()
            .map(|file| file.uri.clone())
            .unwrap_or_default();
        Self {
            kind: UiInputKind::Files,
            source,
            text,
            files,
            image: None,
        }
    }

    /// 构造一个未落盘剪贴板图片快照。
    pub fn clipboard_image(image: ImageData, source: InputSource) -> Self {
        Self {
            kind: UiInputKind::ClipboardImage,
            source,
            text: String::new(),
            files: Vec::new(),
            image: Some(image),
        }
    }

    /// 映射为 Runtime 接收的动作输入。
    ///
    /// 这里不做 `stat`；`PathCandidate -> File` 的存在性归一化应在调用入口统一处理。
    pub fn to_action_input(&self) -> ActionInput {
        match self.kind {
            UiInputKind::Text => ActionInput::plain_text(self.text.clone()),
            UiInputKind::Files => match self.files.as_slice() {
                [] => ActionInput::plain_text(self.text.clone()),
                [file] => ActionInput {
                    kind: InputType::PathCandidate,
                    text: file.uri.clone(),
                    image: None,
                    files: self.files.clone(),
                },
                _ => ActionInput {
                    kind: InputType::Files,
                    text: self
                        .files
                        .first()
                        .map(|file| file.uri.clone())
                        .unwrap_or_default(),
                    image: None,
                    files: self.files.clone(),
                },
            },
            UiInputKind::ClipboardImage => match &self.image {
                Some(image) => ActionInput {
                    kind: InputType::ClipboardImage,
                    text: String::new(),
                    image: Some(image.clone()),
                    files: Vec::new(),
                },
                None => ActionInput::none(),
            },
        }
    }
}

impl From<&SearchInputSnapshot> for ActionInput {
    fn from(snapshot: &SearchInputSnapshot) -> Self {
        snapshot.to_action_input()
    }
}

/// 路径类输入的粗分类；仅表示文本形状，不表示路径是否存在。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathLikeKind {
    /// 绝对路径，例如 `/home/me/file.txt`。
    Absolute,
    /// 相对路径，例如 `./Cargo.toml`、`../README.md`。
    Relative,
    /// 以 `~` 开头的家目录路径。
    Home,
    /// `file://` URI。
    FileUri,
}

/// 单个输入 facet 的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputFacetKind {
    PlainText,
    Url,
    PathLike,
    MathExpression,
    File,
    ClipboardImage,
}

/// Runtime 对输入的一种解释证据；同一输入可同时携带多个 facet。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputFacet {
    /// 纯文本永远存在，保证只声明 PlainText 的普通动作不会被排除。
    PlainText { text: String },
    /// 形如 http(s)/ftp/mailto 的 URL 文本。
    Url { raw: String, normalized: String },
    /// 路径形状的文本或文件 URI。
    PathLike { raw: String, kind: PathLikeKind },
    /// 数学表达式；`explicit` 表示用户用 `=` 前缀显式声明。
    MathExpression { source: String, explicit: bool },
    /// 文件/文件夹附件。
    File { file: FileInput },
    /// 未落盘剪贴板图片。
    ClipboardImage { image: ImageData },
}

impl InputFacet {
    /// 返回 facet 的类别。
    pub fn kind(&self) -> InputFacetKind {
        match self {
            Self::PlainText { .. } => InputFacetKind::PlainText,
            Self::Url { .. } => InputFacetKind::Url,
            Self::PathLike { .. } => InputFacetKind::PathLike,
            Self::MathExpression { .. } => InputFacetKind::MathExpression,
            Self::File { .. } => InputFacetKind::File,
            Self::ClipboardImage { .. } => InputFacetKind::ClipboardImage,
        }
    }
}

/// Runtime 解析后的输入上下文。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputContext {
    /// 本次查询的代号，用于丢弃过期结果。
    pub generation: QueryGeneration,
    /// 原始文本；文件输入时通常是首个 `file://` URI 或空串。
    pub raw_text: String,
    /// 输入来源。
    pub source: InputSource,
    /// 可并存的输入解释证据。
    pub facets: Vec<InputFacet>,
}

impl InputContext {
    /// 从 UI 快照解析输入上下文。
    ///
    /// 所有探测均为纯函数：不读网络、不查数据库、不做文件 `stat`。
    pub fn from_snapshot(generation: QueryGeneration, snapshot: &SearchInputSnapshot) -> Self {
        let mut facets = Vec::new();

        match snapshot.kind {
            UiInputKind::Text => {
                // 文本输入永远保留 PlainText，再叠加 URL / 路径 / 数学表达式证据。
                facets.push(InputFacet::PlainText {
                    text: snapshot.text.clone(),
                });

                if let Some(normalized) = normalize_url(&snapshot.text) {
                    facets.push(InputFacet::Url {
                        raw: snapshot.text.clone(),
                        normalized,
                    });
                }

                if let Some(kind) = detect_path_like(&snapshot.text) {
                    facets.push(InputFacet::PathLike {
                        raw: snapshot.text.clone(),
                        kind,
                    });
                }

                if let Some((source, explicit)) = detect_math_expression(&snapshot.text) {
                    facets.push(InputFacet::MathExpression { source, explicit });
                }
            }
            UiInputKind::Files => {
                // 文件输入保留原始文本，并为每个文件生成 File facet。
                facets.push(InputFacet::PlainText {
                    text: snapshot.text.clone(),
                });

                for file in &snapshot.files {
                    facets.push(InputFacet::File { file: file.clone() });
                }

                // 同时补一个 PathLike facet，供只声明路径能力的动作使用。
                if let Some(first) = snapshot.files.first() {
                    facets.push(InputFacet::PathLike {
                        raw: first.uri.clone(),
                        kind: PathLikeKind::FileUri,
                    });
                }
            }
            UiInputKind::ClipboardImage => {
                if let Some(image) = &snapshot.image {
                    facets.push(InputFacet::ClipboardImage {
                        image: image.clone(),
                    });
                }
            }
        }

        Self {
            generation,
            raw_text: snapshot.text.clone(),
            source: snapshot.source,
            facets,
        }
    }

    /// 判断上下文是否包含某一类 facet。
    pub fn has_facet(&self, kind: InputFacetKind) -> bool {
        self.facets.iter().any(|facet| facet.kind() == kind)
    }
}

/// 归一化 URL 文本；不是 URL 时返回 `None`。
fn normalize_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    let known_scheme = ["http://", "https://", "ftp://", "mailto:"]
        .iter()
        .any(|prefix| lower.starts_with(prefix));
    if !known_scheme {
        return None;
    }

    Some(trimmed.to_string())
}

/// 判断文本是否看起来像路径；只做形状判断，不访问文件系统。
fn detect_path_like(raw: &str) -> Option<PathLikeKind> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.starts_with("file://") {
        return Some(PathLikeKind::FileUri);
    }

    if trimmed == "~" || trimmed.starts_with("~/") {
        return Some(PathLikeKind::Home);
    }

    if trimmed.starts_with('/') {
        return Some(PathLikeKind::Absolute);
    }

    if trimmed.starts_with("./") || trimmed.starts_with("../") {
        return Some(PathLikeKind::Relative);
    }

    // 兼容 Windows 盘符路径（例如 `C:\Users\me`）。
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let separator = bytes[2];
        if separator == b'\\' || separator == b'/' {
            return Some(PathLikeKind::Absolute);
        }
    }

    None
}

/// 探测数学表达式；`=` 前缀为显式模式，其余为保守的隐式模式。
fn detect_math_expression(raw: &str) -> Option<(String, bool)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 显式模式：`=1+2`。去掉 `=` 后按普通表达式校验。
    if let Some(rest) = trimmed.strip_prefix('=') {
        let expression = rest.trim();
        if !expression.is_empty() && is_math_expression_text(expression) {
            return Some((expression.to_string(), true));
        }
    }

    // 隐式模式：必须同时有数字和运算符，且只包含数学字符，避免把普通单词误判。
    if trimmed.chars().any(|ch| ch.is_ascii_digit())
        && trimmed
            .chars()
            .any(|ch| matches!(ch, '+' | '-' | '*' | '/' | '^' | '%'))
        && is_math_expression_text(trimmed)
    {
        return Some((trimmed.to_string(), false));
    }

    None
}

/// 表达式是否只由数字、空格和常见数学符号组成。
fn is_math_expression_text(text: &str) -> bool {
    text.chars().all(|ch| {
        ch.is_ascii_digit()
            || ch.is_ascii_whitespace()
            || matches!(
                ch,
                '+' | '-' | '*' | '/' | '^' | '%' | '(' | ')' | '.' | ','
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, uri: &str, is_dir: bool) -> FileInput {
        FileInput {
            uri: uri.to_string(),
            display_name: name.to_string(),
            extension: None,
            is_dir,
            mime: None,
        }
    }

    #[test]
    fn text_snapshot_has_plain_text_and_url_facet() {
        let snapshot = SearchInputSnapshot::text("https://example.com/a?b=1", InputSource::Paste);
        let context = InputContext::from_snapshot(QueryGeneration::INITIAL, &snapshot);

        assert!(context.has_facet(InputFacetKind::PlainText));
        assert!(context.has_facet(InputFacetKind::Url));
        assert_eq!(context.facets.len(), 2);
    }

    #[test]
    fn text_snapshot_detects_path_and_math_together() {
        let snapshot = SearchInputSnapshot::text("=1+2*3", InputSource::Manual);
        let context = InputContext::from_snapshot(QueryGeneration::INITIAL, &snapshot);

        assert!(context.has_facet(InputFacetKind::MathExpression));
        let math = context
            .facets
            .iter()
            .find_map(|facet| match facet {
                InputFacet::MathExpression { source, explicit } => {
                    Some((source.as_str(), *explicit))
                }
                _ => None,
            })
            .expect("math facet");
        assert_eq!(math, ("1+2*3", true));
    }

    #[test]
    fn path_shapes_are_detected() {
        assert_eq!(detect_path_like("/tmp/a.txt"), Some(PathLikeKind::Absolute));
        assert_eq!(detect_path_like("~/a.txt"), Some(PathLikeKind::Home));
        assert_eq!(detect_path_like("./a.txt"), Some(PathLikeKind::Relative));
        assert_eq!(
            detect_path_like("file:///tmp/a.txt"),
            Some(PathLikeKind::FileUri)
        );
        assert_eq!(detect_path_like("hello"), None);
    }

    #[test]
    fn files_snapshot_maps_single_to_path_candidate() {
        let snapshot = SearchInputSnapshot::files(
            vec![file("a.txt", "file:///tmp/a.txt", false)],
            InputSource::DragDrop,
        );
        let input = snapshot.to_action_input();

        assert_eq!(input.kind, InputType::PathCandidate);
        assert_eq!(input.text, "file:///tmp/a.txt");
        assert_eq!(input.files.len(), 1);
    }

    #[test]
    fn files_snapshot_maps_multiple_to_files() {
        let snapshot = SearchInputSnapshot::files(
            vec![
                file("a.txt", "file:///tmp/a.txt", false),
                file("dir", "file:///tmp/dir", true),
            ],
            InputSource::Paste,
        );
        let input = snapshot.to_action_input();

        assert_eq!(input.kind, InputType::Files);
        assert_eq!(input.text, "file:///tmp/a.txt");
        assert_eq!(input.files.len(), 2);
    }

    #[test]
    fn clipboard_image_snapshot_preserves_image() {
        let image = ImageData {
            mime: "image/png".into(),
            bytes: vec![1, 2, 3],
        };
        let snapshot =
            SearchInputSnapshot::clipboard_image(image.clone(), InputSource::ClipboardRecent);
        let context = InputContext::from_snapshot(QueryGeneration::INITIAL, &snapshot);
        let input = snapshot.to_action_input();

        assert!(context.has_facet(InputFacetKind::ClipboardImage));
        assert_eq!(input.kind, InputType::ClipboardImage);
        assert_eq!(input.image, Some(image));
    }

    #[test]
    fn files_snapshot_generates_file_and_path_like_facets() {
        let snapshot = SearchInputSnapshot::files(
            vec![file("a.txt", "file:///tmp/a.txt", false)],
            InputSource::Paste,
        );
        let context = InputContext::from_snapshot(QueryGeneration::INITIAL, &snapshot);

        assert!(context.has_facet(InputFacetKind::PlainText));
        assert!(context.has_facet(InputFacetKind::File));
        assert!(context.has_facet(InputFacetKind::PathLike));
    }
}
