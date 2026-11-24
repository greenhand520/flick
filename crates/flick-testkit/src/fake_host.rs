//! 用于测试的伪 `HostEventSink`。
//!
//! `FakeHostEventSink` 收集协调器发出的所有 `HostToUi` 事件，
//! 以便用于断言。

use std::sync::{Arc, Mutex};

use flick_application::HostEventSink;
use flick_domain::HostToUi;

/// 用于 `HostEventSink` 的测试替身，记录每一个事件。
///
/// # 用法
///
/// ```ignore
/// let sink = FakeHostEventSink::new();
/// let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));
/// // … 驱动协调器 …
/// let events = sink.events();   // 快照
/// let last  = sink.last();      // 窥视但不移除
/// let all   = sink.drain();     // 排空所有事件
/// ```
pub struct FakeHostEventSink {
    events: Arc<Mutex<Vec<HostToUi>>>,
}

impl FakeHostEventSink {
    /// 创建一个新的空接收端。
    pub fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 返回所有已收集事件的克隆（**不会**排空）。
    pub fn events(&self) -> Vec<HostToUi> {
        self.events.lock().unwrap().clone()
    }

    /// 排空所有已收集的事件，使内部缓冲区为空。
    pub fn drain(&self) -> Vec<HostToUi> {
        self.events.lock().unwrap().drain(..).collect()
    }

    /// 窥视最近发出的事件（不将其移除）。
    pub fn last(&self) -> Option<HostToUi> {
        self.events.lock().unwrap().last().cloned()
    }
}

impl Default for FakeHostEventSink {
    fn default() -> Self {
        Self::new()
    }
}

impl HostEventSink for FakeHostEventSink {
    fn send(&self, event: HostToUi) {
        self.events.lock().unwrap().push(event);
    }
}
