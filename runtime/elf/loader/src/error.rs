//! loader 错误模型。
//!
//! 与 parser / linker 错误严格分工（spec: Typed error separation）：
//! 这里只描述"装载阶段"问题——空间分配、段重叠、权限切换、镜像布局非法，
//! 不描述符号找不到、relocation 无法写回（那些属于 linker 错误）。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ElfLoadError {
    /// 整个镜像所需空间在 guest 地址空间里找不到合适的洞。
    #[error("cannot reserve image space: size={size:#x} align={align:#x}")]
    ReserveFailed { size: u64, align: u64 },

    /// 单个段映射失败（权限非法、与已映射区重叠、对齐非法）。
    /// 底层 [`MemoryError`](rundroid_memory::MemoryError) 作为 source 保留归因。
    #[error("segment map failed at {addr:#x}+{size:#x}")]
    SegmentMap {
        addr: u64,
        size: u64,
        #[source]
        source: rundroid_memory::MemoryError,
    },

    /// 段字节写入失败（地址未映射、越界）。
    #[error("segment write failed at {addr:#x}")]
    SegmentWrite {
        addr: u64,
        #[source]
        source: rundroid_memory::MemoryError,
    },

    /// ParsedElf 里没有任何 PT_LOAD 段——不是可装载对象。
    #[error("no PT_LOAD segments in image")]
    NoLoadableSegments,

    /// 段的 file_offset + filesz 超出输入字节流长度（镜像截断）。
    #[error("segment {vaddr:#x} filesz exceeds backing bytes")]
    SegmentDataTruncated { vaddr: u64 },
}
