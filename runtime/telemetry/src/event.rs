//! 结构化事件载体。
//!
//! 设计目标：事件必须是 machine-readable 且自描述的，
//! 让 `events.jsonl` 可以被 replay / diff 工具直接消费，
//! 不依赖人类日志格式。

/// 单条 telemetry 事件。
///
/// `name` 是稳定的事件标识（例如 `"module.loaded"`），
/// `kind` 是粗分类，便于消费者按类别过滤而无需解析字符串。
#[derive(Debug, Clone)]
pub struct TelemetryEvent<'a> {
    pub name: &'a str,
    pub kind: TelemetryEventKind,
}

/// 事件的粗分类。新增类别时需要同步更新消费者。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryEventKind {
    /// runtime / session 生命周期
    Lifecycle,
    /// 内存映射、段权限变更
    Memory,
    /// ELF 解析、装载、链接步骤
    Elf,
    /// backend 执行、中断、syscall
    Execution,
}

impl<'a> TelemetryEvent<'a> {
    pub fn new(name: &'a str, kind: TelemetryEventKind) -> Self {
        Self { name, kind }
    }
}
