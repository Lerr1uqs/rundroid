//! runtime 层统一 error model。
//!
//! 子系统（backend / memory / elf / os）将来各自有更细的 error 枚举，
//! 它们会在装配层通过 `#[from]` 归一化进 [`RuntimeError`]，
//! 保证上层只面对一个错误类型，同时 `source()` 不丢失归因。
//!
//! bootstrap 阶段子系统尚未落地，这里只保留 core 自身需要的变体；
//! 随着各 crate 实现再逐步扩展，无需提前臆造。

use crate::arch::Arch;
use thiserror::Error;

/// runtime 顶层错误。
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// 配置非法或自相矛盾。
    #[error("runtime config error: {0}")]
    Config(#[from] ConfigError),

    /// session 状态非法（例如重复启动、生命周期错乱）。
    #[error("runtime session error: {0}")]
    Session(#[from] SessionError),

    /// 当前 runtime 不支持该架构。
    ///
    /// 单独成变体而非塞进 `Config`：架构支持矩阵是 runtime 能力面，
    /// 与配置字段合法性是两件事。
    #[error("arch not supported by this runtime: {0:?}")]
    ArchUnsupported(Arch),
}

/// 配置相关错误。
#[derive(Debug, Error)]
pub enum ConfigError {
    /// 栈 / TLS 等尺寸字段为 0 或未对齐。
    #[error("invalid memory layout: {0}")]
    InvalidMemoryLayout(&'static str),
}

/// session 生命周期错误。
#[derive(Debug, Error)]
pub enum SessionError {
    /// 在已结束的 session 上继续分配 / 调用。
    #[error("session already finished")]
    AlreadyFinished,
}
