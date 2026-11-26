//! 纯逻辑更新层：把 UI/宿主事件归约为面板事件，并计算窗口尺寸。
//! 不依赖任何 UI 框架，可直接单测。

use crate::theme::{Metrics, ThemeMode};
use flick_application::presenter::{PanelAction, PanelEvent, PanelState};
use flick_domain::{
    ActionOutcome, HostToUi, SearchInputSnapshot, SurfaceContent, SurfaceSessionId, UiToHost,
};
use flick_ui::style::{SurfaceStyle, estimate_height};
use std::time::Instant;

/// 输入防抖窗口（毫秒）。仅决定何时发协议，不决定显示。
pub const DEBOUNCE_MILLIS: u64 = 150;

/// 进入 app 层的事件。
#[derive(Debug, Clone)]
pub enum UiEvent {
    InputChanged(String),
    Submitted,
    EscapePressed,
    SelectionUp,
    SelectionDown,
    DebounceElapsed { revision: u64 },
    Host(HostToUi),
    WindowClosed,
    ToggleTheme,
    DetachRequested,
}

/// 把一个自带会话的 Surface 分离为独立窗口所需的负载。
#[derive(Debug, Clone, PartialEq)]
pub struct DetachPayload {
    pub session_id: SurfaceSessionId,
    pub content: SurfaceContent,
    pub needs_search: bool,
}

/// app 层要求视图层执行的效果。
#[derive(Debug, PartialEq)]
pub enum Effect {
    Send(UiToHost),
    StartDebounce { revision: u64 },
    Quit,
    Detach(DetachPayload),
}

/// 面板状态 + 主题。
pub struct AppState {
    pub panel: PanelState,
    pub theme: ThemeMode,
    /// 最近一次 Surface 交互失败的错误消息（面板会话内展示）。
    pub surface_error: Option<String>,
}

impl AppState {
    pub fn new(theme: ThemeMode) -> Self {
        Self {
            panel: PanelState::new(),
            theme,
            surface_error: None,
        }
    }

    /// 当前面板的 Surface 交互错误消息（若有）。
    pub fn surface_error(&self) -> Option<&str> {
        self.surface_error.as_deref()
    }

    /// 应用一条事件，返回视图层需执行的效果。
    pub fn update(&mut self, event: UiEvent) -> Vec<Effect> {
        match event {
            UiEvent::ToggleTheme => {
                self.theme = self.theme.toggled();
                Vec::new()
            }
            UiEvent::Host(HostToUi::Shutdown) => vec![Effect::Quit],
            UiEvent::Host(host) => {
                let panel_event = match host {
                    HostToUi::ShowSearchPanel { activation_token } => {
                        self.surface_error = None;
                        PanelEvent::Show { activation_token }
                    }
                    HostToUi::HideSearchPanel => PanelEvent::Hide,
                    HostToUi::Routes { generation, items } => {
                        PanelEvent::Routes { generation, items }
                    }
                    HostToUi::InvocationCompleted {
                        invocation_id,
                        outcome,
                    } => PanelEvent::InvocationCompleted {
                        invocation_id,
                        outcome,
                    },
                    HostToUi::RuntimeStatus(status) => PanelEvent::RuntimeStatus(status),
                    HostToUi::SurfaceUpdated { session_id, content } => {
                        // 仅当更新属于面板当前会话时才进入面板状态；
                        // 分离窗口的更新由 lib.rs 的窗口泵处理。
                        if self.panel.surface_session() == Some(session_id) {
                            self.surface_error = None;
                            PanelEvent::SurfaceUpdated { content }
                        } else {
                            return Vec::new();
                        }
                    }
                    HostToUi::SurfaceFailed { session_id, message } => {
                        if self.panel.surface_session() == Some(session_id) {
                            self.surface_error = Some(message);
                        }
                        return Vec::new();
                    }
                    HostToUi::Shutdown => unreachable!("handled above"),
                };
                self.apply_panel_event(panel_event)
            }
            UiEvent::InputChanged(text) => {
                self.surface_error = None;
                self.apply_panel_event(PanelEvent::InputChanged(text))
            }
            UiEvent::Submitted => self.apply_panel_event(PanelEvent::Submitted),
            UiEvent::EscapePressed => self.apply_panel_event(PanelEvent::EscapePressed),
            UiEvent::SelectionUp => self.apply_panel_event(PanelEvent::SelectionUp),
            UiEvent::SelectionDown => self.apply_panel_event(PanelEvent::SelectionDown),
            UiEvent::DebounceElapsed { revision } => {
                self.apply_panel_event(PanelEvent::DebounceFlush { revision })
            }
            UiEvent::WindowClosed => self.apply_panel_event(PanelEvent::WindowClosed),
            UiEvent::DetachRequested => {
                if let Some(ActionOutcome::OpenSurface {
                    content,
                    session_id,
                    needs_search,
                    ..
                }) = &self.panel.last_outcome
                {
                    let renderable = !content.body.is_empty() || content.title.is_some();
                    if let Some(session_id) = self.panel.surface_session().or(*session_id) {
                        if renderable {
                            return vec![Effect::Detach(DetachPayload {
                                session_id,
                                content: content.clone(),
                                needs_search: *needs_search,
                            })];
                        }
                    }
                }
                Vec::new()
            }
        }
    }

    fn apply_panel_event(&mut self, event: PanelEvent) -> Vec<Effect> {
        let actions = self.panel.update_at(event, Instant::now());
        actions
            .into_iter()
            .map(|action| match action {
                PanelAction::SendToHost(message) => Effect::Send(message),
                PanelAction::StartDebounce { revision } => {
                    Effect::StartDebounce { revision }
                }
            })
            .collect()
    }
}

/// 若最近一次调用打开了 Surface，返回其内容。
pub fn active_surface(panel: &PanelState) -> Option<&SurfaceContent> {
    match &panel.last_outcome {
        Some(ActionOutcome::OpenSurface { content, .. }) => {
            // body 非空，或仅有标题（title-only）的 Surface 也可渲染。
            (!content.body.is_empty() || content.title.is_some()).then_some(content)
        }
        _ => None,
    }
}

/// 由内容计算逻辑窗口尺寸（宽, 高）。修复 iced 缺陷 #2/#4 的核心公式。
pub fn desired_size(panel: &PanelState, m: &Metrics) -> (f32, f32) {
    // Surface 优先：内容即当前动作的输出视图，替换结果列表。
    if let Some(surface) = active_surface(panel) {
        let content_h = estimate_height(surface, &SurfaceStyle::default());
        let height = (m.input_height + m.content_gap + content_h + m.padding_bottom)
            .min(m.panel_max_height);
        return (m.width, height.max(m.input_height));
    }
    let has_results = !(panel.routes.is_empty() && panel.last_outcome.is_none());
    let results_h = if has_results {
        let rows = panel.routes.len().max(1) as f32;
        let raw = rows * m.row_height + (rows - 1.0) * m.row_gap;
        raw.min(m.results_max_height)
    } else {
        0.0
    };
    let gap = if results_h > 0.0 { m.content_gap } else { 0.0 };
    let bottom = if results_h > 0.0 { m.padding_bottom } else { 0.0 };
    let height = (m.input_height + gap + results_h + bottom).min(m.panel_max_height);
    (m.width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flick_domain::{ActionId, RouteItem};
    use flick_domain::{ActionOutcome, SurfaceContent, SurfaceNode, SURFACE_SCHEMA_VERSION};

    fn state() -> AppState {
        AppState::new(ThemeMode::Dark)
    }

    #[test]
    fn empty_panel_is_input_only_height() {
        let s = state();
        let (w, h) = desired_size(&s.panel, &Metrics::default());
        assert_eq!(w, 666.0);
        assert_eq!(h, 64.0);
    }

    #[test]
    fn desired_size_clamps_to_max_height() {
        let mut s = state();
        s.panel.routes = (0..20)
            .map(|i| RouteItem::action(
                ActionId::new(format!("a{i}")).unwrap(),
                format!("T{i}"),
                1.0,
            ))
            .collect();
        let (_w, h) = desired_size(&s.panel, &Metrics::default());
        assert_eq!(h, 480.0);
    }

    #[test]
    fn toggle_theme_flips() {
        let mut s = state();
        s.update(UiEvent::ToggleTheme);
        assert_eq!(s.theme, ThemeMode::Light);
        s.update(UiEvent::ToggleTheme);
        assert_eq!(s.theme, ThemeMode::Dark);
    }

    #[test]
    fn escape_sends_search_panel_closed() {
        let mut s = state();
        s.update(UiEvent::Host(HostToUi::ShowSearchPanel {
            activation_token: None,
        }));
        let effects = s.update(UiEvent::EscapePressed);
        assert!(effects
            .iter()
            .any(|e| matches!(e, Effect::Send(UiToHost::SearchPanelClosed))));
    }

    #[test]
    fn obsolete_debounce_does_not_send_query() {
        let mut s = state();
        s.update(UiEvent::Host(HostToUi::ShowSearchPanel {
            activation_token: None,
        }));
        s.update(UiEvent::InputChanged("ec".into()));
        let obsolete = s.panel.debounce_revision();
        s.update(UiEvent::InputChanged("echo".into()));

        let a = s.update(UiEvent::DebounceElapsed { revision: obsolete });
        assert!(
            !a.iter()
                .any(|e| matches!(e, Effect::Send(UiToHost::QueryChanged { .. })))
        );

        let current = s.panel.debounce_revision();
        let b = s.update(UiEvent::DebounceElapsed { revision: current });
        assert!(b.iter().any(|e| matches!(
            e,
            Effect::Send(UiToHost::QueryChanged { input, .. }) if input.text == "echo"
        )));
    }

    #[test]
    fn active_surface_none_without_outcome() {
        let s = state();
        assert!(active_surface(&s.panel).is_none());
    }

    #[test]
    fn desired_size_uses_surface_estimate() {
        let mut s = state();
        s.panel.last_outcome = Some(ActionOutcome::OpenSurface {
            surface_id: "info".into(),
            content: SurfaceContent {
                schema_version: 1,
                title: None,
                body: vec![SurfaceNode::Heading { id: None, text: "系统信息".into() }],
            },
            session_id: None,
            needs_search: false,
        });
        assert!(active_surface(&s.panel).is_some());
        let (_w, h) = desired_size(&s.panel, &Metrics::default());
        assert!(h > 64.0 && h <= 480.0);
    }

    #[test]
    fn active_surface_some_with_title_only() {
        let mut s = state();
        s.panel.last_outcome = Some(ActionOutcome::OpenSurface {
            surface_id: "info".into(),
            content: SurfaceContent {
                schema_version: SURFACE_SCHEMA_VERSION,
                title: Some("系统信息".into()),
                body: vec![],
            },
            session_id: None,
            needs_search: false,
        });
        // 仅有标题、body 为空的 Surface 也应可渲染。
        assert!(active_surface(&s.panel).is_some());
    }

    #[test]
    fn detach_effect_only_with_surface() {
        let mut s = state();
        // 无 Surface：不产出 Detach。
        let none = s.update(UiEvent::DetachRequested);
        assert!(!none.iter().any(|e| matches!(e, Effect::Detach(_))));

        // 有 Surface：产出一个 Detach，内容与会话一致。
        let content = SurfaceContent {
            schema_version: SURFACE_SCHEMA_VERSION,
            title: Some("系统信息".into()),
            body: vec![SurfaceNode::Heading { id: None, text: "系统信息".into() }],
        };
        let session_id = SurfaceSessionId::new();
        s.panel.last_outcome = Some(ActionOutcome::OpenSurface {
            surface_id: "info".into(),
            content: content.clone(),
            session_id: Some(session_id),
            needs_search: true,
        });
        let effects = s.update(UiEvent::DetachRequested);
        let payload = effects
            .iter()
            .find_map(|e| match e {
                Effect::Detach(p) => Some(p),
                _ => None,
            })
            .expect("should produce Effect::Detach");
        assert_eq!(payload.content, content);
        assert_eq!(payload.session_id, session_id);
        assert!(payload.needs_search);
    }

    #[test]
    fn active_surface_none_with_empty_body() {
        let mut s = state();
        s.panel.last_outcome = Some(ActionOutcome::OpenSurface {
            surface_id: "info".into(),
            content: SurfaceContent {
                schema_version: SURFACE_SCHEMA_VERSION,
                title: None,
                body: vec![],
            },
            session_id: None,
            needs_search: false,
        });
        assert!(active_surface(&s.panel).is_none());
    }

    fn surface_with_text(text: &str) -> SurfaceContent {
        SurfaceContent {
            schema_version: SURFACE_SCHEMA_VERSION,
            title: Some("剪贴板".into()),
            body: vec![SurfaceNode::Text {
                id: None,
                text: text.into(),
                muted: false,
            }],
        }
    }

    /// 通过发 Invoke + InvocationCompleted 建立面板 Surface 会话。
    fn enter_surface_session(s: &mut AppState, session: SurfaceSessionId, needs_search: bool) {
        s.update(UiEvent::Host(HostToUi::ShowSearchPanel {
            activation_token: None,
        }));
        s.panel.query = "clipboard".into();
        s.panel.routes = vec![RouteItem::action(
            ActionId::new("demo.builtin.clipboard.show").unwrap(),
            "剪贴板",
            1.0,
        )];
        s.panel.selection.set_total(1);
        let effects = s.update(UiEvent::Submitted);
        let invocation_id = effects
            .iter()
            .find_map(|e| match e {
                Effect::Send(UiToHost::Invoke { invocation_id, .. }) => Some(*invocation_id),
                _ => None,
            })
            .expect("submit should invoke");
        s.update(UiEvent::Host(HostToUi::InvocationCompleted {
            invocation_id,
            outcome: ActionOutcome::OpenSurface {
                surface_id: "clipboard".into(),
                content: surface_with_text("old"),
                session_id: Some(session),
                needs_search,
            },
        }));
    }

    #[test]
    fn surface_updated_for_panel_session_updates_content() {
        let mut s = state();
        let session = SurfaceSessionId::new();
        enter_surface_session(&mut s, session, true);
        assert_eq!(s.panel.surface_session(), Some(session));

        let updated = surface_with_text("new");
        let effects = s.update(UiEvent::Host(HostToUi::SurfaceUpdated {
            session_id: session,
            content: updated.clone(),
        }));
        assert!(effects.is_empty());
        assert_eq!(active_surface(&s.panel), Some(&updated));
    }

    #[test]
    fn surface_updated_other_session_is_ignored() {
        let mut s = state();
        let session = SurfaceSessionId::new();
        enter_surface_session(&mut s, session, false);
        let before = active_surface(&s.panel).cloned();

        let effects = s.update(UiEvent::Host(HostToUi::SurfaceUpdated {
            session_id: SurfaceSessionId::new(),
            content: surface_with_text("other"),
        }));
        assert!(effects.is_empty());
        assert_eq!(active_surface(&s.panel), before.as_ref());
    }

    #[test]
    fn surface_failed_sets_error_and_input_clears_it() {
        let mut s = state();
        let session = SurfaceSessionId::new();
        enter_surface_session(&mut s, session, true);

        let effects = s.update(UiEvent::Host(HostToUi::SurfaceFailed {
            session_id: session,
            message: "boom".into(),
        }));
        assert!(effects.is_empty());
        assert_eq!(s.surface_error(), Some("boom"));

        s.update(UiEvent::InputChanged("next".into()));
        assert_eq!(s.surface_error(), None);
    }

    #[test]
    fn surface_failed_other_session_is_ignored() {
        let mut s = state();
        let session = SurfaceSessionId::new();
        enter_surface_session(&mut s, session, true);

        s.update(UiEvent::Host(HostToUi::SurfaceFailed {
            session_id: SurfaceSessionId::new(),
            message: "ignored".into(),
        }));
        assert_eq!(s.surface_error(), None);
    }
}
