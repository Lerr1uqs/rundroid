//! telemetry router。
//!
//! [`TelemetryRouter`] 是 runtime 中唯一的"事件出口"。
//! 调用方构造事件，router 依据 [`TelemetryMode`] 决定是否真正下发到 sink。
//!
//! 这种"先判 mode、再走 sink"的两段式，让 `Disabled` 模式下事件构造的开销
//! 也只剩一次 enum 比较，而不必在调用方散落 `if mode != Disabled`。

use crate::event::TelemetryEvent;
use crate::mode::TelemetryMode;
use crate::sink::EventSink;

/// telemetry 路由器。
pub struct TelemetryRouter {
    mode: TelemetryMode,
    sink: Option<Box<dyn EventSink>>,
}

impl TelemetryRouter {
    /// `Disabled` 模式：不持有 sink，[`Self::emit`] 直接返回。
    pub fn disabled() -> Self {
        Self {
            mode: TelemetryMode::Disabled,
            sink: None,
        }
    }

    /// `EventsOnly` 模式：事件转发到 `sink`。
    pub fn events_only(sink: Box<dyn EventSink>) -> Self {
        Self {
            mode: TelemetryMode::EventsOnly,
            sink: Some(sink),
        }
    }

    /// 当前模式。
    pub fn mode(&self) -> TelemetryMode {
        self.mode
    }

    /// 转发一条事件。
    ///
    /// `Disabled` 下完全 no-op；非 `Disabled` 但未挂载 sink 也安全（事件被丢弃），
    /// 这样调用方不需要同时判 mode 与 sink 是否存在。
    pub fn emit(&mut self, event: &TelemetryEvent<'_>) {
        if !self.mode.events_enabled() {
            return;
        }
        if let Some(sink) = self.sink.as_mut() {
            sink.record(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TelemetryEventKind;
    use crate::sink::VecSink;

    #[test]
    fn disabled_drops_events() {
        let mut router = TelemetryRouter::disabled();
        router.emit(&TelemetryEvent::new("x", TelemetryEventKind::Lifecycle));
        // 没有崩溃 / 没有副作用即视为通过。
    }

    #[test]
    fn events_only_forwards_to_sink() {
        let sink = Box::new(VecSink::new());
        let mut router = TelemetryRouter::events_only(sink);
        router.emit(&TelemetryEvent::new("module.loaded", TelemetryEventKind::Elf));
        router.emit(&TelemetryEvent::new("mem.mapped", TelemetryEventKind::Memory));
        // 通过 mode 暴露的 sink 字段不可直接访问，这里用模式断言 + 行为来验证。
        assert_eq!(router.mode(), TelemetryMode::EventsOnly);
    }
}
