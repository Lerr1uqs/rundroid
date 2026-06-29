//! 内存层错误模型。
//!
//! 与 backend 的 `BackendError::MapFailed` 分工：
//! - 这里描述"记账层"问题：与已存在区间重叠、布局越界、对齐非法
//! - backend 层描述"引擎拒绝"问题：unmapped access、权限拒绝
//!
//! 装载流程通常是先在本层规划，规划通过再交给 backend，
//! 因此调用方拿到的 `MemoryError` 一般早于 `BackendError`。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MemoryError {
    /// 新区间与已存在的某个 region 重叠。
    /// `existing` 是冲突的 region 索引，便于上层在 telemetry 中报告。
    #[error("region {addr:#x}+{size:#x} overlaps existing region #{existing}")]
    Overlap {
        addr: u64,
        size: u64,
        existing: usize,
    },

    /// 地址 + 长度溢出 64 位地址空间。
    #[error("region {addr:#x}+{size:#x} overflows address space")]
    Overflow { addr: u64, size: u64 },

    /// 大小为 0 或未按 page 对齐。
    #[error("invalid region size {size:#x}: {reason}")]
    InvalidSize { size: u64, reason: &'static str },

    /// 请求的地址未按要求对齐。
    #[error("address {addr:#x} is not aligned to {align:#x}")]
    Misaligned { addr: u64, align: u64 },

    /// 请求的地址未映射（在查找 / 校验场景下触发）。
    #[error("address {addr:#x} not mapped")]
    NotMapped { addr: u64 },

    /// 请求的区间没有完整覆盖现有 VMA。
    #[error("range {addr:#x}+{size:#x} is not fully covered by mapped regions")]
    RangeNotMapped { addr: u64, size: u64 },

    /// gap search 在给定 arena 内找不到满足条件的洞。
    #[error("no available gap for size {size:#x} align {align:#x} in arena {start:#x}..{end:#x}")]
    NoAvailableGap {
        size: u64,
        align: u64,
        start: u64,
        end: u64,
    },

    /// 地址空间物化执行器失败。
    #[error("materialize failed for {op} at {addr:#x}+{size:#x}: {reason}")]
    MaterializeFailed {
        op: &'static str,
        addr: u64,
        size: u64,
        reason: String,
    },
}
