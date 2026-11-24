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
    /// 守护进程套接字路径（默认：$XDG_RUNTIME_DIR/flick/flick.sock）
    #[arg(long)]
    socket: Option<PathBuf>,

    /// 输出 JSON 而非人类可读文本
    #[arg(long)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 切换搜索面板的可见性（桌面快捷键绑定的 IPC 回退方案）
    Toggle,
    /// 按标识符触发特定动作
    Trigger {
        /// 要调用的动作标识符
        action_id: String,
        /// 动作的文本输入
        #[arg(long)]
        text: String,
    },
    /// 显示守护进程状态
    Status,
    /// 显示有关会话和 Portal 的诊断信息
    Doctor,
    /// 快捷键管理
    Shortcuts {
        #[command(subcommand)]
        action: ShortcutsAction,
    },
    /// 正常关闭守护进程
    Shutdown,
    /// 显示搜索面板（绕过 Portal 快捷键）
    Show,
    /// 隐藏搜索面板
    Hide,
}

#[derive(Subcommand)]
enum ShortcutsAction {
    /// 通过 Portal 配置搜索快捷键（打开 Portal 绑定界面）。
    /// 默认首选触发器：Super+Space。
    Configure {
        /// show-search 的首选按键触发器（例如 "Super+Space"、"Ctrl+Space"）
        #[arg(long = "show-search", default_value = "Super+Space")]
        preferred_trigger: String,
    },
    /// 重新连接到 Portal 会话（仅用于恢复，无绑定界面）
    Reconnect,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let path = match cli.socket {
        Some(path) => path,
        None => match default_socket_path() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("Error: {error}");
                process::exit(2);
            }
        },
    };
    let json_output = cli.json;

    let command = match cli.command {
        Command::Toggle => IpcCommand::Toggle,
        Command::Trigger { action_id, text } => IpcCommand::Trigger { action_id, text },
        Command::Status => IpcCommand::Status,
        Command::Doctor => IpcCommand::Doctor,
        Command::Shortcuts { action } => match action {
            ShortcutsAction::Configure { preferred_trigger } => {
                IpcCommand::ShortcutsConfigure { preferred_trigger }
            }
            ShortcutsAction::Reconnect => IpcCommand::ShortcutsReconnect,
        },
        Command::Shutdown => IpcCommand::Shutdown,
        Command::Show => IpcCommand::Show,
        Command::Hide => IpcCommand::Hide,
    };

    let request = build_request(command);

    // 首次尝试
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
            2 // 将在下方处理以进行 toggle 重试
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

    // 如果 Toggle 时守护进程不可用，尝试启动一次服务
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

            // 重试一次
            let retry_code = match try_connect_and_send(&path, &request).await {
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

        // 非 Toggle：打印连接错误
        let (output, is_stderr) = format_connection_error("daemon not reachable", json_output);
        if is_stderr {
            eprintln!("{output}");
        } else {
            println!("{output}");
        }
    }

    process::exit(exit_code);
}
