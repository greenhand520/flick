//! Typed boundary between Portal lifecycle policy and a platform client.

use crate::session::{ShortcutBinding, ShortcutError};

/// Normalized events emitted by a Portal client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortalEvent {
    /// A shortcut registered by the current Portal session was activated.
    Activated {
        shortcut_id: String,
        activation_token: Option<String>,
    },
    /// The current Portal session was closed by the desktop or Portal.
    SessionClosed,
    /// The Portal transport stopped delivering events.
    TransportError { message: String },
}

/// Platform-facing Portal client.
///
/// The client owns platform-specific session objects. The adapter only sees
/// normalized bindings, errors, and events, keeping Portal implementation
/// details out of the shortcut policy and diagnostics model.
#[async_trait::async_trait]
pub trait PortalClient: Send + Sync {
    /// Creates a session and lists its existing shortcut bindings.
    async fn open_session_and_list(&self) -> Result<Vec<ShortcutBinding>, ShortcutError>;

    /// Makes the one explicit bind attempt for the show-search shortcut.
    async fn bind_show_search(
        &self,
        preferred_trigger: &str,
    ) -> Result<Vec<ShortcutBinding>, ShortcutError>;

    /// Closes the currently active platform session, if any.
    async fn close_session(&self);

    /// Returns the one receiver supplying normalized Portal events.
    ///
    /// Returning `None` signals an internal client lifecycle/programming error.
    fn take_event_receiver(&self) -> Option<tokio::sync::mpsc::UnboundedReceiver<PortalEvent>>;
}
