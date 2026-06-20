//! backend 错误模型。
//!
//! 保持粗粒度：上层关心"映射失败 / 读写越界 / 执行异常"等类别，
//! 不需要关心 Unicorn 的 `uc_error` 枚举细节，
//! 后者由 backend 实现层翻译成这里的变体。

use thiserror::Error;

/// backend 抽象层统一错误。
#[derive(Debug, Error)]
pub enum BackendError {
    /// `mem_map` 时传入未对齐地址 / 大小为 0 / 与已映射区域重叠。
    #[error("memory map failed at {addr:#x}+{size:#x}: {reason}")]
    MapFailed {
        addr: u64,
        size: u64,
        reason: &'static str,
    },

    /// `mem_read` / `mem_write` 触发了未映射或权限不足的地址。
    #[error("memory access failed at {addr:#x} len {len}")]
    MemoryAccess { addr: u64, len: usize },

    /// 传入了不存在的寄存器编号。
    #[error("invalid register: {0}")]
    InvalidRegister(u32),

    /// `emu_start` 期间发生异常：未映射执行、非法指令、中断未处理等。
    #[error("emulation failed: {0}")]
    Emulation(&'static str),

    /// backend 初始化失败（例如引擎自身构造失败）。
    #[error("backend initialization failed: {0}")]
    Init(&'static str),
}
