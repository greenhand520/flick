//! Portal capability probe.
//!
//! Run: `cargo run -p flick-adapters-linux --example portal_probe`
//!
//! Opens and lists a GlobalShortcuts session for diagnostics only. It does not
//! call BindShortcuts and it never forwards UI intents.

use flick_adapters_linux::{
    portal::{IntentSink, PortalShortcutAdapter},
    portal_client::AshpdPortalClient,
    session::ShortcutError,
};
use flick_domain::UiIntent;
use std::sync::Arc;

struct ProbeIntentSink;

impl IntentSink for ProbeIntentSink {
    fn send_intent(&self, _intent: UiIntent) {}
}

#[tokio::main]
async fn main() {
    let client = Arc::new(AshpdPortalClient::new());
    let intent_sink: Arc<dyn IntentSink> = Arc::new(ProbeIntentSink);
    let adapter = PortalShortcutAdapter::new_with_client(intent_sink, client);

    let snapshot = match adapter.initialize().await {
        Ok(snapshot) => snapshot,
        Err(ShortcutError::Unavailable { reason }) => {
            eprintln!("Portal GlobalShortcuts is unavailable: {reason}");
            adapter.diagnostics().await
        }
        Err(error) => {
            eprintln!("Portal GlobalShortcuts diagnostic failed: {error}");
            adapter.diagnostics().await
        }
    };

    match serde_json::to_string_pretty(&snapshot) {
        Ok(serialized) => println!("{serialized}"),
        Err(error) => eprintln!("Could not serialize Portal diagnostics: {error}"),
    }

    adapter.shutdown().await;
}
