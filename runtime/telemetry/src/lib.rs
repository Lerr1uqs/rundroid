//! `rundroid-telemetry`
//!
//! 统一 telemetry 子系统。日志、结构化事件、trace、debugger transcript
//! 都通过同一个 router 暴露，受 [`TelemetryMode`] 控制。
//!
//! bootstrap 阶段至少实现 [`TelemetryMode::Disabled`] 与 [`TelemetryMode::EventsOnly`]：
//! - `Disabled`：router 完全 no-op，调用方零开销
//! - `EventsOnly`：把结构化事件转发到 [`EventSink`]，由 sink 决定落盘 / 内存 / 上报
//!
//! `Full` 模式（trace + debugger transcript）暂时与 `EventsOnly` 行为一致，
//! 等后续 trace / debugger 子任务接入后再扩展。

#![forbid(unsafe_code)]

pub mod event;
pub mod mode;
pub mod router;
pub mod sink;

pub use event::{TelemetryEvent, TelemetryEventKind};
pub use mode::TelemetryMode;
pub use router::TelemetryRouter;
pub use sink::{EventSink, VecSink};
