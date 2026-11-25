//! HostPlugin：报告宿主基础信息，输出 OpenSurface("info")。

use std::sync::Arc;

use async_trait::async_trait;
use flick_domain::*;
use flick_runtime::{ActionHandler, ActionSpec, Plugin, PluginLifecycle, PluginRuntimeError};

pub const HOST_ACTION_ID: &str = "flick.builtin.host.show_info";

pub struct HostPlugin;

impl HostPlugin {
    fn descriptor() -> ActionDescriptor {
        let mut d = ActionDescriptor::new(
            ActionId::from_static(HOST_ACTION_ID),
            "系统信息",
            vec!["host".into(), "系统".into(), "info".into()],
            Matcher::prefix_command("host"),
            ConcurrencyPolicy::Parallel,
        );
        d.output_types = vec![OutputType::Surface];
        d
    }
}

impl Plugin for HostPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "flick.builtin.host".into(),
            name: "Host Info".into(),
            version: "1.0.0".into(),
            kind: PluginKind::BuiltInRust,
            keywords: vec![
                "host".into(),
                "系统信息".into(),
                "sysinfo".into(),
                "fastfetch".into(),
            ],
            profile: PluginProfile {
                description: "显示系统基础信息".into(),
                categories: vec![Category::System],
                tags: vec!["fastfetch".into(), "硬件".into(), "sysinfo".into()],
                languages: vec![],
                execution: ExecutionMode::Local,
                data_handling: None,
            },
            capabilities: vec![],
            plugin_dependencies: vec![],
            settings: vec![],
            surfaces: vec![UiSurfaceDecl {
                id: "info".into(),
                title: "系统信息".into(),
                kind: SurfaceKind::Panel,
                default_placement: SurfacePlacement::BelowSearchBox,
            }],
            // 依赖 /proc、hostname 等 Linux 工具，仅声明支持 Linux。
            supported_os: vec![OperatingSystem::Linux],
            supported_architectures: vec![],
        }
    }
    fn actions(&self) -> Vec<ActionSpec> {
        vec![ActionSpec {
            descriptor: Self::descriptor(),
            handler: Arc::new(HostHandler),
        }]
    }
    fn lifecycle(&self) -> Option<&dyn PluginLifecycle> {
        Some(&HOST_LIFECYCLE)
    }
}

struct HostHandler;

#[async_trait]
impl ActionHandler for HostHandler {
    async fn invoke(
        &self,
        request: ActionRequest,
        ctx: ActionContext,
    ) -> Result<ActionOutcome, ActionError> {
        if request.input.kind != InputType::PlainText {
            return Err(ActionError::InvalidInput {
                message: "host 仅支持纯文本输入".into(),
            });
        }
        let host = match ctx.host_info {
            Some(host) => host,
            None => {
                return Err(ActionError::Unavailable {
                    reason: "host info 未在启动时注入".into(),
                });
            }
        };
        let info = (*host).clone();
        let extras = collect_extras();
        Ok(ActionOutcome::OpenSurface {
            surface_id: "info".into(),
            args: serde_json::json!({ "info": info, "extras": extras }),
        })
    }
}

static HOST_LIFECYCLE: HostLifecycle = HostLifecycle;

struct HostLifecycle;
#[async_trait]
impl PluginLifecycle for HostLifecycle {
    async fn activate(&self) -> Result<(), PluginRuntimeError> {
        Ok(())
    }
    async fn deactivate(&self) -> Result<(), PluginRuntimeError> {
        Ok(())
    }
    fn idle_timeout(&self) -> Option<std::time::Duration> {
        Some(std::time::Duration::from_secs(60))
    }
}

/// 尽力采集 fastfetch 风格的“其他信息”。
///
/// 仅做廉价、非阻塞的本地读取；locale 已由 [`flick_domain::HostInfo`] 提供。
///
/// TODO: 以下字段尚未实现 —— GPU / Displays / Disk(s) / Packages count /
/// Battery / Icons / Font / Cursor（涉及外部进程或较慢探测，暂不实现）。
fn collect_extras() -> serde_json::Value {
    // uptime: /proc/uptime 第一列是开机秒数。
    let uptime = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_string))
        .and_then(|s| s.parse::<f64>().ok())
        .map(|secs| {
            let total = secs as u64;
            format!("{}h {}m", total / 3600, (total % 3600) / 60)
        })
        .unwrap_or_default();

    // cpu: /proc/cpuinfo 中第一个 "model name"，线程数用 available_parallelism。
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
        })
        .unwrap_or_default();
    let cpu_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);

    // memory / swap: /proc/meminfo（kB → GiB）。
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mem_kb = |key: &str| -> Option<f64> {
        meminfo
            .lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<f64>().ok())
    };
    let gib = |kb: f64| (kb / 1024.0 / 1024.0 * 100.0).round() / 100.0;
    let memory_total_gib = mem_kb("MemTotal:").map(gib).unwrap_or(0.0);
    let memory_available_gib = mem_kb("MemAvailable:").map(gib).unwrap_or(0.0);
    let swap_total_gib = mem_kb("SwapTotal:").map(gib).unwrap_or(0.0);
    let swap_free_gib = mem_kb("SwapFree:").map(gib).unwrap_or(0.0);

    let desktop_environment = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let theme = std::env::var("GTK_THEME").unwrap_or_default();
    let terminal = std::env::var("TERM_PROGRAM")
        .or_else(|_| std::env::var("TERM"))
        .unwrap_or_default();

    serde_json::json!({
        "uptime": uptime,
        "cpu": cpu,
        "cpu_threads": cpu_threads,
        "memory_total_gib": memory_total_gib,
        "memory_available_gib": memory_available_gib,
        "swap_total_gib": swap_total_gib,
        "swap_free_gib": swap_free_gib,
        "desktop_environment": desktop_environment,
        "session_type": session_type,
        "theme": theme,
        "terminal": terminal,
    })
}
