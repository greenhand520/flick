//! Host ↔ UI message bridge.
//!
//! `IcedHostBridge` receives `HostToUi` events from the coordinator
//! via an mpsc channel and converts them into `ShellMessage` variants
//! that the Iced update loop can process.

use crate::message::ShellMessage;
use flick_domain::HostToUi;
use tokio::sync::mpsc;

/// Bridge between the Host event channel and the Iced message loop.
///
/// The daemon creates this bridge during composition and passes the
/// receiver end to the Iced application. The bridge polls for events
/// and emits `ShellMessage::HostEvent(...)` variants.
pub struct IcedHostBridge {
    rx: mpsc::UnboundedReceiver<HostToUi>,
}

impl IcedHostBridge {
    /// Create a new bridge with the given receiver.
    pub fn new(rx: mpsc::UnboundedReceiver<HostToUi>) -> Self {
        Self { rx }
    }

    /// Try to receive the next host event without blocking.
    ///
    /// Returns `Some(ShellMessage::HostEvent(...))` if an event is
    /// available, `None` otherwise.
    pub fn try_recv(&mut self) -> Option<ShellMessage> {
        self.rx.try_recv().ok().map(ShellMessage::HostEvent)
    }

    /// Receive the next host event, blocking until one is available.
    pub async fn recv(&mut self) -> Option<ShellMessage> {
        self.rx.recv().await.map(ShellMessage::HostEvent)
    }
}

/// Create a connected pair of (sender, bridge).
pub fn create_bridge() -> (mpsc::UnboundedSender<HostToUi>, IcedHostBridge) {
    let (tx, rx) = mpsc::unbounded_channel();
    (tx, IcedHostBridge::new(rx))
}
