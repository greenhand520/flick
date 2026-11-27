//! ActionRegistry — 存储与其处理器配对的 ActionDescriptors。

use crate::search::score_action;
use flick_domain::{
    ActionDescriptor, ActionId, ActionVisibility, DependencyStatus, InputContext, InputType,
    PluginManifest, RouteItem,
};
use flick_plugin::{
    ActionHandler, Plugin, PluginLifecycle, PluginRegistrationError, SurfaceHandler,
    validate_manifest,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// 单次搜索最多返回多少条 action candidates（设计文档 §9 约束 8）。
pub const MAX_ACTION_RESULTS: usize = 100;

/// 注册表中的一个条目：描述符 + 处理器 + 所属插件 id（裸注册时为 None）。
struct Entry {
    descriptor: ActionDescriptor,
    handler: Arc<dyn ActionHandler>,
    plugin_id: Option<String>,
}

/// 搜索阶段的内部候选，保存排序所需的辅助字段。
struct ScoredRoute {
    item: RouteItem,
    category_rank: u32,
    title_len: usize,
}

/// 将 [`ActionId`] 映射到（[`ActionDescriptor`]，[`ActionHandler`]）。
///
/// 提供关键字搜索和前缀命令匹配。完整的
/// 注册表**绝不**暴露给调用方 — 只有 `find_by_id` 和
/// `search` 可用。
pub struct ActionRegistry {
    entries: HashMap<ActionId, Entry>,
    /// 插件 id → manifest（仅通过 `register_plugin` 注册的插件）。
    ///
    /// 用 `BTreeMap` 保证依赖解析的迭代顺序确定。
    manifests: BTreeMap<String, PluginManifest>,
    /// 插件 id → Surface 交互处理器。
    surface_handlers: HashMap<String, Arc<dyn SurfaceHandler>>,
    /// 插件 id → 生命周期句柄（仅在有提供时记录）。
    lifecycles: HashMap<String, Arc<dyn PluginLifecycle>>,
    /// 插件 id → 该插件的依赖解析结果（由 `resolve_dependencies` 填充）。
    dependency_status: HashMap<String, Vec<DependencyStatus>>,
    /// 因必需依赖缺失而被判定不可用的插件 id。
    unavailable: HashSet<String>,
}

impl ActionRegistry {
    /// 创建一个空的注册表。
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            manifests: BTreeMap::new(),
            surface_handlers: HashMap::new(),
            lifecycles: HashMap::new(),
            dependency_status: HashMap::new(),
            unavailable: HashSet::new(),
        }
    }

    /// 注册一个新动作（裸注册，无插件归属）。
    ///
    /// 如果已存在具有相同 [`ActionId`] 的动作，则将其
    /// 替换。
    pub fn register(&mut self, descriptor: ActionDescriptor, handler: Arc<dyn ActionHandler>) {
        let id = descriptor.id.clone();
        self.entries.insert(
            id,
            Entry {
                descriptor,
                handler,
                plugin_id: None,
            },
        );
    }

    /// 注册一个插件提供的全部动作，并做命名空间/冲突/元数据校验。
    ///
    /// 校验全部通过后才写入，避免半写入状态。
    ///
    /// 静态规则委托给 [`flick_plugin::validate_manifest`]；`ActionId`
    /// 冲突需要注册表状态，在这里单独校验。
    pub fn register_plugin(&mut self, plugin: &dyn Plugin) -> Result<(), PluginRegistrationError> {
        let manifest = plugin.manifest();
        let lifecycle = plugin.lifecycle();
        let actions = plugin.actions();

        // 1. 纯静态校验（不依赖其它插件信息）。
        validate_manifest(&manifest, &actions)?;
        // 2. 唯一需要注册表状态的规则：ActionId 冲突。
        for spec in &actions {
            if self.entries.contains_key(&spec.descriptor.id) {
                return Err(PluginRegistrationError::IdCollision {
                    action_id: spec.descriptor.id.as_str().to_string(),
                });
            }
        }
        // 3. 全量校验通过，写入。
        for spec in actions {
            self.entries.insert(
                spec.descriptor.id.clone(),
                Entry {
                    descriptor: spec.descriptor,
                    handler: spec.handler,
                    plugin_id: Some(manifest.id.clone()),
                },
            );
        }
        if let Some(handler) = plugin.surface_handler() {
            self.surface_handlers.insert(manifest.id.clone(), handler);
        }
        if let Some(lifecycle) = lifecycle {
            self.lifecycles.insert(manifest.id.clone(), lifecycle);
        }
        self.manifests.insert(manifest.id.clone(), manifest);
        Ok(())
    }

    /// 解析所有已注册插件的依赖关系，并把结果应用到注册表。
    ///
    /// 必需依赖（含传递）不满足的插件会被标记为不可用：其动作、
    /// Surface 处理器与生命周期句柄一并移除，manifest 保留以便诊断
    /// 区分「未安装」与「依赖不满足」。
    ///
    /// 返回全部 `DependencyMissing` 错误，不短路。幂等。
    pub fn resolve_dependencies(&mut self) -> Vec<PluginRegistrationError> {
        let resolution = flick_plugin::resolve_dependencies(&self.manifests);
        for plugin_id in &resolution.unavailable {
            self.entries
                .retain(|_, entry| entry.plugin_id.as_deref() != Some(plugin_id.as_str()));
            self.surface_handlers.remove(plugin_id);
            self.lifecycles.remove(plugin_id);
        }
        self.dependency_status = resolution.statuses;
        self.unavailable = resolution.unavailable;
        resolution.errors
    }

    /// 返回某插件声明的每条依赖的解析状态；未注册或没有依赖时返回空。
    pub fn dependency_status(&self, plugin_id: &str) -> Vec<DependencyStatus> {
        self.dependency_status
            .get(plugin_id)
            .cloned()
            .unwrap_or_default()
    }

    /// 返回所有已注册插件 id（`BTreeMap` 键天然升序）。
    pub fn plugin_ids(&self) -> Vec<String> {
        self.manifests.keys().cloned().collect()
    }

    /// 返回已注册插件的生命周期句柄（用于启动时统一 activate）。
    pub fn plugin_lifecycles(&self) -> Vec<Arc<dyn PluginLifecycle>> {
        self.lifecycles.values().cloned().collect()
    }

    /// 按 id 查找单个动作。
    pub fn find_by_id(
        &self,
        id: &ActionId,
    ) -> Option<(&ActionDescriptor, &Arc<dyn ActionHandler>)> {
        self.entries.get(id).map(|e| (&e.descriptor, &e.handler))
    }

    /// 返回持有给定动作的插件 manifest（裸注册动作返回 None）。
    pub fn plugin_for_action(&self, id: &ActionId) -> Option<&PluginManifest> {
        let plugin_id = self.entries.get(id)?.plugin_id.as_ref()?;
        self.manifests.get(plugin_id)
    }

    /// 返回插件的 Surface 交互处理器（若有注册）。
    pub fn surface_handler_for(&self, plugin_id: &str) -> Option<Arc<dyn SurfaceHandler>> {
        self.surface_handlers.get(plugin_id).cloned()
    }

    /// 返回所有 Public、输入匹配且显式 `search_invocable` 的 [`ActionId`]。
    ///
    /// 这些动作允许在搜索阶段被调用（必须只读、无副作用）：结果为
    /// `ActionOutcome::Data` 时由运行时归一化为执行项。
    pub fn search_invocable_actions(&self, input_kind: InputType) -> Vec<ActionId> {
        self.entries
            .values()
            .filter(|entry| {
                let d = &entry.descriptor;
                d.visibility == flick_domain::ActionVisibility::Public
                    && d.search_invocable
                    && (d.supported_inputs.is_empty() || d.supported_inputs.contains(&input_kind))
            })
            .map(|entry| entry.descriptor.id.clone())
            .collect()
    }

    /// 搜索匹配的动作。
    ///
    /// 当前过渡实现只做关键词召回：将查询词元与每个描述符的关键字
    /// 进行不区分大小写比较。前缀命令匹配将随输入搜索/路由重构另行设计。
    ///
    /// 结果按分数降序排列。
    pub fn search(&self, query: &str, _input_kind: InputType) -> Vec<RouteItem> {
        let query_trimmed = query.trim();
        if query_trimmed.is_empty() {
            return Vec::new();
        }

        let query_lower = query_trimmed.to_ascii_lowercase();
        let tokens: Vec<&str> = query_lower.split_whitespace().collect();
        let mut results: Vec<RouteItem> = Vec::new();

        for entry in self.entries.values() {
            let kw_lower: Vec<String> = entry
                .descriptor
                .keywords
                .iter()
                .map(|k| k.to_ascii_lowercase())
                .collect();

            let mut matched = 0usize;
            for token in &tokens {
                if kw_lower.iter().any(|kw| kw.contains(token)) {
                    matched += 1;
                }
            }

            if matched > 0 {
                let score = matched as f64 / tokens.len() as f64;
                results.push(RouteItem::action(
                    entry.descriptor.id.clone(),
                    entry.descriptor.title.clone(),
                    score,
                ));
            }
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(MAX_ACTION_RESULTS);
        results
    }

    /// 按完整输入上下文搜索候选。
    ///
    /// 与旧的 [`Self::search`] 不同，这里会应用 `visibility`、
    /// `supported_inputs`、`file_extensions` 过滤，并使用分层匹配和
    /// 确定性排序。为避免 `HashMap` 迭代顺序影响结果，先用
    /// `(plugin_id, action_id)` 建立稳定的候选顺序。
    pub fn search_input(&self, context: &InputContext) -> Vec<RouteItem> {
        if context.raw_text.trim().is_empty() {
            return Vec::new();
        }

        let mut entries: Vec<(&ActionId, &Entry)> = self.entries.iter().collect();
        entries.sort_by(|(left_id, left), (right_id, right)| {
            left.plugin_id
                .as_deref()
                .unwrap_or("")
                .cmp(right.plugin_id.as_deref().unwrap_or(""))
                .then_with(|| left_id.as_str().cmp(right_id.as_str()))
        });

        let mut scored: Vec<(usize, ScoredRoute)> = Vec::new();
        for (candidate_order, (_, entry)) in entries.into_iter().enumerate() {
            if entry.descriptor.visibility != ActionVisibility::Public {
                continue;
            }
            let plugin = entry
                .plugin_id
                .as_ref()
                .and_then(|plugin_id| self.manifests.get(plugin_id));
            let Some(action_match) = score_action(&entry.descriptor, plugin, context) else {
                continue;
            };

            let mut item = RouteItem::action(
                entry.descriptor.id.clone(),
                entry.descriptor.title.clone(),
                action_match.score,
            );
            item.facet = action_match.facet;
            item.matched_field = Some(action_match.matched_field);
            item.matched_range = action_match.matched_range;

            scored.push((
                candidate_order,
                ScoredRoute {
                    item,
                    category_rank: action_match.category_rank,
                    title_len: action_match.title_len,
                },
            ));
        }

        // 当前还没有 last_used 存储；保留该排序位，先按 0 处理。
        scored.sort_by(|(left_order, left), (right_order, right)| {
            right
                .item
                .score
                .total_cmp(&left.item.score)
                .then_with(|| left.category_rank.cmp(&right.category_rank))
                .then_with(|| left.title_len.cmp(&right.title_len))
                .then_with(|| left_order.cmp(right_order))
        });

        let mut results: Vec<RouteItem> = scored
            .into_iter()
            .map(|(_, candidate)| candidate.item)
            .collect();
        results.truncate(MAX_ACTION_RESULTS);
        results
    }
}

impl Default for ActionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flick_domain::{ActionDescriptor, ActionId, ConcurrencyPolicy};
    use flick_plugin::ActionSpec;

    /// 用于测试的空操作处理器。
    struct NoopHandler;

    #[async_trait::async_trait]
    impl ActionHandler for NoopHandler {
        async fn invoke(
            &self,
            _req: flick_domain::ActionRequest,
            _ctx: flick_domain::ActionContext,
        ) -> Result<flick_domain::ActionOutcome, flick_domain::ActionError> {
            Ok(flick_domain::ActionOutcome::Notice {
                message: "noop".into(),
            })
        }
    }

    fn echo_descriptor() -> ActionDescriptor {
        ActionDescriptor::new(
            ActionId::from_static("flick.builtin.echo.v1.echo"),
            "Echo",
            vec!["echo".into(), "repeat".into()],
            ConcurrencyPolicy::Parallel,
        )
    }

    fn notes_descriptor() -> ActionDescriptor {
        ActionDescriptor::new(
            ActionId::from_static("flick.builtin.notes.v1.search"),
            "Search Notes",
            vec!["note".into(), "notes".into(), "search".into()],
            ConcurrencyPolicy::Parallel,
        )
    }

    #[test]
    fn find_by_id_returns_registered_action() {
        let mut reg = ActionRegistry::new();
        let desc = echo_descriptor();
        let id = desc.id.clone();
        reg.register(desc, Arc::new(NoopHandler));
        assert!(reg.find_by_id(&id).is_some());
    }

    #[test]
    fn find_by_id_returns_none_for_unknown() {
        let reg = ActionRegistry::new();
        assert!(reg.find_by_id(&ActionId::from_static("unknown")).is_none());
    }

    #[test]
    fn keyword_matches_query_tokens() {
        let mut reg = ActionRegistry::new();
        reg.register(echo_descriptor(), Arc::new(NoopHandler));

        // "echo hello" 应当匹配
        let results = reg.search("echo hello", InputType::PlainText);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].action_id,
            ActionId::from_static("flick.builtin.echo.v1.echo")
        );

        // "echohello" 不含关键词 "echo"（关键词方向是 kw.contains(token)）
        let results = reg.search("echohello", InputType::PlainText);
        assert!(
            results.is_empty(),
            "echohello should not match echo keyword"
        );
    }

    #[test]
    fn keyword_match_works() {
        let mut reg = ActionRegistry::new();
        reg.register(notes_descriptor(), Arc::new(NoopHandler));

        let results = reg.search("notes", InputType::PlainText);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn empty_query_returns_empty() {
        let mut reg = ActionRegistry::new();
        reg.register(echo_descriptor(), Arc::new(NoopHandler));
        assert!(reg.search("", InputType::PlainText).is_empty());
        assert!(reg.search("   ", InputType::PlainText).is_empty());
    }

    fn text_context(text: &str) -> flick_domain::InputContext {
        flick_domain::InputContext::from_snapshot(
            flick_domain::QueryGeneration::INITIAL,
            &flick_domain::SearchInputSnapshot::text(text, flick_domain::InputSource::Manual),
        )
    }

    #[test]
    fn search_input_uses_title_match_and_reports_field() {
        let mut reg = ActionRegistry::new();
        reg.register(echo_descriptor(), Arc::new(NoopHandler));

        let results = reg.search_input(&text_context("echo"));
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].matched_field,
            Some(flick_domain::MatchField::Name)
        );
        assert!(results[0].score >= 800.0);
    }

    #[test]
    fn search_input_filters_internal_and_supported_inputs() {
        let mut internal = echo_descriptor();
        internal.visibility = flick_domain::ActionVisibility::Internal;
        let mut url_only = notes_descriptor();
        url_only.title = "https://example.com".into();
        url_only.keywords = vec!["example".into()];
        url_only.supported_inputs = vec![InputType::Url];

        let mut reg = ActionRegistry::new();
        reg.register(internal, Arc::new(NoopHandler));
        reg.register(url_only, Arc::new(NoopHandler));

        assert!(reg.search_input(&text_context("echo")).is_empty());
        assert!(reg.search_input(&text_context("notes")).is_empty());
        // URL facet 存在时才允许 url-only 动作进入。
        assert_eq!(
            reg.search_input(&text_context("https://example.com")).len(),
            1
        );
    }

    #[test]
    fn search_input_filters_file_extensions() {
        let mut descriptor = echo_descriptor();
        descriptor.title = "main".into();
        descriptor.keywords = vec!["main".into()];
        descriptor.supported_inputs = vec![InputType::File];
        descriptor.file_extensions = vec!["rs".into()];

        let file = flick_domain::FileInput {
            uri: "file:///tmp/main.rs".into(),
            display_name: "main.rs".into(),
            extension: Some("rs".into()),
            is_dir: false,
            mime: Some("text/plain".into()),
        };
        let mut reg = ActionRegistry::new();
        reg.register(descriptor, Arc::new(NoopHandler));
        let context = flick_domain::InputContext::from_snapshot(
            flick_domain::QueryGeneration::INITIAL,
            &flick_domain::SearchInputSnapshot {
                kind: flick_domain::UiInputKind::Files,
                source: flick_domain::InputSource::DragDrop,
                text: "main".into(),
                files: vec![file],
                image: None,
            },
        );
        assert_eq!(reg.search_input(&context).len(), 1);
    }

    #[test]
    fn search_input_plugin_keyword_expands_public_actions() {
        let manifest = PluginManifest {
            id: "demo.plugin".into(),
            name: "Demo".into(),
            author: "Demo".into(),
            version: "1.0.0".into(),
            kind: flick_domain::PluginKind::BuiltInRust,
            keywords: vec!["demo".into()],
            icon: None,
            profile: flick_domain::PluginProfile::default(),
            permissions: vec![],
            plugin_dependencies: vec![],
            settings: vec![],
            surfaces: vec![],
            supported_os: vec![],
            supported_architectures: vec![],
            needs_search: false,
        };
        let descriptor = ActionDescriptor::new(
            ActionId::from_static("demo.plugin.v1.run"),
            "Unrelated",
            vec![],
            ConcurrencyPolicy::Parallel,
        );
        let mut entries = HashMap::new();
        entries.insert(
            descriptor.id.clone(),
            Entry {
                descriptor,
                handler: Arc::new(NoopHandler),
                plugin_id: Some("demo.plugin".into()),
            },
        );
        let mut manifests = BTreeMap::new();
        manifests.insert("demo.plugin".into(), manifest);
        let reg = ActionRegistry {
            entries,
            manifests,
            surface_handlers: HashMap::new(),
            lifecycles: HashMap::new(),
            dependency_status: HashMap::new(),
            unavailable: HashSet::new(),
        };

        let results = reg.search_input(&text_context("demo"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Unrelated");
    }

    /// 构造一个带依赖声明的最小插件 manifest。
    fn plugin_manifest(
        id: &str,
        dependencies: Vec<(&str, flick_domain::DependencyRequirement)>,
    ) -> PluginManifest {
        PluginManifest {
            id: id.into(),
            name: id.into(),
            author: "Flick".into(),
            version: "1.0.0".into(),
            kind: flick_domain::PluginKind::BuiltInRust,
            keywords: vec![],
            icon: None,
            profile: flick_domain::PluginProfile::default(),
            permissions: vec![],
            plugin_dependencies: dependencies
                .into_iter()
                .map(|(plugin_id, requirement)| flick_domain::PluginDependency {
                    plugin_id: plugin_id.into(),
                    requirement,
                })
                .collect(),
            settings: vec![],
            surfaces: vec![],
            supported_os: vec![],
            supported_architectures: vec![],
            needs_search: false,
        }
    }

    /// 测试用插件替身：manifest + 动作集。
    struct TestPlugin {
        manifest: PluginManifest,
        actions: Vec<ActionSpec>,
    }

    impl Plugin for TestPlugin {
        fn manifest(&self) -> PluginManifest {
            self.manifest.clone()
        }
        fn actions(&self) -> Vec<ActionSpec> {
            self.actions
                .iter()
                .map(|spec| ActionSpec {
                    descriptor: spec.descriptor.clone(),
                    handler: spec.handler.clone(),
                })
                .collect()
        }
    }

    #[test]
    fn search_input_orders_equal_scores_by_action_id() {
        let mut reg = ActionRegistry::new();
        for id in ["demo.plugin.v1.b", "demo.plugin.v1.a"] {
            let descriptor = ActionDescriptor::new(
                ActionId::from_static(id),
                "Same",
                vec!["same".into()],
                ConcurrencyPolicy::Parallel,
            );
            reg.register(descriptor, Arc::new(NoopHandler));
        }
        let results = reg.search_input(&text_context("same"));
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].action_id.as_str(), "demo.plugin.v1.a");
        assert_eq!(results[1].action_id.as_str(), "demo.plugin.v1.b");
    }

    #[test]
    fn resolve_dependencies_removes_actions_of_unavailable_plugin() {
        let mut reg = ActionRegistry::new();
        let dependent = TestPlugin {
            manifest: plugin_manifest(
                "demo.dependent",
                vec![(
                    "demo.missing",
                    flick_domain::DependencyRequirement::Required,
                )],
            ),
            actions: vec![ActionSpec {
                descriptor: ActionDescriptor::new(
                    ActionId::from_static("demo.dependent.v1.run"),
                    "Run",
                    vec!["run".into()],
                    ConcurrencyPolicy::Parallel,
                ),
                handler: Arc::new(NoopHandler),
            }],
        };
        reg.register_plugin(&dependent).unwrap();
        let action_id = ActionId::from_static("demo.dependent.v1.run");
        assert!(reg.find_by_id(&action_id).is_some());

        let errors = reg.resolve_dependencies();

        assert_eq!(errors.len(), 1);
        assert!(
            reg.find_by_id(&action_id).is_none(),
            "unavailable plugin actions must be removed"
        );
        // manifest 保留：诊断需要区分「未安装」与「依赖不满足」。
        assert!(reg.plugin_ids().contains(&"demo.dependent".to_string()));
        assert!(!reg.dependency_status("demo.dependent")[0].available);
    }

    #[test]
    fn resolve_dependencies_is_idempotent() {
        let mut reg = ActionRegistry::new();
        let dependent = TestPlugin {
            manifest: plugin_manifest(
                "demo.dependent",
                vec![(
                    "demo.missing",
                    flick_domain::DependencyRequirement::Required,
                )],
            ),
            actions: vec![ActionSpec {
                descriptor: ActionDescriptor::new(
                    ActionId::from_static("demo.dependent.v1.run"),
                    "Run",
                    vec!["run".into()],
                    ConcurrencyPolicy::Parallel,
                ),
                handler: Arc::new(NoopHandler),
            }],
        };
        reg.register_plugin(&dependent).unwrap();
        let first = reg.resolve_dependencies();
        let second = reg.resolve_dependencies();
        assert_eq!(first, second);
        assert_eq!(reg.plugin_ids().len(), 1);
        assert!(
            reg.find_by_id(&ActionId::from_static("demo.dependent.v1.run"))
                .is_none()
        );
    }
}
