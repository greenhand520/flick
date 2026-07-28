//! ActionRegistry — stores ActionDescriptors paired with their handlers.

use crate::handler::ActionHandler;
use flick_domain::{ActionDescriptor, ActionId, InputKind, RouteItem};
use std::collections::HashMap;
use std::sync::Arc;

/// An entry in the registry: descriptor + handler.
struct Entry {
    descriptor: ActionDescriptor,
    handler: Arc<dyn ActionHandler>,
}

/// Maps [`ActionId`] → ([`ActionDescriptor`], [`ActionHandler`]).
///
/// Provides keyword search and prefix-command matching.  The full
/// registry is **never** exposed to callers — only `find_by_id` and
/// `search` are available.
pub struct ActionRegistry {
    entries: HashMap<ActionId, Entry>,
}

impl ActionRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Register a new action.
    ///
    /// If an action with the same [`ActionId`] already exists it is
    /// replaced and the old handler is returned (wrapped as `Ok`).
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

    /// Look up a single action by id.
    pub fn find_by_id(
        &self,
        id: &ActionId,
    ) -> Option<(&ActionDescriptor, &Arc<dyn ActionHandler>)> {
        self.entries.get(id).map(|e| (&e.descriptor, &e.handler))
    }

    /// Search for matching actions.
    ///
    /// Two matching strategies run in order:
    ///
    /// 1. **Prefix-command match** — if `query` starts with a
    ///    registered `PrefixCommand { command }` **and** the character
    ///    immediately after the command is end-of-string or whitespace
    ///    (word-boundary), the action is returned with a high score.
    /// 2. **Keyword match** — the query tokens are compared
    ///    case-insensitively against each descriptor's keywords.
    ///
    /// Results are sorted by descending score.
    pub fn search(&self, query: &str, _input_kind: InputKind) -> Vec<RouteItem> {
        let query_trimmed = query.trim();
        if query_trimmed.is_empty() {
            return Vec::new();
        }

        let query_lower = query_trimmed.to_ascii_lowercase();
        let mut results: Vec<RouteItem> = Vec::new();

        for entry in self.entries.values() {
            // ── 1. Prefix-command matching ──────────────────────────
            if let flick_domain::Matcher::PrefixCommand { command } = &entry.descriptor.matcher {
                let cmd_lower = command.to_ascii_lowercase();
                if query_lower.starts_with(&cmd_lower) {
                    // Word boundary check: next char must be space or end-of-string
                    let rest = &query_lower[cmd_lower.len()..];
                    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                        results.push(RouteItem {
                            action_id: entry.descriptor.id.clone(),
                            title: entry.descriptor.title.clone(),
                            score: 1.0, // highest priority
                        });
                        continue;
                    }
                }
            }

            // ── 2. Keyword matching ─────────────────────────────────
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

    /// A no-op handler for testing.
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

        // "echo hello" should match
        let results = reg.search("echo hello", InputKind::PlainText);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].action_id,
            ActionId::from_static("flick.builtin.echo.v1.echo")
        );

        // "echohello" should NOT match (no word boundary)
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
