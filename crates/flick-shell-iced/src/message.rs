//! Shell-level messages: the union of user actions, host events, and internal
//! timer ticks that the Iced update loop processes.

use flick_domain::*;

/// Messages processed by the Iced shell's update function.
#[derive(Debug, Clone)]
pub enum ShellMessage {
    // ── User input ───────────────────────────────────────────────────────
    /// Text input changed in the search box.
    InputChanged(String),
    /// User pressed Enter to invoke the selected route.
    Submitted,
    /// User pressed Escape to hide the panel.
    EscapePressed,
    /// User pressed ArrowUp to move selection up.
    SelectionUp,
    /// User pressed ArrowDown to move selection down.
    SelectionDown,
    /// The window was closed (close button, WM delete, etc.).
    WindowClosed,

    // ── Host events ──────────────────────────────────────────────────────
    /// A host event arrived via the bridge.
    HostEvent(HostToUi),

    // ── Internal ─────────────────────────────────────────────────────────
    /// Debounce timer fired — flush the pending query.
    DebounceFlush,
}
