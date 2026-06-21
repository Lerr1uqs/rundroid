//! 事件落地接口（sink）。
//!
//! router 把"事件是否发出"和"事件发到哪里"解耦：
//! - router 自己只关心 [`TelemetryMode`]
//! - 具体的落盘 / 上报 / 缓冲由 [`EventSink`] 实现负责
//!
//! 这样后续要支持 `events.jsonl` / OTLP / 内存缓冲，
//! 都只需要新增 sink 实现，不动 router 与调用方。

use crate::event::TelemetryEvent;

/// 事件落地 trait。
///
/// 实现方负责把事件转成自己需要的格式（JSON 行 / 二进制 / 内存结构）。
/// trait 是 `&mut self`，因为大多数 sink 需要维护内部状态（缓冲、计数、文件句柄）。
pub trait EventSink: Send {
    fn record(&mut self, event: &TelemetryEvent<'_>);
}

/// 把所有事件收集进 `Vec` 的 sink，仅用于测试与内存 replay。
pub struct VecSink {
    pub events: Vec<(String, crate::event::TelemetryEventKind)>,
}

impl VecSink {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }
}

impl Default for VecSink {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSink for VecSink {
    fn record(&mut self, event: &TelemetryEvent<'_>) {
        // `TelemetryEvent` 持有借用 `&'a str`，无法直接 long-lived 存储，
        // 因此这里拷贝 name 成 String；测试场景下足够。
        self.events.push((event.name.to_string(), event.kind));
    }
}
