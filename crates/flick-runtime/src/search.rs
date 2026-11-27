//! 搜索匹配的纯函数层。
//!
//! 这里只负责“文本 → 匹配档位/字段/质量分”和“输入上下文 → 可消费 facet”的
//! 计算，不持有注册表，也不做任何 I/O。注册表负责遍历候选并应用确定性排序。

use flick_domain::{
    ActionDescriptor, Category, FileInput, InputContext, InputFacet, InputFacetKind, InputType,
    MatchField, PluginManifest,
};

/// 原文匹配档位；数值越高越优先。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum MatchTier {
    /// 只在描述、分类、作者等弱字段命中。
    Description,
    /// 非词首的子串命中。
    Inner,
    /// 拼音前缀命中（阶段 2）。
    PrefixPinyin,
    /// 两字及以上的纯首字母缩写完全命中（阶段 2）。
    ShortAcronymExact,
    /// 词首前缀命中。
    PrefixLiteral,
    /// 全拼完全相等（阶段 2）。
    ExactPinyin,
    /// 整字段完全相等。
    ExactLiteral,
}

impl MatchTier {
    /// 档位基础分。
    pub(crate) fn score(self) -> f64 {
        match self {
            Self::Description => 100.0,
            Self::Inner => 300.0,
            Self::PrefixPinyin => 520.0,
            Self::ShortAcronymExact => 550.0,
            Self::PrefixLiteral => 600.0,
            Self::ExactPinyin => 700.0,
            Self::ExactLiteral => 800.0,
        }
    }
}

/// 匹配来源是动作自身字段，还是插件级字段展开。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatchScope {
    /// 命中动作标题、关键词、id 或描述。
    Action,
    /// 命中插件名/关键词/标签，从而展开该插件所有公开动作。
    PluginExpansion,
}

/// 一个动作在一次搜索中的最终评分。
#[derive(Debug, Clone)]
pub(crate) struct ActionMatch {
    /// 档位分 + 字段质量分 + 分类权重。
    pub score: f64,
    /// 该动作消费的输入 facet。
    pub facet: Option<InputFacetKind>,
    /// 最佳命中字段。
    pub matched_field: MatchField,
    /// 最佳命中的字符区间。
    pub matched_range: Option<(usize, usize)>,
    /// 动作标题长度，用于稳定排序。
    pub title_len: usize,
    /// 分类排序秩；越小越优先。
    pub category_rank: u32,
}

/// 单个字段上的文本命中。
#[derive(Debug, Clone, Copy)]
struct TextMatch {
    tier: MatchTier,
    range: Option<(usize, usize)>,
}

/// 字段 + 命中信息。
#[derive(Debug, Clone, Copy)]
struct FieldMatch {
    field: MatchField,
    tier: MatchTier,
    range: Option<(usize, usize)>,
}

/// 对动作与插件做一次评分；没有命中或输入类型不兼容时返回 `None`。
pub(crate) fn score_action(
    descriptor: &ActionDescriptor,
    plugin: Option<&PluginManifest>,
    context: &InputContext,
) -> Option<ActionMatch> {
    let query = context.raw_text.trim();
    if query.is_empty() {
        return None;
    }

    let facet = select_input(descriptor, context)?;
    let allow_inner = !(query.chars().count() == 1 && query.is_ascii());
    let allow_subsequence =
        query.chars().count() >= 2 && query.chars().all(|ch| ch.is_ascii_alphanumeric());

    let action_match =
        match_action_fields(descriptor, plugin, query, allow_inner, allow_subsequence);
    let plugin_match = plugin
        .and_then(|manifest| match_plugin_fields(manifest, query, allow_inner, allow_subsequence));

    let (field_match, scope) = match (action_match, plugin_match) {
        (Some(action), Some(plugin_match)) => {
            if field_match_better(plugin_match, action) {
                (plugin_match, MatchScope::PluginExpansion)
            } else {
                (action, MatchScope::Action)
            }
        }
        (Some(action), None) => (action, MatchScope::Action),
        (None, Some(plugin_match)) => (plugin_match, MatchScope::PluginExpansion),
        (None, None) => return None,
    };

    let primary_name = match scope {
        MatchScope::Action => descriptor.title.as_str(),
        MatchScope::PluginExpansion => plugin.map(|manifest| manifest.name.as_str()).unwrap_or(""),
    };
    let quality = quality_score(field_match.field, primary_name, query.chars().count());
    let score = field_match.tier.score() + quality + category_weight(plugin, scope);

    Some(ActionMatch {
        score,
        facet,
        matched_field: field_match.field,
        matched_range: field_match.range,
        title_len: descriptor.title.chars().count(),
        category_rank: category_rank(plugin, scope),
    })
}

/// 判断动作声明的输入类型是否兼容当前上下文，并选出最具体的 facet。
///
/// 返回 `None` 表示不兼容；返回 `Some(None)` 表示动作声明了 `InputType::None`
/// （不需要输入）。
pub(crate) fn select_input(
    descriptor: &ActionDescriptor,
    context: &InputContext,
) -> Option<Option<InputFacetKind>> {
    if descriptor.supported_inputs.len() == 1
        && descriptor.supported_inputs.first() == Some(&InputType::None)
    {
        return Some(None);
    }

    let selected = if descriptor.supported_inputs.is_empty() {
        most_specific_facet(context)
    } else {
        let mut matched = Vec::new();
        let file_count = file_count(context);
        for input_type in &descriptor.supported_inputs {
            match input_type {
                InputType::PlainText if context.has_facet(InputFacetKind::PlainText) => {
                    matched.push(InputFacetKind::PlainText)
                }
                InputType::Url if context.has_facet(InputFacetKind::Url) => {
                    matched.push(InputFacetKind::Url)
                }
                InputType::PathCandidate if context.has_facet(InputFacetKind::PathLike) => {
                    matched.push(InputFacetKind::PathLike)
                }
                InputType::MathExpression if context.has_facet(InputFacetKind::MathExpression) => {
                    matched.push(InputFacetKind::MathExpression)
                }
                // `File` 只接受单个文件/文件夹；`Files` 接受一个或多个。
                // 只声明 `File` 的动作不接受多文件，避免隐式截断（设计文档 §2.3）。
                InputType::File if file_count == 1 => matched.push(InputFacetKind::File),
                InputType::Files if file_count >= 1 => matched.push(InputFacetKind::File),
                InputType::ClipboardImage if context.has_facet(InputFacetKind::ClipboardImage) => {
                    matched.push(InputFacetKind::ClipboardImage)
                }
                InputType::None => {}
                _ => {}
            }
        }
        if matched.is_empty() {
            return None;
        }
        matched.into_iter().max_by_key(facet_priority)
    };

    // file_extensions 只对确实带了文件/文件夹的输入生效。
    if let Some(file) = first_file(context) {
        if !descriptor.file_extensions.is_empty()
            && !file_extension_allowed(file, &descriptor.file_extensions)
        {
            return None;
        }
    }

    Some(selected)
}

/// 文本 → 字段匹配；支持多词“所有词都命中同一字段”。
fn match_action_fields(
    descriptor: &ActionDescriptor,
    plugin: Option<&PluginManifest>,
    query: &str,
    allow_inner: bool,
    allow_subsequence: bool,
) -> Option<FieldMatch> {
    let mut best: Option<FieldMatch> = None;

    if let Some(text_match) = match_text(&descriptor.title, query, allow_inner, allow_subsequence) {
        best = choose_best(
            best,
            FieldMatch {
                field: MatchField::Name,
                tier: text_match.tier,
                range: text_match.range,
            },
        );
    }

    for keyword in &descriptor.keywords {
        if let Some(text_match) = match_text(keyword, query, allow_inner, allow_subsequence) {
            best = choose_best(
                best,
                FieldMatch {
                    field: MatchField::Keyword,
                    tier: text_match.tier,
                    range: text_match.range,
                },
            );
        }
    }

    if allow_inner {
        if let Some(text_match) = match_text(descriptor.id.as_str(), query, true, allow_subsequence)
        {
            best = choose_best(
                best,
                FieldMatch {
                    field: MatchField::Id,
                    tier: text_match.tier,
                    range: text_match.range,
                },
            );
        }
    }

    if let Some(manifest) = plugin {
        if !manifest.profile.description.is_empty() {
            if let Some(text_match) = match_text(
                &manifest.profile.description,
                query,
                allow_inner,
                allow_subsequence,
            ) {
                best = choose_best(
                    best,
                    FieldMatch {
                        field: MatchField::Description,
                        // 描述字段只作为兜底档，不参与 EXACT/PREFIX/INNER 排名。
                        tier: MatchTier::Description,
                        range: text_match.range,
                    },
                );
            }
        }
    }

    best
}

/// 插件级字段匹配：插件名、插件关键词、插件标签、插件描述。
fn match_plugin_fields(
    manifest: &PluginManifest,
    query: &str,
    allow_inner: bool,
    allow_subsequence: bool,
) -> Option<FieldMatch> {
    let mut best: Option<FieldMatch> = None;

    if let Some(text_match) = match_text(&manifest.name, query, allow_inner, allow_subsequence) {
        best = choose_best(
            best,
            FieldMatch {
                field: MatchField::Name,
                tier: text_match.tier,
                range: text_match.range,
            },
        );
    }

    for keyword in &manifest.keywords {
        if let Some(text_match) = match_text(keyword, query, allow_inner, allow_subsequence) {
            best = choose_best(
                best,
                FieldMatch {
                    field: MatchField::Keyword,
                    tier: text_match.tier,
                    range: text_match.range,
                },
            );
        }
    }

    for tag in &manifest.profile.tags {
        if let Some(text_match) = match_text(tag, query, allow_inner, allow_subsequence) {
            best = choose_best(
                best,
                FieldMatch {
                    field: MatchField::Keyword,
                    tier: text_match.tier,
                    range: text_match.range,
                },
            );
        }
    }

    if !manifest.profile.description.is_empty() {
        if let Some(text_match) = match_text(
            &manifest.profile.description,
            query,
            allow_inner,
            allow_subsequence,
        ) {
            best = choose_best(
                best,
                FieldMatch {
                    field: MatchField::Description,
                    tier: MatchTier::Description,
                    range: text_match.range,
                },
            );
        }
    }

    best
}

/// 在字段文本中匹配查询；支持多词匹配。
fn match_text(
    text: &str,
    query: &str,
    allow_inner: bool,
    allow_subsequence: bool,
) -> Option<TextMatch> {
    if query.trim().is_empty() {
        return None;
    }

    let text_lower = text.to_lowercase();
    let query_lower = query.to_lowercase();

    if let Some(found) = match_single(&text_lower, &query_lower, allow_inner, allow_subsequence) {
        return Some(found);
    }

    let terms: Vec<&str> = query_lower.split_whitespace().collect();
    if terms.len() < 2 {
        return None;
    }

    let mut best: Option<TextMatch> = None;
    for term in terms {
        let found = match_single(&text_lower, term, allow_inner, allow_subsequence)?;
        if best.map_or(true, |current| found.tier < current.tier) {
            best = Some(found);
        }
    }
    best
}

/// 单段文本匹配：完全相等 → 词首前缀 → 非词首子串 → 词首子序列。
fn match_single(
    text: &str,
    query: &str,
    allow_inner: bool,
    allow_subsequence: bool,
) -> Option<TextMatch> {
    if text == query {
        return Some(TextMatch {
            tier: MatchTier::ExactLiteral,
            range: Some((0, query.chars().count())),
        });
    }

    if text.starts_with(query) {
        return Some(TextMatch {
            tier: MatchTier::PrefixLiteral,
            range: Some((0, query.chars().count())),
        });
    }

    // 拼音（全拼 / 首字母缩写 / 拼音前缀）—— 设计文档 §4.2 阶段 2。
    if let Some(tier) = match_pinyin(text, query) {
        return Some(TextMatch { tier, range: None });
    }

    if allow_inner {
        if let Some(byte_index) = text.find(query) {
            let at_word_start = byte_index == 0
                || text[..byte_index]
                    .chars()
                    .next_back()
                    .map(|ch| !ch.is_alphanumeric())
                    .unwrap_or(true);
            let tier = if at_word_start {
                MatchTier::PrefixLiteral
            } else {
                MatchTier::Inner
            };
            return Some(TextMatch {
                tier,
                range: Some((
                    text[..byte_index].chars().count(),
                    text[..byte_index + query.len()].chars().count(),
                )),
            });
        }
    }

    if allow_inner && allow_subsequence {
        if let Some(range) = word_subsequence_range(text, query) {
            return Some(TextMatch {
                tier: MatchTier::Inner,
                range: Some(range),
            });
        }
    }

    None
}

/// 词首子序列匹配；返回首字符到末字符的字符区间。
fn word_subsequence_range(text: &str, query: &str) -> Option<(usize, usize)> {
    let mut query_chars = query.chars();
    let mut current = query_chars.next()?;
    let mut start_byte = None;

    for (byte_index, ch) in text.char_indices() {
        if ch != current {
            continue;
        }
        let at_word_start = byte_index == 0
            || text[..byte_index]
                .chars()
                .next_back()
                .map(|prev| !prev.is_alphanumeric())
                .unwrap_or(true);
        if !at_word_start {
            continue;
        }

        if start_byte.is_none() {
            start_byte = Some(byte_index);
        }
        let end_byte = byte_index + ch.len_utf8();

        match query_chars.next() {
            Some(next) => current = next,
            None => {
                let start = start_byte?;
                return Some((
                    text[..start].chars().count(),
                    text[..end_byte].chars().count(),
                ));
            }
        }
    }

    None
}

/// 拼音查询长度上限（与 ZTools `MAX_PINYIN_QUERY_LENGTH` 一致）。
const MAX_PINYIN_QUERY_LENGTH: usize = 64;
/// 拼音字符串缓存上限（与 ZTools `TOKEN_CACHE_LIMIT` 一致）。
const PINYIN_CACHE_LIMIT: usize = 5000;

/// 全局拼音数据（惰性初始化一次）。
fn pinyin_data() -> &'static ib_pinyin::pinyin::PinyinData {
    static DATA: std::sync::OnceLock<ib_pinyin::pinyin::PinyinData> = std::sync::OnceLock::new();
    DATA.get_or_init(|| {
        use ib_pinyin::pinyin::{PinyinData, PinyinNotation};
        PinyinData::new(PinyinNotation::Ascii | PinyinNotation::AsciiFirstLetter)
    })
}

/// 文本 → 拼音查询串的缓存：`(全拼, 首字母)`。
fn cached_pinyin(text: &str) -> (String, String) {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, (String, String)>>,
    > = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Ok(map) = cache.lock() {
        if let Some(found) = map.get(text) {
            return found.clone();
        }
    }
    let computed = compute_pinyin(text);
    if let Ok(mut map) = cache.lock() {
        if map.len() >= PINYIN_CACHE_LIMIT {
            map.clear();
        }
        map.insert(text.to_string(), computed.clone());
    }
    computed
}

/// 计算文本的全拼串与首字母串；ASCII 字母数字按原字符参与，标点忽略。
fn compute_pinyin(text: &str) -> (String, String) {
    use ib_pinyin::pinyin::PinyinNotation;
    let data = pinyin_data();
    let mut full = String::new();
    let mut initials = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            let lower = ch.to_ascii_lowercase();
            full.push(lower);
            initials.push(lower);
        } else if let Some(pinyin) = data.get_pinyins(ch).next() {
            if let Some(ascii) = pinyin.notation(PinyinNotation::Ascii) {
                full.push_str(ascii);
                if let Some(first) = ascii.chars().next() {
                    initials.push(first);
                }
            }
        }
    }
    (full, initials)
}

/// 拼音匹配：返回命中档位，不返回高亮区间（拼音命中不映射回原文区间）。
fn match_pinyin(text: &str, query: &str) -> Option<MatchTier> {
    if query.is_empty() || text.is_ascii() {
        return None;
    }
    if query.chars().count() > MAX_PINYIN_QUERY_LENGTH
        || !query.chars().all(|ch| ch.is_ascii_alphanumeric())
    {
        return None;
    }
    let (full, initials) = cached_pinyin(text);
    if full.is_empty() {
        return None;
    }
    let query_len = query.chars().count();
    if full == query {
        return Some(MatchTier::ExactPinyin);
    }
    if query_len >= 2 && initials == query {
        return Some(MatchTier::ShortAcronymExact);
    }
    if query_len >= 2 && (full.starts_with(query) || initials.starts_with(query)) {
        return Some(MatchTier::PrefixPinyin);
    }
    None
}

/// 字段质量分：字段基础权重 + 标题紧凑度奖励。
fn quality_score(field: MatchField, primary_name: &str, query_len: usize) -> f64 {
    let field_weight = match field {
        MatchField::Name => 40.0,
        MatchField::Keyword => 25.0,
        MatchField::Id => 10.0,
        MatchField::Description => 0.0,
    };

    let compactness = if field == MatchField::Name {
        let title_len = primary_name.chars().count();
        (20usize.saturating_sub(title_len.saturating_sub(query_len))) as f64
    } else {
        0.0
    };

    field_weight + compactness
}

/// 分类权重。
fn category_weight(plugin: Option<&PluginManifest>, scope: MatchScope) -> f64 {
    if scope == MatchScope::PluginExpansion {
        return 35.0;
    }

    let Some(manifest) = plugin else {
        return 0.0;
    };

    if manifest.profile.categories.contains(&Category::System) {
        70.0
    } else if manifest.profile.categories.contains(&Category::Apps) || is_apps_plugin(manifest) {
        60.0
    } else if manifest.id.starts_with("flick.builtin.") {
        0.0
    } else {
        20.0
    }
}

/// 分类排序秩；越小越靠前。
fn category_rank(plugin: Option<&PluginManifest>, scope: MatchScope) -> u32 {
    if scope == MatchScope::PluginExpansion {
        return 3;
    }

    let Some(manifest) = plugin else {
        return 7;
    };

    if manifest.profile.categories.contains(&Category::System) {
        1
    } else if manifest.profile.categories.contains(&Category::Apps) || is_apps_plugin(manifest) {
        2
    } else if manifest.id.starts_with("flick.builtin.") {
        7
    } else {
        4
    }
}

fn is_apps_plugin(manifest: &PluginManifest) -> bool {
    manifest.id == "flick.builtin.apps"
}

/// 上下文中最具体的 facet；用于没有声明 supported_inputs 的动作。
fn most_specific_facet(context: &InputContext) -> Option<InputFacetKind> {
    context
        .facets
        .iter()
        .map(InputFacet::kind)
        .max_by_key(facet_priority)
}

fn facet_priority(kind: &InputFacetKind) -> u8 {
    match kind {
        InputFacetKind::PlainText => 0,
        InputFacetKind::MathExpression => 1,
        InputFacetKind::Url => 2,
        InputFacetKind::PathLike => 3,
        InputFacetKind::File => 4,
        InputFacetKind::ClipboardImage => 5,
    }
}

fn file_count(context: &InputContext) -> usize {
    context
        .facets
        .iter()
        .filter(|facet| matches!(facet, InputFacet::File { .. }))
        .count()
}

fn first_file(context: &InputContext) -> Option<&FileInput> {
    context.facets.iter().find_map(|facet| match facet {
        InputFacet::File { file } => Some(file),
        _ => None,
    })
}

fn file_extension_allowed(file: &FileInput, allowed: &[String]) -> bool {
    if file.is_dir {
        return false;
    }
    let Some(extension) = file.extension.as_deref() else {
        return false;
    };
    let extension = extension.trim_start_matches('.').to_ascii_lowercase();
    allowed.iter().any(|candidate| {
        candidate
            .trim_start_matches('.')
            .eq_ignore_ascii_case(&extension)
    })
}

/// 选择两个字段匹配中更优的一个。
fn choose_best(current: Option<FieldMatch>, candidate: FieldMatch) -> Option<FieldMatch> {
    match current {
        Some(existing) if !field_match_better(candidate, existing) => Some(existing),
        _ => Some(candidate),
    }
}

/// 比较两个字段匹配；档位优先，其次字段质量，再其次区间起点更小者。
fn field_match_better(candidate: FieldMatch, current: FieldMatch) -> bool {
    candidate.tier > current.tier
        || (candidate.tier == current.tier
            && field_weight(candidate.field) > field_weight(current.field))
        || (candidate.tier == current.tier
            && field_weight(candidate.field) == field_weight(current.field)
            && range_start(candidate.range) < range_start(current.range))
}

fn field_weight(field: MatchField) -> u8 {
    match field {
        MatchField::Name => 40,
        MatchField::Keyword => 25,
        MatchField::Id => 10,
        MatchField::Description => 0,
    }
}

fn range_start(range: Option<(usize, usize)>) -> usize {
    range.map(|(start, _)| start).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flick_domain::{
        ActionDescriptor, ActionId, ConcurrencyPolicy, InputContext, InputSource, QueryGeneration,
        SearchInputSnapshot,
    };

    fn context(text: &str) -> InputContext {
        InputContext::from_snapshot(
            QueryGeneration::INITIAL,
            &SearchInputSnapshot::text(text, InputSource::Manual),
        )
    }

    fn descriptor(title: &str, keywords: &[&str]) -> ActionDescriptor {
        ActionDescriptor::new(
            ActionId::from_static("demo.plugin.v1.action"),
            title,
            keywords
                .iter()
                .map(|keyword| (*keyword).to_string())
                .collect(),
            ConcurrencyPolicy::Parallel,
        )
    }

    #[test]
    fn exact_beats_prefix_beats_inner() {
        let exact = match_text("Echo", "echo", true, true).unwrap();
        let prefix = match_text("Echo Hello", "echo", true, true).unwrap();
        let inner = match_text("Xecho", "echo", true, true).unwrap();

        assert_eq!(exact.tier, MatchTier::ExactLiteral);
        assert_eq!(prefix.tier, MatchTier::PrefixLiteral);
        assert_eq!(inner.tier, MatchTier::Inner);
        assert!(exact.tier.score() > prefix.tier.score());
        assert!(prefix.tier.score() > inner.tier.score());
    }

    #[test]
    fn description_is_fallback_tier() {
        let found = match_text("a plugin that echoes text", "echo", true, true).unwrap();
        assert_eq!(found.tier, MatchTier::PrefixLiteral);

        let descriptor = descriptor("Unrelated", &[]);
        let plugin = PluginManifest {
            id: "demo.plugin".into(),
            name: "Demo".into(),
            author: "Demo".into(),
            version: "1.0.0".into(),
            kind: flick_domain::PluginKind::BuiltInRust,
            keywords: vec![],
            icon: None,
            profile: flick_domain::PluginProfile {
                description: "echo helper".into(),
                ..Default::default()
            },
            permissions: vec![],
            plugin_dependencies: vec![],
            settings: vec![],
            surfaces: vec![],
            supported_os: vec![],
            supported_architectures: vec![],
            needs_search: true,
        };
        let action_match = score_action(&descriptor, Some(&plugin), &context("echo")).unwrap();
        assert_eq!(action_match.matched_field, MatchField::Description);
    }

    #[test]
    fn pinyin_full_prefix_and_acronym_match_chinese_text() {
        // 火狐浏览器 -> 全拼 huohuliulanqi，首字母 hhllq
        let text = "火狐浏览器";
        assert_eq!(
            compute_pinyin(text),
            ("huohuliulanqi".to_string(), "hhllq".to_string())
        );
        assert_eq!(
            match_pinyin(text, "huohuliulanqi"),
            Some(MatchTier::ExactPinyin)
        );
        assert_eq!(match_pinyin(text, "huohu"), Some(MatchTier::PrefixPinyin));
        assert_eq!(
            match_pinyin(text, "hhllq"),
            Some(MatchTier::ShortAcronymExact)
        );
        assert_eq!(match_pinyin(text, "zzz"), None);
        // 纯 ASCII 文本走原文匹配，不做拼音。
        assert_eq!(match_pinyin("Firefox", "fire"), None);
    }

    #[test]
    fn pinyin_tier_scores_rank_between_literal_tiers() {
        assert!(MatchTier::ExactPinyin.score() > MatchTier::PrefixLiteral.score());
        assert!(MatchTier::ExactLiteral.score() > MatchTier::ExactPinyin.score());
        assert!(MatchTier::PrefixLiteral.score() > MatchTier::ShortAcronymExact.score());
        assert!(MatchTier::ShortAcronymExact.score() > MatchTier::PrefixPinyin.score());
        assert!(MatchTier::PrefixPinyin.score() > MatchTier::Inner.score());
    }

    #[test]
    fn multi_term_matches_only_when_all_terms_match() {
        assert!(match_text("Search Notes", "search notes", true, true).is_some());
        assert!(match_text("Search Notes", "search missing", true, true).is_none());
    }

    #[test]
    fn empty_supported_inputs_accept_all_contexts() {
        let descriptor = descriptor("Echo", &["echo"]);
        let text = context("echo");
        assert_eq!(
            select_input(&descriptor, &text),
            Some(Some(InputFacetKind::PlainText))
        );

        let file = FileInput {
            uri: "file:///tmp/a.txt".into(),
            display_name: "a.txt".into(),
            extension: Some("txt".into()),
            is_dir: false,
            mime: Some("text/plain".into()),
        };
        let files = InputContext::from_snapshot(
            QueryGeneration::INITIAL,
            &SearchInputSnapshot::files(vec![file], InputSource::DragDrop),
        );
        assert_eq!(
            select_input(&descriptor, &files),
            Some(Some(InputFacetKind::File))
        );
    }

    #[test]
    fn supported_input_filter_rejects_mismatch() {
        let mut descriptor = descriptor("Open URL", &["url"]);
        descriptor.supported_inputs = vec![InputType::Url];
        assert!(select_input(&descriptor, &context("plain text")).is_none());
        assert!(select_input(&descriptor, &context("https://example.com")).is_some());
    }

    #[test]
    fn file_extensions_filter_only_affects_file_inputs() {
        let mut descriptor = descriptor("Open Rust", &["rust"]);
        descriptor.supported_inputs = vec![InputType::File];
        descriptor.file_extensions = vec!["rs".into()];

        let rs = FileInput {
            uri: "file:///tmp/main.rs".into(),
            display_name: "main.rs".into(),
            extension: Some("rs".into()),
            is_dir: false,
            mime: Some("text/plain".into()),
        };
        let txt = FileInput {
            uri: "file:///tmp/notes.txt".into(),
            display_name: "notes.txt".into(),
            extension: Some("txt".into()),
            is_dir: false,
            mime: Some("text/plain".into()),
        };

        let rs_context = InputContext::from_snapshot(
            QueryGeneration::INITIAL,
            &SearchInputSnapshot::files(vec![rs], InputSource::DragDrop),
        );
        let txt_context = InputContext::from_snapshot(
            QueryGeneration::INITIAL,
            &SearchInputSnapshot::files(vec![txt], InputSource::DragDrop),
        );

        assert!(select_input(&descriptor, &rs_context).is_some());
        assert!(select_input(&descriptor, &txt_context).is_none());
    }

    /// `InputType::File` 只接受单个文件；多文件必须声明 `Files`；
    /// 同时声明两者时多文件放行。
    #[test]
    fn file_requires_single_file_but_files_accepts_many() {
        fn file_input(name: &str) -> FileInput {
            FileInput {
                uri: format!("file:///tmp/{name}"),
                display_name: name.into(),
                extension: name.rsplit_once('.').map(|(_, ext)| ext.to_string()),
                is_dir: false,
                mime: None,
            }
        }
        fn ctx(files: Vec<FileInput>) -> InputContext {
            InputContext::from_snapshot(
                QueryGeneration::INITIAL,
                &SearchInputSnapshot::files(files, InputSource::DragDrop),
            )
        }

        let one = ctx(vec![file_input("a.txt")]);
        let many = ctx(vec![file_input("a.txt"), file_input("b.txt")]);

        let mut only_file = descriptor("Open", &["open"]);
        only_file.supported_inputs = vec![InputType::File];
        assert!(
            select_input(&only_file, &one).is_some(),
            "单文件应匹配 File"
        );
        assert!(
            select_input(&only_file, &many).is_none(),
            "只声明 File 的动作不接受多文件"
        );

        let mut only_files = descriptor("Open", &["open"]);
        only_files.supported_inputs = vec![InputType::Files];
        assert!(select_input(&only_files, &one).is_some());
        assert!(select_input(&only_files, &many).is_some());

        let mut both = descriptor("Open", &["open"]);
        both.supported_inputs = vec![InputType::File, InputType::Files];
        assert!(
            select_input(&both, &many).is_some(),
            "同时声明 File 与 Files 时多文件放行"
        );
    }
}
