//! 可运行 demo：内置真实的 `ActionRuntime`（echo + host 插件），不依赖 daemon。

use std::sync::Arc;

use flick_domain::{ActionRequest, Caller, Deadline, HostToUi, InputType, UiToHost};
use flick_runtime::ActionRuntime;
use tokio::sync::mpsc;

async fn fake_host(
    host_tx: mpsc::UnboundedSender<HostToUi>,
    mut ui_rx: mpsc::UnboundedReceiver<UiToHost>,
) {
    // 使用与 daemon 相同的运行时与内置插件，让 demo 的路由/调用行为贴近真实。
    let runtime = Arc::new(ActionRuntime::default());
    flick_builtins::register_phase0_builtins(&runtime).await;
    runtime.set_host_info(Arc::new(
        flick_adapters_linux::host_info::collect_host_info(),
    ));

    // 模拟一次全局快捷键唤出面板。
    let _ = host_tx.send(HostToUi::ShowSearchPanel {
        activation_token: None,
    });

    while let Some(message) = ui_rx.recv().await {
        match message {
            UiToHost::QueryChanged { generation, text } => {
                let items = runtime.route(&text, InputType::PlainText).await;
                let _ = host_tx.send(HostToUi::Routes { generation, items });
            }
            UiToHost::Invoke {
                invocation_id,
                action_id,
                input,
            } => {
                let request = ActionRequest {
                    invocation_id,
                    action_id,
                    input,
                    caller: Caller::ui("gpui-demo"),
                    deadline: Deadline::from_secs(30),
                };
                let outcome = runtime.invoke(request).await;
                let _ = host_tx.send(HostToUi::InvocationCompleted {
                    invocation_id,
                    outcome,
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
