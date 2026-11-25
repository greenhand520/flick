//! HostPlugin：报告宿主基础信息，输出 OpenSurface("info")。

use std::sync::Arc;

use async_trait::async_trait;
use flick_domain::*;
use flick_runtime::{ActionHandler, ActionSpec, Plugin, PluginLifecycle, PluginRuntimeError};

pub const HOST_ACTION_ID: &str = "flick.builtin.host.show_info";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HostInfo {
    pub os: String,
    pub kernel: String,
    pub locale: String,
    pub shell: String, // "zsh" | "bash" | "other"
    pub lan_ip: Option<String>,
}

pub struct HostPlugin;

impl HostPlugin {
    fn descriptor() -> ActionDescriptor {
        ActionDescriptor::new(
            ActionId::from_static(HOST_ACTION_ID),
            "系统信息",
            vec!["host".into(), "系统".into(), "info".into()],
            Matcher::prefix_command("host"),
            ConcurrencyPolicy::Parallel,
        )
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
        _ctx: ActionContext,
    ) -> Result<ActionOutcome, ActionError> {
        if request.input.kind != InputKind::PlainText {
            return Err(ActionError::InvalidInput {
                message: "host 仅支持纯文本输入".into(),
            });
        }
        let info = collect_host_info();
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

fn read_trim(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub fn collect_host_info() -> HostInfo {
    let os = std::env::consts::OS.to_string();
    let kernel = read_trim("/proc/sys/kernel/osrelease");
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_else(|_| "en_US.UTF-8".into());
    let shell = match std::env::var("SHELL").unwrap_or_default() {
        s if s.ends_with("/zsh") => "zsh".to_string(),
        s if s.ends_with("/bash") => "bash".to_string(),
        _ => "other".to_string(),
    };
    let lan_ip = std::process::Command::new("hostname")
        .arg("-I")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split_whitespace().next().map(|s| s.to_string()));
    HostInfo {
        os,
        kernel,
        locale,
        shell,
        lan_ip,
    }
}

fn collect_extras() -> serde_json::Value {
    let uptime = read_trim("/proc/uptime");
    let cpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    let mem_total_kb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .map(|l| l.to_string())
        })
        .unwrap_or_default();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    serde_json::json!({
        "uptime_raw": uptime,
        "cpu_threads": cpu,
        "mem_total_line": mem_total_kb,
        "desktop": desktop,
    })
}
