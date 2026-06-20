//! 单次执行 session。
//!
//! Session 对应一次 case 运行 / 一次导出符号调用，
//! 它划定了 id 分配、telemetry 事件归属的边界。
//!
//! Session 本身不持有 guest memory 或 backend handle，
//! 那些由 backend / memory 子系统在 runtime 装配后注入，
//! 从而保持 core 与具体实现解耦。

use crate::ids::{IdAllocator, ModuleId, SessionId};

/// runtime 执行 session。
pub struct Session {
    id: SessionId,
    allocator: IdAllocator,
}

impl Session {
    /// 构造一个独立 session。
    ///
    /// `IdAllocator::new()` 给到的是从 1 开始的全新编号空间，
    /// 与外部其它 session 互不影响。
    pub fn new() -> Self {
        Self {
            id: SessionId::from_raw(0),
            allocator: IdAllocator::new(),
        }
    }

    /// 当前 session 的 ID。
    ///
    /// 注意：bootstrap 阶段 session id 固定为 0，因为 [`Self::new`]
    /// 不依赖外部分配器；后续若需要跨进程唯一，再在 runtime 层注入。
    pub fn id(&self) -> SessionId {
        self.id
    }

    /// 在本 session 内分配一个新模块 ID。
    pub fn allocate_module(&self) -> ModuleId {
        self.allocator.module()
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}
