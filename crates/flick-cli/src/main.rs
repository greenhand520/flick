use clap::{Parser, Subcommand};
use flick_ipc::message::*;
use flick_ipc::unix::UnixSocketClient;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process;

#[derive(Parser)]
#[command(name = "flick", about = "Flick launcher control")]
struct Cli {
    /// Path to daemon socket (default: $XDG_RUNTIME_DIR/flick/flick.sock)
    #[arg(long)]
    socket: Option<PathBuf>,

    /// Output JSON instead of human-readable text
    #[arg(long)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Toggle the search panel (IPC fallback for desktop shortcut binding)
    Toggle,
    /// Trigger a specific action
    Trigger {
        /// Action ID to invoke
        action_id: String,
        /// Text input for the action
        #[arg(long)]
        text: String,
    },
    /// Show daemon status
    Status,
    /// Show diagnostic information
    Doctor,
    /// Shortcut management
    Shortcuts {
        #[command(subcommand)]
        action: ShortcutsAction,
    },
    /// Shut down the daemon
    Shutdown,
}

#[derive(Subcommand)]
enum ShortcutsAction {
    /// Configure the search shortcut via Portal
    Configure,
    /// Reconnect to the Portal session
    Reconnect,
}

fn socket_path(custom: Option<PathBuf>) -> PathBuf {
    if let Some(p) = custom {
        return p;
    }
    let runtime_dir =
        std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("flick").join("flick.sock")
}

fn next_request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    format!("cli-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn make_request(command: IpcCommand) -> IpcRequestEnvelope {
    IpcRequestEnvelope {
        protocol_version: IPC_PROTOCOL_VERSION,
        request_id: next_request_id(),
        command,
    }
}

fn print_result(result: &IpcResult, json_output: bool) -> i32 {
    match result {
        IpcResult::Ok(value) => {
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string(&json!({ "ok": value })).unwrap()
                );
            } else {
                println!("{}", value);
            }
            0
        }
        IpcResult::Error { code, message } => {
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string(&json!({ "error": { "code": code.as_str(), "message": message } }))
                        .unwrap()
                );
            } else {
                eprintln!("Error: {message}");
            }
            match code {
                IpcErrorCode::DaemonUnavailable => 2,
                IpcErrorCode::ProtocolVersionMismatch => 3,
                _ => 1,
            }
        }
    }
}

async fn try_connect_and_send(
    path: &Path,
    request: IpcRequestEnvelope,
    json_output: bool,
) -> i32 {
    match UnixSocketClient::connect(path).await {
        Ok(mut client) => match client.request(&request).await {
            Ok(response) => print_result(&response.result, json_output),
            Err(e) => {
                if json_output {
                    println!(
                        "{}",
                        serde_json::to_string(&json!({
                            "error": { "code": "INTERNAL", "message": format!("{e}") }
                        }))
                        .unwrap()
                    );
                } else {
                    eprintln!("Error communicating with daemon: {e}");
                }
                1
            }
        },
        Err(e) => {
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string(&json!({
                        "error": { "code": "DAEMON_UNAVAILABLE", "message": format!("{e}") }
                    }))
                    .unwrap()
                );
            } else {
                eprintln!("Cannot connect to daemon: {e}");
            }
            2
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let path = socket_path(cli.socket);
    let json_output = cli.json;

    let command = match cli.command {
        Command::Toggle => IpcCommand::Toggle,
        Command::Trigger { action_id, text } => IpcCommand::Trigger { action_id, text },
        Command::Status => IpcCommand::Status,
        Command::Doctor => IpcCommand::Doctor,
        Command::Shortcuts { action } => match action {
            ShortcutsAction::Configure => IpcCommand::ShortcutsConfigure,
            ShortcutsAction::Reconnect => IpcCommand::ShortcutsReconnect,
        },
        Command::Shutdown => IpcCommand::Shutdown,
    };

    let request = make_request(command);

    // First attempt
    let exit_code = try_connect_and_send(&path, request.clone(), json_output).await;

    // If daemon unavailable for Toggle, try starting the service once
    if exit_code == 2 {
        if matches!(request.command, IpcCommand::Toggle) {
            if !json_output {
                eprintln!("Daemon not running, attempting to start flick.service...");
            }
            let _ = tokio::process::Command::new("systemctl")
                .args(["--user", "start", "flick.service"])
                .output()
                .await;

            // Give the service a moment to start
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            // Retry once
            let retry_code = try_connect_and_send(&path, request, json_output).await;
            process::exit(retry_code);
        }
    }

    process::exit(exit_code);
}
