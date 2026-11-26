//! 应用级剪贴板观察器：只记录内容变化时刻，不解析内容。
//! 轮询任务持有 `&mut AsyncApp`，因此窗口销毁/重建不会丢失 5 秒窗口。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::{App, AsyncApp, ClipboardItem};

use crate::input_capture::clipboard_fingerprint;

/// 剪贴板最近一次变化的指纹与时刻。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClipboardRecent {
    pub fingerprint: u64,
    pub changed_at: Instant,
}

/// 观察器的共享状态：上一帧指纹与变化时刻。
#[derive(Default)]
struct WatchState {
    last_fingerprint: Option<u64>,
    last_change_at: Option<Instant>,
    /// 启动时的基线时刻：用户刚启动应用就开框时，把启动前已存在的剪贴板
    /// 也视为“最近 5 秒内”，避免 demo/短生命周期进程读取不到启动前复制的内容。
    baseline_at: Option<Instant>,
}

/// 剪贴板观察器句柄；可克隆并跨窗口重建使用。
#[derive(Clone)]
pub(crate) struct ClipboardWatch {
    state: Arc<Mutex<WatchState>>,
}

impl ClipboardWatch {
    /// 判定窗口打开时是否应自动带入剪贴板：`observe` 后变化时刻仍在 5 秒内。
    pub const RECENT_WINDOW: Duration = Duration::from_secs(5);
    /// 轮询周期；只用于发现“内容刚变化”。
    const POLL_INTERVAL: Duration = Duration::from_secs(1);

    /// 启动应用级轮询任务；任务随 `Application` 生命周期结束。
    ///
    /// 启动时先把当前剪贴板作为基线（只记指纹、不记变化时刻），避免应用启动前
    /// 早已存在的旧内容被误判为“最近 5 秒变化”；之后真正的变化才更新时间。
    pub(crate) fn start(cx: &mut App) -> Self {
        let baseline = cx
            .read_from_clipboard()
            .map(|item| clipboard_fingerprint(&item));
        let watch = Self {
            state: Arc::new(Mutex::new(WatchState {
                last_fingerprint: baseline,
                last_change_at: None,
                baseline_at: Some(Instant::now()),
            })),
        };
        let state = Arc::clone(&watch.state);
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(Self::POLL_INTERVAL).await;
                let Some(item) = cx.update(|app| app.read_from_clipboard()) else {
                    continue;
                };
                let now = Instant::now();
                let fingerprint = clipboard_fingerprint(&item);
                let mut guard = state.lock().unwrap();
                if guard.last_fingerprint != Some(fingerprint) {
                    guard.last_fingerprint = Some(fingerprint);
                    guard.last_change_at = Some(now);
                }
            }
        })
        .detach();
        watch
    }

    /// 用当前剪贴板内容刷新观察器；返回该内容的变化记录。
    pub(crate) fn observe(&self, item: &ClipboardItem, now: Instant) -> ClipboardRecent {
        let fingerprint = clipboard_fingerprint(item);
        let mut guard = self.state.lock().unwrap();
        if guard.last_fingerprint != Some(fingerprint) {
            guard.last_fingerprint = Some(fingerprint);
            guard.last_change_at = Some(now);
        }
        ClipboardRecent {
            fingerprint,
            changed_at: guard.last_change_at.or(guard.baseline_at).unwrap_or(now),
        }
    }

    /// 若最近一次变化仍在 5 秒窗口内，返回其记录；否则返回 `None`。
    pub(crate) fn recent_at(&self, now: Instant) -> Option<ClipboardRecent> {
        let guard = self.state.lock().unwrap();
        let changed_at = guard.last_change_at.or(guard.baseline_at)?;
        let recent = now.saturating_duration_since(changed_at) <= Self::RECENT_WINDOW;
        recent.then(|| ClipboardRecent {
            fingerprint: guard.last_fingerprint.unwrap_or_default(),
            changed_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::ClipboardItem;

    #[test]
    fn observe_tracks_change_and_recent_window() {
        let watch = ClipboardWatch {
            state: Arc::new(Mutex::new(WatchState::default())),
        };
        let base = Instant::now();
        let a = ClipboardItem::new_string("a".into());
        let b = ClipboardItem::new_string("b".into());

        let first = watch.observe(&a, base);
        assert_eq!(first.changed_at, base);
        assert_eq!(watch.observe(&a, base + Duration::from_millis(100)), first);
        assert!(watch.recent_at(base + Duration::from_secs(4)).is_some());

        let second = watch.observe(&b, base + Duration::from_secs(6));
        assert_eq!(second.changed_at, base + Duration::from_secs(6));
        assert_ne!(first.fingerprint, second.fingerprint);
        assert!(watch.recent_at(base + Duration::from_secs(12)).is_none());
    }

    #[test]
    fn startup_baseline_is_recent_only_within_first_five_seconds() {
        let base = Instant::now();
        let watch = ClipboardWatch {
            state: Arc::new(Mutex::new(WatchState {
                last_fingerprint: Some(1),
                last_change_at: None,
                baseline_at: Some(base),
            })),
        };
        assert!(watch.recent_at(base + Duration::from_secs(4)).is_some());
        assert!(watch.recent_at(base + Duration::from_secs(6)).is_none());
    }
}
