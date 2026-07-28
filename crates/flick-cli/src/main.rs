use clap::{Parser, Subcommand};
use flick_cli::*;
use flick_ipc::message::*;
use std::path::PathBuf;
use std::process;

#[derive(Parser)]
#[command(
    name = "flick",
    about = "Flick launcher control",
    long_about = "Control the Flick daemon. The 'toggle' command is intended as an IPC fallback for desktop environment custom shortcut binding."
)]
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
    /// Toggle the search panel visibility (IPC fallback for desktop shortcut binding)
    Toggle,
    /// Trigger a specific action by ID
    Trigger {
        /// Action ID to invoke
        action_id: String,
        /// Text input for the action
        #[arg(long)]
        text: String,
    },
    /// Show daemon status
    Status,
    /// Show diagnostic information about the session and Portal
    Doctor,
    /// Shortcut management
    Shortcuts {
        #[command(subcommand)]
        action: ShortcutsAction,
    },
    /// Gracefully shut down the daemon
    Shutdown,
    /// Show the search panel (bypass Portal shortcut)
    Show,
    /// Hide the search panel
    Hide,
}

#[derive(Subcommand)]
enum ShortcutsAction {
    /// Configure the search shortcut via Portal (opens Portal binding UI).
    /// Default preferred trigger: Super+Space.
    Configure {
        /// Preferred key trigger for show-search (e.g. "Super+Space", "Ctrl+Space")
        #[arg(long = "show-search", default_value = "Super+Space")]
        preferred_trigger: String,
    },
    /// Reconnect to the Portal session (recovery only, no binding UI)
    Reconnect,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let path = cli
        .socket
        .unwrap_or_else(default_socket_path);
    let json_output = cli.json;

    let command = match cli.command {
        Command::Toggle => IpcCommand::Toggle,
        Command::Trigger { action_id, text } => IpcCommand::Trigger { action_id, text },
        Command::Status => IpcCommand::Status,
        Command::Doctor => IpcCommand::Doctor,
        Command::Shortcuts { action } => match action {
            ShortcutsAction::Configure { preferred_trigger } => IpcCommand::ShortcutsConfigure { preferred_trigger },
            ShortcutsAction::Reconnect => IpcCommand::ShortcutsReconnect,
        },
        Command::Shutdown => IpcCommand::Shutdown,
        Command::Show => IpcCommand::Show,
        Command::Hide => IpcCommand::Hide,
    };

    let request = build_request(command);

    // First attempt
    let exit_code = match try_connect_and_send(&path, &request).await {
        ConnectResult::Response(response) => {
            let (output, is_stderr, code) = format_result(&response.result, json_output);
            if is_stderr {
                eprintln!("{output}");
            } else {
                println!("{output}");
            }
            code
        }
        ConnectResult::ConnectError(_e) => {
            2 // will be handled below for toggle retry
        }
        ConnectResult::TransportError(e) => {
            let (output, is_stderr) = format_transport_error(&e, json_output);
            if is_stderr {
                eprintln!("{output}");
            } else {
                println!("{output}");
            }
            1
        }
    };

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

            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            // Retry once
            let retry_code = match try_connect_and_send(&path, &request).await {
                ConnectResult::Response(response) => {
                    let (output, is_stderr, code) =
                        format_result(&response.result, json_output);
                    if is_stderr {
                        eprintln!("{output}");
                    } else {
                        println!("{output}");
                    }
                    code
                }
                ConnectResult::ConnectError(_e) => {
                    let (output, is_stderr) = format_connection_error(&_e, json_output);
                    if is_stderr {
                        eprintln!("{output}");
                    } else {
                        println!("{output}");
                    }
                    2
                }
                ConnectResult::TransportError(e) => {
                    let (output, is_stderr) = format_transport_error(&e, json_output);
                    if is_stderr {
                        eprintln!("{output}");
                    } else {
                        println!("{output}");
                    }
                    1
                }
            };
            process::exit(retry_code);
        }

        // Non-toggle: print the connect error
        let (output, is_stderr) =
            format_connection_error("daemon not reachable", json_output);
        if is_stderr {
            eprintln!("{output}");
        } else {
            println!("{output}");
        }
    }

    process::exit(exit_code);
}
