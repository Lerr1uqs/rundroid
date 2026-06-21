//! 稳定 ID 类型与分配器。
//!
//! 这些 ID 只在单次 runtime 进程内有意义，用于 loader / linker / telemetry
//! 之间引用模块而无需拷贝名字字符串。
//!
//! 设计上避免引入 uuid 之类的依赖：bootstrap 的回归场景下，
//! 单调递增的 u64 足够唯一，且在 events.jsonl 中更紧凑、可读。

use std::sync::atomic::{AtomicU64, Ordering};

/// 已装载模块的稳定 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(u64);

/// 单次 case / session 执行的 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(u64);

impl ModuleId {
    /// 暴露内部 u64，仅用于日志 / 序列化，调用方不应基于具体数值做判断。
    pub fn raw(self) -> u64 {
        self.0
    }
}

impl SessionId {
    /// crate 内构造，bootstrap 阶段 session id 由 runtime 决定。
    pub(crate) fn from_raw(value: u64) -> Self {
        Self(value)
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

/// ID 分配器。
///
/// 故意做成显式对象而不是全局静态：分配顺序与 runtime 实例绑定，
/// 便于在多 session / 测试场景下复现一致编号。
#[derive(Debug, Default)]
pub struct IdAllocator {
    next: AtomicU64,
}

impl IdAllocator {
    pub const fn new() -> Self {
        Self { next: AtomicU64::new(1) }
    }

    /// 分配下一个模块 ID。从 1 开始，0 保留为"未分配"哨兵。
    pub fn module(&self) -> ModuleId {
        ModuleId(self.next.fetch_add(1, Ordering::Relaxed))
    }
}
