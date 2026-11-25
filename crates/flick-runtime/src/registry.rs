//! ActionRegistry — 存储与其处理器配对的 ActionDescriptors。

use crate::handler::ActionHandler;
use crate::plugin::{Plugin, PluginRegistrationError, SurfaceHandler};
use flick_domain::{ActionDescriptor, ActionId, InputType, PluginManifest, RouteItem};
use std::collections::HashMap;
use std::sync::Arc;

/// 注册表中的一个条目：描述符 + 处理器 + 所属插件 id（裸注册时为 None）。
struct Entry {
    descriptor: ActionDescriptor,
    handler: Arc<dyn ActionHandler>,
    plugin_id: Option<String>,
}

/// 将 [`ActionId`] 映射到（[`ActionDescriptor`]，[`ActionHandler`]）。
///
/// 提供关键字搜索和前缀命令匹配。完整的
/// 注册表**绝不**暴露给调用方 — 只有 `find_by_id` 和
/// `search` 可用。
pub struct ActionRegistry {
    entries: HashMap<ActionId, Entry>,
    /// 插件 id → manifest（仅通过 `register_plugin` 注册的插件）。
    manifests: HashMap<String, PluginManifest>,
    /// 插件 id → Surface 交互处理器。
    surface_handlers: HashMap<String, Arc<dyn SurfaceHandler>>,
}

impl ActionRegistry {
    /// 创建一个空的注册表。
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            manifests: HashMap::new(),
            surface_handlers: HashMap::new(),
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
    pub fn register_plugin(
        &mut self,
        plugin: &dyn Plugin,
    ) -> Result<(), PluginRegistrationError> {
        let manifest = plugin.manifest();
        if manifest.id.trim().is_empty() {
            return Err(PluginRegistrationError::InvalidManifest {
                reason: "empty plugin id".into(),
            });
        }
        // 设置 key 唯一
        let mut seen_keys = std::collections::HashSet::new();
        for s in &manifest.settings {
            if !seen_keys.insert(s.key.as_str()) {
                return Err(PluginRegistrationError::DuplicateSettingKey {
                    key: s.key.clone(),
                });
            }
        }
        // surface id 唯一
        let mut seen_surfaces = std::collections::HashSet::new();
        for s in &manifest.surfaces {
            if !seen_surfaces.insert(s.id.as_str()) {
                return Err(PluginRegistrationError::SurfaceIdCollision {
                    surface_id: s.id.clone(),
                });
            }
        }
        let actions = plugin.actions();
        if actions.is_empty() {
            return Err(PluginRegistrationError::NoActions);
        }
        let prefix = format!("{}.", manifest.id);
        // 先全量校验，再插入（避免半写入）
        for spec in &actions {
            let id = spec.descriptor.id.as_str();
            if !id.starts_with(&prefix) {
                return Err(PluginRegistrationError::NamespaceViolation {
                    action_id: id.to_string(),
                    plugin_id: manifest.id.clone(),
                });
            }
            if self.entries.contains_key(&spec.descriptor.id) {
                return Err(PluginRegistrationError::IdCollision {
                    action_id: id.to_string(),
                });
            }
        }
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
            self.surface_handlers
                .insert(manifest.id.clone(), handler);
        }
        self.manifests.insert(manifest.id.clone(), manifest);
        Ok(())
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

    /// 搜索匹配的动作。
    ///
    /// 依次执行两种匹配策略：
    ///
    /// 1. **前缀命令匹配** — 如果 `query` 以已注册的
    ///    `PrefixCommand { command }` 开头，**并且**紧接在命令
    ///    之后的字符是字符串结尾或空白字符
    ///    （词边界），则该动作会以高分返回。
    /// 2. **关键字匹配** — 将查询词元与每个描述符的关键字
    ///    进行不区分大小写的比较。
    ///
    /// 结果按分数降序排列。
    pub fn search(&self, query: &str, _input_kind: InputType) -> Vec<RouteItem> {
        let query_trimmed = query.trim();
        if query_trimmed.is_empty() {
            return Vec::new();
        }

        let query_lower = query_trimmed.to_ascii_lowercase();
        let mut results: Vec<RouteItem> = Vec::new();

        for entry in self.entries.values() {
            // ── 1. 前缀命令匹配 ──────────────────────────
            if let flick_domain::Matcher::PrefixCommand { command } = &entry.descriptor.matcher {
                let cmd_lower = command.to_ascii_lowercase();
                if query_lower.starts_with(&cmd_lower) {
                    // 词边界检查：下一个字符必须是空格或字符串结尾
                    let rest = &query_lower[cmd_lower.len()..];
                    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                        results.push(RouteItem {
                            action_id: entry.descriptor.id.clone(),
                            title: entry.descriptor.title.clone(),
                            score: 1.0, // 最高优先级
                        });
                        continue;
                    }
                }
            }

            // ── 2. 关键字匹配 ─────────────────────────────────
            if matches!(entry.descriptor.matcher, flick_domain::Matcher::Keyword) {
                let tokens: Vec<&str> = query_lower.split_whitespace().collect();
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
                    results.push(RouteItem {
                        action_id: entry.descriptor.id.clone(),
                        title: entry.descriptor.title.clone(),
                        score,
                    });
                }
            }
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
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
    use flick_domain::{ActionDescriptor, ActionId, ConcurrencyPolicy, Matcher};

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
            Matcher::prefix_command("echo"),
            ConcurrencyPolicy::Parallel,
        )
    }

    fn notes_descriptor() -> ActionDescriptor {
        ActionDescriptor::new(
            ActionId::from_static("flick.builtin.notes.v1.search"),
            "Search Notes",
            vec!["note".into(), "notes".into(), "search".into()],
            Matcher::Keyword,
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
    fn prefix_command_matches_with_word_boundary() {
        let mut reg = ActionRegistry::new();
        reg.register(echo_descriptor(), Arc::new(NoopHandler));

        // "echo hello" 应当匹配
        let results = reg.search("echo hello", InputType::PlainText);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].action_id,
            ActionId::from_static("flick.builtin.echo.v1.echo")
        );

        // "echohello" 不应匹配（无词边界）
        let results = reg.search("echohello", InputType::PlainText);
        assert!(results.is_empty(), "echohello should not match echo prefix");
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
}
