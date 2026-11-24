//! ActionRegistry — 存储与其处理器配对的 ActionDescriptors。

use crate::handler::ActionHandler;
use flick_domain::{ActionDescriptor, ActionId, InputKind, RouteItem};
use std::collections::HashMap;
use std::sync::Arc;

/// 注册表中的一个条目：描述符 + 处理器。
struct Entry {
    descriptor: ActionDescriptor,
    handler: Arc<dyn ActionHandler>,
}

/// 将 [`ActionId`] 映射到（[`ActionDescriptor`]，[`ActionHandler`]）。
///
/// 提供关键字搜索和前缀命令匹配。完整的
/// 注册表**绝不**暴露给调用方 — 只有 `find_by_id` 和
/// `search` 可用。
pub struct ActionRegistry {
    entries: HashMap<ActionId, Entry>,
}

impl ActionRegistry {
    /// 创建一个空的注册表。
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// 注册一个新动作。
    ///
    /// 如果已存在具有相同 [`ActionId`] 的动作，则将其
    /// 替换，并返回旧的处理器（包装为 `Ok`）。
    pub fn register(&mut self, descriptor: ActionDescriptor, handler: Arc<dyn ActionHandler>) {
        let id = descriptor.id.clone();
        self.entries.insert(
            id,
            Entry {
                descriptor,
                handler,
            },
        );
    }

    /// 按 id 查找单个动作。
    pub fn find_by_id(
        &self,
        id: &ActionId,
    ) -> Option<(&ActionDescriptor, &Arc<dyn ActionHandler>)> {
        self.entries.get(id).map(|e| (&e.descriptor, &e.handler))
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
    pub fn search(&self, query: &str, _input_kind: InputKind) -> Vec<RouteItem> {
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
        ActionDescriptor {
            id: ActionId::from_static("flick.builtin.echo.v1.echo"),
            title: "Echo".into(),
            keywords: vec!["echo".into(), "repeat".into()],
            matcher: Matcher::prefix_command("echo"),
            concurrency: ConcurrencyPolicy::Parallel,
        }
    }

    fn notes_descriptor() -> ActionDescriptor {
        ActionDescriptor {
            id: ActionId::from_static("flick.builtin.notes.v1.search"),
            title: "Search Notes".into(),
            keywords: vec!["note".into(), "notes".into(), "search".into()],
            matcher: Matcher::Keyword,
            concurrency: ConcurrencyPolicy::Parallel,
        }
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
        let results = reg.search("echo hello", InputKind::PlainText);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].action_id,
            ActionId::from_static("flick.builtin.echo.v1.echo")
        );

        // "echohello" 不应匹配（无词边界）
        let results = reg.search("echohello", InputKind::PlainText);
        assert!(results.is_empty(), "echohello should not match echo prefix");
    }

    #[test]
    fn keyword_match_works() {
        let mut reg = ActionRegistry::new();
        reg.register(notes_descriptor(), Arc::new(NoopHandler));

        let results = reg.search("notes", InputKind::PlainText);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn empty_query_returns_empty() {
        let mut reg = ActionRegistry::new();
        reg.register(echo_descriptor(), Arc::new(NoopHandler));
        assert!(reg.search("", InputKind::PlainText).is_empty());
        assert!(reg.search("   ", InputKind::PlainText).is_empty());
    }
}
