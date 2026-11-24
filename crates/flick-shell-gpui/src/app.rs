//! 纯逻辑更新层：把 UI/宿主事件归约为面板事件，并计算窗口尺寸。
//! 不依赖任何 UI 框架，可直接单测。

use flick_application::presenter::{PanelAction, PanelEvent, PanelState};
use crate::theme::{Metrics, ThemeMode};
use flick_domain::{HostToUi, UiToHost};
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
}

/// app 层要求视图层执行的效果。
#[derive(Debug, PartialEq)]
pub enum Effect {
    Send(UiToHost),
    StartDebounce { revision: u64 },
    Quit,
}

/// 面板状态 + 主题。
pub struct AppState {
    pub panel: PanelState,
    pub theme: ThemeMode,
}

impl AppState {
    pub fn new(theme: ThemeMode) -> Self {
        Self {
            panel: PanelState::new(),
            theme,
        }
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
                    HostToUi::Shutdown => unreachable!("handled above"),
                };
                self.apply_panel_event(panel_event)
            }
            UiEvent::InputChanged(text) => {
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

/// 由内容计算逻辑窗口尺寸（宽, 高）。修复 iced 缺陷 #2/#4 的核心公式。
pub fn desired_size(panel: &PanelState, m: &Metrics) -> (f32, f32) {
    let has_results = !(panel.routes.is_empty() && panel.last_outcome.is_none());
    let results_h = if has_results {
        let rows = panel.routes.len().max(1) as f32;
        let raw = rows * m.row_height + (rows - 1.0) * m.row_gap;
        raw.min(m.results_max_height)
    } else {
        0.0
    };
    let gap = if results_h > 0.0 { m.content_gap } else { 0.0 };
    let height = (m.input_height + gap + results_h).min(m.panel_max_height);
    (m.width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flick_domain::{ActionId, RouteItem};

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
            .map(|i| RouteItem {
                action_id: ActionId::new(format!("a{i}")).unwrap(),
                title: format!("T{i}"),
                score: 1.0,
            })
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
        assert!(!a
            .iter()
            .any(|e| matches!(e, Effect::Send(UiToHost::QueryChanged { .. }))));

        let current = s.panel.debounce_revision();
        let b = s.update(UiEvent::DebounceElapsed { revision: current });
        assert!(b.iter().any(|e| matches!(
            e,
            Effect::Send(UiToHost::QueryChanged { text, .. }) if text == "echo"
        )));
    }
}
