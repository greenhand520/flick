//! Iced daemon application for the Flick search panel.
//!
//! The daemon starts without a window. Host events open and close the panel;
//! Iced keyboard/window events are translated into the same [`AppMessage`] path.

use crate::panel::{PanelAction, PanelEvent, PanelState};
use flick_domain::*;
use iced::widget::{column, container, scrollable, text, text_input};
use iced::{Element, Length, Subscription, Task, Theme};
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub const PANEL_WIDTH: f32 = 480.0;
pub const INPUT_ONLY_HEIGHT: f32 = 72.0;
pub const MAX_PANEL_HEIGHT: f32 = 360.0;
const CONTENT_GAP: f32 = 8.0;
const RESULT_ROW_HEIGHT: f32 = 32.0;
const RESULT_VERTICAL_PADDING: f32 = 32.0;
const MAX_RESULTS_HEIGHT: f32 = 200.0;

/// Whether the panel needs a results/outcome section below its text input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultsArea {
    Hidden,
    Visible,
}

/// Returns whether routes or an invocation outcome require result UI.
pub fn results_area(panel: &PanelState) -> ResultsArea {
    if panel.routes.is_empty() && panel.last_outcome.is_none() {
        ResultsArea::Hidden
    } else {
        ResultsArea::Visible
    }
}

/// Height reserved for results/outcome content when it is visible.
///
/// This is intentionally shared by [`panel_window_size`] and [`results_view`]:
/// a small result set must not open a short window containing a fixed 200 px
/// scrollable, while a larger set remains bounded and scrollable.
fn results_area_height(panel: &PanelState) -> f32 {
    match results_area(panel) {
        ResultsArea::Hidden => 0.0,
        ResultsArea::Visible => {
            let rows = panel.routes.len().clamp(1, 6) as f32;
            (RESULT_VERTICAL_PADDING + (rows * RESULT_ROW_HEIGHT)).min(MAX_RESULTS_HEIGHT)
        }
    }
}

/// Calculates the search panel's content-driven logical window size.
pub fn panel_window_size(panel: &PanelState) -> iced::Size {
    let results_height = results_area_height(panel);
    let content_gap = if results_height > 0.0 {
        CONTENT_GAP
    } else {
        0.0
    };

    iced::Size::new(
        PANEL_WIDTH,
        (INPUT_ONLY_HEIGHT + content_gap + results_height).min(MAX_PANEL_HEIGHT),
    )
}

/// App-owned, one-shot receiver source for the host-event subscription.
#[derive(Clone)]
struct HostEventReceiver {
    receiver: Arc<Mutex<Option<mpsc::UnboundedReceiver<HostToUi>>>>,
}

// Iced uses subscription data only as an identity key. Each AppState owns one
// receiver, so a stable key deliberately avoids making Tokio's receiver Hash.
impl Hash for HostEventReceiver {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "flick-host-event-receiver".hash(state);
    }
}

/// Application state owned by the single Iced bridge.
pub struct AppState {
    pub panel: PanelState,
    host_tx: mpsc::UnboundedSender<UiToHost>,
    /// One Iced subscription takes this receiver and forwards its ordered events.
    host_rx: HostEventReceiver,
    window_id: Option<iced::window::Id>,
}

/// Messages accepted by the Iced update loop.
#[derive(Debug, Clone)]
pub enum AppMessage {
    InputChanged(String),
    Submitted,
    EscapePressed,
    SelectionUp,
    SelectionDown,
    HostEvent(HostToUi),
    Shutdown,
    WindowOpened(iced::window::Id),
    CloseRequested(iced::window::Id),
    DebounceFlush,
}

/// Converts a host event to the Iced bridge message that handles it.
///
/// Shutdown is a lifecycle control message, not panel data: mapping it here
/// ensures a hidden daemon also exits its Iced event loop.
pub fn map_host_event(host_event: HostToUi) -> AppMessage {
    match host_event {
        HostToUi::Shutdown => AppMessage::Shutdown,
        other => AppMessage::HostEvent(other),
    }
}

/// Starts the daemon Iced program without opening a window.
pub fn boot(
    host_tx: mpsc::UnboundedSender<UiToHost>,
    host_rx: mpsc::UnboundedReceiver<HostToUi>,
) -> (AppState, Task<AppMessage>) {
    (
        AppState {
            panel: PanelState::new(),
            host_tx,
            host_rx: HostEventReceiver {
                receiver: Arc::new(Mutex::new(Some(host_rx))),
            },
            window_id: None,
        },
        Task::none(),
    )
}

/// Applies one Iced message to the panel and produces any required tasks.
pub fn update(state: &mut AppState, message: AppMessage) -> Task<AppMessage> {
    match message {
        AppMessage::HostEvent(host_event) => {
            let event = match host_event {
                HostToUi::ShowSearchPanel { activation_token } => {
                    PanelEvent::Show { activation_token }
                }
                HostToUi::HideSearchPanel => PanelEvent::Hide,
                HostToUi::Routes { generation, items } => PanelEvent::Routes { generation, items },
                HostToUi::InvocationCompleted {
                    invocation_id,
                    outcome,
                } => PanelEvent::InvocationCompleted {
                    invocation_id,
                    outcome,
                },
                HostToUi::RuntimeStatus(status) => PanelEvent::RuntimeStatus(status),
                HostToUi::Shutdown => return update(state, AppMessage::Shutdown),
            };
            update_panel(state, event)
        }
        AppMessage::Shutdown => {
            let close_panel = if state.panel.visible {
                update_panel(state, PanelEvent::Hide)
            } else {
                close_hidden_window(state)
            };
            Task::batch([close_panel, iced::exit()])
        }
        AppMessage::InputChanged(text) => update_panel(state, PanelEvent::InputChanged(text)),
        AppMessage::Submitted => update_panel(state, PanelEvent::Submitted),
        AppMessage::EscapePressed => update_panel(state, PanelEvent::EscapePressed),
        AppMessage::SelectionUp => {
            state.panel.update(PanelEvent::SelectionUp);
            Task::none()
        }
        AppMessage::SelectionDown => {
            state.panel.update(PanelEvent::SelectionDown);
            Task::none()
        }
        AppMessage::WindowOpened(id) => {
            if state.panel.visible && state.window_id == Some(id) {
                Task::none()
            } else {
                // A hide/shutdown can race the asynchronous open completion.
                // Do not resurrect a stale window id in that case.
                iced::window::close(id)
            }
        }
        AppMessage::CloseRequested(id) => {
            if state.window_id != Some(id) {
                return Task::none();
            }
            update_panel(state, PanelEvent::WindowClosed)
        }
        AppMessage::DebounceFlush => update_panel(state, PanelEvent::DebounceFlush),
    }
}

fn update_panel(state: &mut AppState, event: PanelEvent) -> Task<AppMessage> {
    let size_before = panel_window_size(&state.panel);
    let actions = state.panel.update(event);
    apply_actions(state, actions, size_before)
}

fn apply_actions(
    state: &mut AppState,
    actions: Vec<PanelAction>,
    size_before: iced::Size,
) -> Task<AppMessage> {
    let mut tasks = Vec::new();

    for action in actions {
        match action {
            PanelAction::SendToHost(message) => {
                let _ = state.host_tx.send(message);
            }
            PanelAction::StartDebounce => {
                tasks.push(Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    },
                    |_| AppMessage::DebounceFlush,
                ));
            }
            PanelAction::CancelDebounce => {}
        }
    }

    if state.panel.visible && state.window_id.is_none() {
        let (id, task) = iced::window::open(panel_window_settings(&state.panel));
        state.window_id = Some(id);
        tasks.push(task.map(AppMessage::WindowOpened));
    }

    if state.panel.visible
        && let Some(id) = state.window_id
        && panel_window_size(&state.panel) != size_before
    {
        tasks.push(iced::window::resize(id, panel_window_size(&state.panel)));
    }

    if !state.panel.visible
        && let Some(id) = state.window_id.take()
    {
        tasks.push(iced::window::close(id));
    }

    Task::batch(tasks)
}

fn close_hidden_window(state: &mut AppState) -> Task<AppMessage> {
    match state.window_id.take() {
        Some(id) => iced::window::close(id),
        None => Task::none(),
    }
}

fn panel_window_settings(panel: &PanelState) -> iced::window::Settings {
    iced::window::Settings {
        size: panel_window_size(panel),
        min_size: Some(iced::Size::new(PANEL_WIDTH, INPUT_ONLY_HEIGHT)),
        max_size: Some(iced::Size::new(PANEL_WIDTH, MAX_PANEL_HEIGHT)),
        resizable: false,
        decorations: false,
        // The close request is mapped to SearchPanelClosed; the daemon itself
        // remains alive and opens a new panel window on the next activation.
        exit_on_close_request: false,
        ..Default::default()
    }
}

/// Renders one visible panel window.
pub fn view(state: &AppState, _window: iced::window::Id) -> Element<'_, AppMessage> {
    let search_input = text_input("Search actions...", &state.panel.query)
        .on_input(AppMessage::InputChanged)
        .on_submit(AppMessage::Submitted)
        .padding(12)
        .size(16);

    let mut content = column![search_input].spacing(CONTENT_GAP).padding(16);
    if results_area(&state.panel) == ResultsArea::Visible {
        content = content.push(results_view(&state.panel));
    }

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn results_view(panel: &PanelState) -> Element<'_, AppMessage> {
    let mut content = column![].spacing(4);

    for (index, route) in panel.routes.iter().enumerate() {
        let prefix = if index == panel.selection.index {
            "▶ "
        } else {
            "  "
        };
        content = content.push(
            container(text(format!("{prefix}{}", route.title)).size(14))
                .padding(4)
                .width(Length::Fill),
        );
    }

    if let Some(outcome) = &panel.last_outcome {
        let outcome_text = match outcome {
            ActionOutcome::Data(value) => format!("Result: {value}"),
            ActionOutcome::Notice { message } => format!("Notice: {message}"),
            ActionOutcome::Error(error) => format!("Error: {error}"),
        };
        content = content.push(
            container(text(outcome_text).size(14))
                .padding(8)
                .width(Length::Fill),
        );
    }

    scrollable(content)
        .height(Length::Fixed(results_area_height(panel)))
        .into()
}

fn keyboard_message(event: iced::keyboard::Event) -> Option<AppMessage> {
    use iced::keyboard::{Event, Key, key::Named};

    match event {
        Event::KeyPressed {
            key: Key::Named(Named::Escape),
            ..
        } => Some(AppMessage::EscapePressed),
        Event::KeyPressed {
            key: Key::Named(Named::ArrowUp),
            ..
        } => Some(AppMessage::SelectionUp),
        Event::KeyPressed {
            key: Key::Named(Named::ArrowDown),
            ..
        } => Some(AppMessage::SelectionDown),
        _ => None,
    }
}

fn runtime_event_message(
    event: iced::Event,
    _status: iced::event::Status,
    window_id: iced::window::Id,
) -> Option<AppMessage> {
    match event {
        iced::Event::Keyboard(event) => keyboard_message(event),
        iced::Event::Window(iced::window::Event::CloseRequested) => {
            Some(AppMessage::CloseRequested(window_id))
        }
        _ => None,
    }
}

fn host_event_stream(
    host_events: &HostEventReceiver,
) -> Pin<Box<dyn iced::futures::Stream<Item = AppMessage> + Send>> {
    let receiver = Arc::clone(&host_events.receiver);
    Box::pin(iced::stream::channel(
        100,
        move |mut sender: iced::futures::channel::mpsc::Sender<AppMessage>| async move {
            use iced::futures::SinkExt;

            let receiver = receiver
                .lock()
                .expect("host event receiver mutex poisoned")
                .take();
            let Some(mut receiver) = receiver else {
                std::future::pending::<()>().await;
                return;
            };

            while let Some(event) = receiver.recv().await {
                if sender.send(map_host_event(event)).await.is_err() {
                    break;
                }
            }
        },
    ))
}

/// Subscribes to the daemon host channel plus actual keyboard/window events.
pub fn subscription(state: &AppState) -> Subscription<AppMessage> {
    Subscription::batch([
        Subscription::run_with(state.host_rx.clone(), |host_events| {
            host_event_stream(host_events)
        }),
        iced::event::listen_with(runtime_event_message),
    ])
}

/// Uses Flick's dark shell theme.
pub fn theme(_state: &AppState, _window: iced::window::Id) -> Theme {
    Theme::Dark
}

/// Runs the Iced daemon application on the calling thread.
pub fn run(
    host_tx: mpsc::UnboundedSender<UiToHost>,
    host_rx: mpsc::UnboundedReceiver<HostToUi>,
) -> iced::Result {
    let host_tx = Arc::new(Mutex::new(Some(host_tx)));
    let host_rx = Arc::new(Mutex::new(Some(host_rx)));

    iced::daemon::daemon(
        move || {
            let tx = host_tx
                .lock()
                .expect("host sender mutex poisoned")
                .take()
                .expect("Flick Iced boot called twice");
            let rx = host_rx
                .lock()
                .expect("host receiver mutex poisoned")
                .take()
                .expect("Flick Iced boot called twice");
            boot(tx, rx)
        },
        update,
        view,
    )
    .subscription(subscription)
    .theme(theme)
    .run()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_event(key: iced::keyboard::Key) -> iced::keyboard::Event {
        use iced::keyboard::{Event, Key, Location, Modifiers, key::Named};

        Event::KeyPressed {
            key,
            modified_key: Key::Named(Named::Enter),
            physical_key: iced::keyboard::key::Physical::Unidentified(
                iced::keyboard::key::NativeCode::Unidentified,
            ),
            location: Location::Standard,
            modifiers: Modifiers::default(),
            text: None,
            repeat: false,
        }
    }

    fn app_state() -> (AppState, mpsc::UnboundedReceiver<UiToHost>) {
        let (_host_tx, host_rx) = mpsc::unbounded_channel();
        let (ui_tx, ui_rx) = mpsc::unbounded_channel();
        let (state, _) = boot(ui_tx, host_rx);
        (state, ui_rx)
    }

    #[test]
    fn shutdown_host_event_maps_to_shutdown_message() {
        assert!(matches!(
            map_host_event(HostToUi::Shutdown),
            AppMessage::Shutdown
        ));
    }

    #[test]
    fn keyboard_mapping_only_handles_panel_navigation_keys() {
        use iced::keyboard::{Key, key::Named};

        assert!(matches!(
            keyboard_message(key_event(Key::Named(Named::Escape))),
            Some(AppMessage::EscapePressed)
        ));
        assert!(matches!(
            keyboard_message(key_event(Key::Named(Named::ArrowUp))),
            Some(AppMessage::SelectionUp)
        ));
        assert!(matches!(
            keyboard_message(key_event(Key::Named(Named::ArrowDown))),
            Some(AppMessage::SelectionDown)
        ));
        assert!(keyboard_message(key_event(Key::Named(Named::Enter))).is_none());
    }

    #[test]
    fn escape_hides_an_open_panel() {
        let (mut state, mut host_rx) = app_state();
        let _ = update(
            &mut state,
            AppMessage::HostEvent(HostToUi::ShowSearchPanel {
                activation_token: None,
            }),
        );
        assert!(state.panel.visible);

        let _ = update(&mut state, AppMessage::EscapePressed);

        assert!(!state.panel.visible);
        assert!(matches!(
            host_rx.try_recv(),
            Ok(UiToHost::SearchPanelClosed)
        ));
    }

    #[test]
    fn selection_messages_change_only_the_selection() {
        let (mut state, _host_rx) = app_state();
        state.panel.routes = vec![
            RouteItem {
                action_id: ActionId::new("first").unwrap(),
                title: "First".into(),
                score: 1.0,
            },
            RouteItem {
                action_id: ActionId::new("second").unwrap(),
                title: "Second".into(),
                score: 0.5,
            },
        ];
        state.panel.selection.set_total(state.panel.routes.len());
        let query_before = state.panel.query.clone();
        let routes_before = state.panel.routes.clone();

        let _ = update(&mut state, AppMessage::SelectionDown);

        assert_eq!(state.panel.selection.index, 1);
        assert_eq!(state.panel.query, query_before);
        assert_eq!(state.panel.routes, routes_before);
    }

    #[test]
    fn close_request_sends_search_panel_closed_and_clears_window_id() {
        let (mut state, mut host_rx) = app_state();
        let id = iced::window::Id::unique();
        state.panel.visible = true;
        state.window_id = Some(id);

        let _ = update(&mut state, AppMessage::CloseRequested(id));

        assert!(!state.panel.visible);
        assert_eq!(state.window_id, None);
        assert!(matches!(
            host_rx.try_recv(),
            Ok(UiToHost::SearchPanelClosed)
        ));
    }
}
