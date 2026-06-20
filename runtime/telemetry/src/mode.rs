//! telemetry 的开关粒度。
//!
//! 为什么是 enum 而不是一组 bool flag：
//! 不同观测手段之间存在互斥关系（例如 `Disabled` 下任何输出都不应该发生），
//! 用 enum 让"当前可用面"成为单一事实源，调用方不需要在每条事件处再判断多个开关。

/// runtime telemetry 的运行模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TelemetryMode {
    /// 完全关闭，router 丢弃所有事件，等价于零开销 no-op。
    #[default]
    Disabled,
    /// 只允许结构化事件输出（machine-readable），屏蔽人类日志。
    /// bootstrap 阶段要求至少支持这一档，用于产生 `events.jsonl`。
    EventsOnly,
    /// 全量观测：结构化事件 + trace + debugger transcript。
    /// bootstrap 不要求实现，仅占位以便后续扩展时不破坏 enum 兼容性。
    Full,
}

impl TelemetryMode {
    /// 当前模式下是否允许发出任何事件。
    /// router / 调用方在热路径上据此提前 short-circuit，避免构造事件 payload。
    pub fn events_enabled(self) -> bool {
        !matches!(self, TelemetryMode::Disabled)
    }
}
