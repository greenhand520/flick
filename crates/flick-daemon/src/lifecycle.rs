use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};
use tokio_util::sync::CancellationToken;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownReason {
    Signal,
    Ipc,
    IcedExited,
    IcedFailed,
    StartupFailed,
}
#[derive(Clone)]
pub struct DaemonLifecycle {
    token: CancellationToken,
    reason: Arc<OnceLock<ShutdownReason>>,
}
impl DaemonLifecycle {
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            reason: Arc::new(OnceLock::new()),
        }
    }
    pub fn request_shutdown(&self, reason: ShutdownReason) {
        let _ = self.reason.set(reason);
        self.token.cancel()
    }
    pub async fn cancelled(&self) {
        self.token.cancelled().await
    }
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }
    #[allow(dead_code)]
    pub fn reason(&self) -> Option<ShutdownReason> {
        self.reason.get().copied()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketMode {
    Graphical,
    Headless,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SocketPathError {
    #[error("XDG_RUNTIME_DIR is required; pass --socket for an explicit path")]
    MissingRuntimeDir,
}
pub fn resolve_socket_path<F>(
    custom: Option<PathBuf>,
    _: SocketMode,
    get: F,
) -> Result<PathBuf, SocketPathError>
where
    F: FnOnce(&str) -> Option<String>,
{
    if let Some(path) = custom {
        return Ok(path);
    }
    let runtime = get("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .ok_or(SocketPathError::MissingRuntimeDir)?;
    Ok(PathBuf::from(runtime).join("flick").join("flick.sock"))
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[tokio::test]
    async fn first_shutdown_reason_wins_and_cancels_waiters() {
        let l = DaemonLifecycle::new();
        l.request_shutdown(ShutdownReason::Ipc);
        l.request_shutdown(ShutdownReason::Signal);
        assert_eq!(l.reason(), Some(ShutdownReason::Ipc));
        tokio::time::timeout(Duration::from_millis(50), l.cancelled())
            .await
            .unwrap();
    }
    #[test]
    fn socket_resolution_requires_xdg_runtime_dir_in_every_mode() {
        for mode in [SocketMode::Graphical, SocketMode::Headless] {
            assert!(matches!(
                resolve_socket_path(None, mode, |_| None),
                Err(SocketPathError::MissingRuntimeDir)
            ));
        }
    }

    #[test]
    fn explicit_socket_overrides_environment_requirements() {
        let socket = PathBuf::from("/tmp/flick-test.sock");
        assert_eq!(
            resolve_socket_path(Some(socket.clone()), SocketMode::Graphical, |_| None).unwrap(),
            socket
        );
    }
}
