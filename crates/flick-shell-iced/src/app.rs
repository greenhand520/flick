//! Iced daemon application for the Flick search panel.
//!
//! Uses `iced::daemon::daemon()` — no window on startup.
//! Opens window on ShowSearchPanel, closes on Escape/WindowClosed.

use crate::panel::{PanelAction, PanelEvent, PanelState};
use flick_domain::*;
use iced::widget::{column, container, scrollable, text, text_input};
use iced::{Element, Length, Subscription, Task, Theme};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc;

// ─── Global host event channel (one-time setup) ─────────────────────────────

static HOST_RX: OnceLock<Arc<Mutex<Option<mpsc::UnboundedReceiver<HostToUi>>>>> = OnceLock::new();

fn take_host_rx() -> Option<mpsc::UnboundedReceiver<HostToUi>> {
    HOST_RX
        .get()
        .and_then(|arc| arc.lock().ok())
        .and_then(|mut guard| guard.take())
}

// ─── Application state ──────────────────────────────────────────────────────

pub struct AppState {
    pub panel: PanelState,
    host_tx: mpsc::UnboundedSender<UiToHost>,
    window_id: Option<iced::window::Id>,
}

// ─── Messages ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum AppMessage {
    InputChanged(String),
    Submitted,
    EscapePressed,
    SelectionUp,
    SelectionDown,
    HostEvent(HostToUi),
    WindowOpened(iced::window::Id),
    CloseRequested(iced::window::Id),
    DebounceFlush,
}

// ─── Boot ───────────────────────────────────────────────────────────────────

pub fn boot(
    host_tx: mpsc::UnboundedSender<UiToHost>,
    host_rx: mpsc::UnboundedReceiver<HostToUi>,
) -> (AppState, Task<AppMessage>) {
    // Store receiver globally for the subscription to consume
    let _ = HOST_RX.set(Arc::new(Mutex::new(Some(host_rx))));

    let state = AppState {
        panel: PanelState::new(),
        host_tx,
        window_id: None,
    };
    (state, Task::none())
}

// ─── Update ─────────────────────────────────────────────────────────────────

pub fn update(state: &mut AppState, message: AppMessage) -> Task<AppMessage> {
    match message {
        AppMessage::HostEvent(HostToUi::ShowSearchPanel { activation_token }) => {
            let actions = state.panel.update(PanelEvent::Show { activation_token });
            apply_actions(state, actions)
        }
        AppMessage::HostEvent(HostToUi::Shutdown) => iced::exit(),
        AppMessage::HostEvent(HostToUi::HideSearchPanel) => {
            let actions = state.panel.update(PanelEvent::Hide);
            apply_actions(state, actions)
        }
        AppMessage::HostEvent(host_event) => {
            let event = match host_event {
                HostToUi::Routes { generation, items } => PanelEvent::Routes { generation, items },
                HostToUi::InvocationCompleted {
                    invocation_id,
                    outcome,
                } => PanelEvent::InvocationCompleted {
                    invocation_id,
                    outcome,
                },
                HostToUi::RuntimeStatus(status) => PanelEvent::RuntimeStatus(status),
                _ => return Task::none(),
            };
            let actions = state.panel.update(event);
            apply_actions(state, actions)
        }
        AppMessage::InputChanged(t) => {
            let actions = state.panel.update(PanelEvent::InputChanged(t));
            apply_actions(state, actions)
        }
        AppMessage::Submitted => {
            let actions = state.panel.update(PanelEvent::Submitted);
            apply_actions(state, actions)
        }
        AppMessage::EscapePressed => {
            let actions = state.panel.update(PanelEvent::EscapePressed);
            apply_actions(state, actions)
        }
        AppMessage::SelectionUp => {
            state.panel.update(PanelEvent::SelectionUp);
            Task::none()
        }
        AppMessage::SelectionDown => {
            state.panel.update(PanelEvent::SelectionDown);
            Task::none()
        }
        AppMessage::WindowOpened(id) => {
            state.window_id = Some(id);
            Task::none()
        }
        AppMessage::CloseRequested(_id) => {
            let actions = state.panel.update(PanelEvent::WindowClosed);
            apply_actions(state, actions)
        }
        AppMessage::DebounceFlush => {
            let actions = state.panel.update(PanelEvent::DebounceFlush);
            apply_actions(state, actions)
        }
    }
}

fn apply_actions(state: &mut AppState, actions: Vec<PanelAction>) -> Task<AppMessage> {
    let mut tasks = Vec::new();

    for action in actions {
        match action {
            PanelAction::SendToHost(msg) => {
                let _ = state.host_tx.send(msg);
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

    // Open window if panel visible and no window yet
    if state.panel.visible && state.window_id.is_none() {
        let (id, task) = iced::window::open(iced::window::Settings {
            size: iced::Size::new(480.0, 400.0),
            resizable: false,
            decorations: true,
            ..Default::default()
        });
        state.window_id = Some(id);
        tasks.push(task.map(AppMessage::WindowOpened));
    }

    // Close window if panel hidden and window exists
    if !state.panel.visible
        && let Some(id) = state.window_id.take()
    {
        tasks.push(iced::window::close(id));
    }

    Task::batch(tasks)
}

// ─── View ───────────────────────────────────────────────────────────────────

pub fn view(state: &AppState, _window: iced::window::Id) -> Element<'_, AppMessage> {
    if !state.panel.visible {
        return container(text(""))
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
    }

    let search_input = text_input("Search actions...", &state.panel.query)
        .on_input(AppMessage::InputChanged)
        .on_submit(AppMessage::Submitted)
        .padding(12)
        .size(16);

    let mut routes_col = column![].spacing(4);
    for (i, route) in state.panel.routes.iter().enumerate() {
        let prefix = if i == state.panel.selection.index {
            "▶ "
        } else {
            "  "
        };
        routes_col = routes_col.push(
            container(text(format!("{prefix}{}", route.title)).size(14))
                .padding(4)
                .width(Length::Fill),
        );
    }
    let routes_scroll = scrollable(routes_col).height(Length::Fixed(200.0));

    let mut content = column![search_input, routes_scroll].spacing(8).padding(16);

    if let Some(outcome) = &state.panel.last_outcome {
        let outcome_text = match outcome {
            ActionOutcome::Data(val) => format!("Result: {val}"),
            ActionOutcome::Notice { message } => format!("Notice: {message}"),
            ActionOutcome::Error(err) => format!("Error: {err}"),
        };
        content = content.push(
            container(text(outcome_text).size(14))
                .padding(8)
                .width(Length::Fill),
        );
    }

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .into()
}

// ─── Subscription ───────────────────────────────────────────────────────────

/// Subscription that bridges the host event channel into Iced messages.
/// Uses `run_with` where data=() (always same hash) and the stream
/// consumes the global receiver on first invocation.
fn host_event_stream() -> impl iced::futures::Stream<Item = AppMessage> {
    iced::stream::channel(
        100,
        move |mut sender: iced::futures::channel::mpsc::Sender<AppMessage>| async move {
            use iced::futures::SinkExt;
            if let Some(mut rx) = take_host_rx() {
                while let Some(event) = rx.recv().await {
                    let _ = sender.send(AppMessage::HostEvent(event)).await;
                }
            } else {
                // No receiver available — never produce items
                std::future::pending::<()>().await;
            }
        },
    )
}

pub fn subscription(_state: &AppState) -> Subscription<AppMessage> {
    Subscription::run(host_event_stream)
}

// ─── Theme ───────────────────────────────────────────────────────────────────

pub fn theme(_state: &AppState, _window: iced::window::Id) -> Theme {
    Theme::Dark
}

// ─── Entry point ─────────────────────────────────────────────────────────────

pub fn run(
    host_tx: mpsc::UnboundedSender<UiToHost>,
    host_rx: mpsc::UnboundedReceiver<HostToUi>,
) -> iced::Result {
    // Wrap in Arc<Mutex<Option>> so the closure can be Fn (not FnOnce)
    let host_tx = Arc::new(Mutex::new(Some(host_tx)));
    let host_rx = Arc::new(Mutex::new(Some(host_rx)));

    iced::daemon::daemon(
        move || {
            let tx = host_tx.lock().unwrap().take().expect("boot called twice");
            let rx = host_rx.lock().unwrap().take().expect("boot called twice");
            boot(tx, rx)
        },
        update,
        view,
    )
    .subscription(subscription)
    .theme(theme)
    .run()
}
