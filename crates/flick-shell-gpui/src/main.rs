//! 可运行 demo：内置假 host，不依赖 daemon / flick-runtime。

use flick_domain::{ActionId, ActionOutcome, HostToUi, RouteItem, UiToHost};
use tokio::sync::mpsc;

const CATALOG: &[(&str, &str)] = &[
    ("echo", "Echo"),
    ("open-browser", "Open in Browser"),
    ("copy", "Copy to Clipboard"),
    ("search-files", "Search Files"),
    ("settings", "Settings"),
];

async fn fake_host(
    host_tx: mpsc::UnboundedSender<HostToUi>,
    mut ui_rx: mpsc::UnboundedReceiver<UiToHost>,
) {
    // 模拟一次全局快捷键唤出面板。
    let _ = host_tx.send(HostToUi::ShowSearchPanel {
        activation_token: None,
    });

    while let Some(message) = ui_rx.recv().await {
        match message {
            UiToHost::QueryChanged { generation, text } => {
                let q = text.to_lowercase();
                let items: Vec<RouteItem> = CATALOG
                    .iter()
                    .filter(|(id, title)| {
                        q.is_empty() || id.contains(&q) || title.to_lowercase().contains(&q)
                    })
                    .map(|(id, title)| RouteItem {
                        action_id: ActionId::new(*id).unwrap(),
                        title: (*title).to_string(),
                        score: 1.0,
                    })
                    .collect();
                let _ = host_tx.send(HostToUi::Routes { generation, items });
            }
            UiToHost::Invoke {
                invocation_id, ..
            } => {
                // NOTE: flick-shell-gpui 无 serde_json 依赖，用 Notice 而非 Data。
                let _ = host_tx.send(HostToUi::InvocationCompleted {
                    invocation_id,
                    outcome: ActionOutcome::Notice {
                        message: "ok".to_string(),
                    },
                });
            }
            UiToHost::Cancel { .. } => {}
            UiToHost::SearchPanelClosed => {
                eprintln!("[demo] search panel closed");
            }
        }
    }
}

fn main() {
    let (ui_tx, ui_rx) = mpsc::unbounded_channel::<UiToHost>();
    let (host_tx, host_rx) = mpsc::unbounded_channel::<HostToUi>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build host runtime");
        rt.block_on(fake_host(host_tx, ui_rx));
    });

    let initial = flick_shell_gpui::detect_initial_theme();
    flick_shell_gpui::run_with_theme(ui_tx, host_rx, initial);
}
