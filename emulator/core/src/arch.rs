//! 目标架构。
//!
//! bootstrap 只锁定 ARM64，但用 enum 而不是常量，
//! 是因为后续 ARM32 / Thumb 接入时只需新增变体，
//! 所有匹配处会被编译器逼着显式处理新分支。

/// guest CPU 架构。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Arch {
    /// AArch64，bootstrap 唯一支持的架构。
    #[default]
    Arm64,
}
