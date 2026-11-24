//! `flick-testkit` — Flick 各 crate 的测试工具与伪实现。
//!
//! 本 crate 为 `flick-application` 中定义的端口 trait 以及
//! `flick-domain` 中的领域类型提供可复用的测试替身（伪实现、桩、间谍）。
//! 在需要集成测试支持的任意 Flick crate 中，将其作为 `[dev-dependency]`
//! 导入即可。

pub mod fake_host;

pub use fake_host::FakeHostEventSink;
