//! Fake `HostEventSink` for testing.
//!
//! `FakeHostEventSink` collects all `HostToUi` events emitted by the
//! coordinator, making them available for assertions.

use std::sync::{Arc, Mutex};

use flick_application::HostEventSink;
use flick_domain::HostToUi;

/// A test double for `HostEventSink` that records every event.
///
/// # Usage
///
/// ```ignore
/// let sink = FakeHostEventSink::new();
/// let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));
/// // … drive the coordinator …
/// let events = sink.events();   // snapshot
/// let last  = sink.last();      // peek without removing
/// let all   = sink.drain();     // drain all events
/// ```
pub struct FakeHostEventSink {
    events: Arc<Mutex<Vec<HostToUi>>>,
}

impl FakeHostEventSink {
    /// Create a new, empty sink.
    pub fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Return a clone of all collected events (does **not** drain).
    pub fn events(&self) -> Vec<HostToUi> {
        self.events.lock().unwrap().clone()
    }

    /// Drain all collected events, leaving the internal buffer empty.
    pub fn drain(&self) -> Vec<HostToUi> {
        self.events.lock().unwrap().drain(..).collect()
    }

    /// Peek at the most recently emitted event (without removing it).
    pub fn last(&self) -> Option<HostToUi> {
        self.events.lock().unwrap().last().cloned()
    }
}

impl Default for FakeHostEventSink {
    fn default() -> Self {
        Self::new()
    }
}

impl HostEventSink for FakeHostEventSink {
    fn send(&self, event: HostToUi) {
        self.events.lock().unwrap().push(event);
    }
}
