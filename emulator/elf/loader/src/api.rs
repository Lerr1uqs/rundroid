//! loader trait 与装载上下文契约。
//!
//! [`LoadContext`] 是 loader 与"外部世界"（backend + 内存账本 + telemetry）
//! 之间的全部接口。把 backend 句柄挡在外面，让 loader 可以：
//! - 在没有 Unicorn 的纯单元测试中用 mock context 跑布局逻辑
//! - 不依赖具体 backend 实现

use crate::error::ElfLoadError;
use crate::model::LoadedModule;
use rundroid_elf_parser::model::SegmentPerms;
use rundroid_elf_parser::ParsedElf;
use rundroid_memory::{MemoryError, MemoryPerms, MemoryUsage};
use rundroid_telemetry::TelemetryEvent;

/// loader 抽象。
///
/// 实现方（[`crate::loader::DefaultLoader`]）应是无状态的，
/// 真正的装载状态由返回的 [`LoadedModule`] 持有。
pub trait ElfLoader: Send + Sync {
    fn load(
        &self,
        ctx: &mut dyn LoadContext,
        image: &ParsedElf,
        request: LoadRequest<'_>,
    ) -> Result<LoadedModule, ElfLoadError>;
}

/// 单次装载请求。
pub struct LoadRequest<'a> {
    /// 分配镜像空间时使用的对齐（通常 = ARM64 page size 4096）。
    pub image_align: u64,
    /// 镜像对应的原始字节流，用于按 file_offset 读取段数据。
    pub bytes: &'a [u8],
    /// 由 session 分配的模块 ID。
    pub module_id: rundroid_core::ModuleId,
}

/// 装载上下文：loader 通过它与 backend / 内存账本 / telemetry 交互。
///
/// 所有方法都接受 `&mut self`，因为 backend 句柄本身就是可变的。
#[allow(unused_variables)]
pub trait LoadContext {
    /// 在 guest 地址空间里预留一段连续空间并返回其起始地址。
    /// 实现负责通过共享 guest 地址空间 authority 完成选址与 eager materialize。
    fn reserve_image_space(&mut self, size: u64, align: u64) -> Result<u64, MemoryError>;

    /// 按 spec 映射一个段到 guest 地址空间。
    /// footprint 已在 [`Self::reserve_image_space`] 中 materialize；
    /// 这里主要用于段级权限视图与 telemetry。
    fn map_segment(&mut self, spec: SegmentMapSpec<'_>) -> Result<MappedSegment, MemoryError>;

    /// 更新已预留镜像 footprint 的权限视图。
    fn protect_segment(
        &mut self,
        guest_addr: u64,
        size: u64,
        perms: MemoryPerms,
        usage: MemoryUsage,
    ) -> Result<(), MemoryError>;

    /// 向 guest 地址写入字节。
    fn write_bytes(&mut self, guest_addr: u64, bytes: &[u8]) -> Result<(), MemoryError>;

    /// 对 `[addr, addr+len)` 做零填充。
    fn zero_fill(&mut self, guest_addr: u64, len: u64) -> Result<(), MemoryError>;

    /// 上报一条 telemetry 事件。
    fn emit(&mut self, event: TelemetryEvent<'_>);
}

/// 单个段的映射规格。
#[derive(Debug, Clone, Copy)]
pub struct SegmentMapSpec<'a> {
    pub guest_addr: u64,
    pub size: u64,
    pub perms: SegmentPerms,
    pub label: &'a str,
}

/// 段映射的结果。
#[derive(Debug, Clone, Copy)]
pub struct MappedSegment {
    pub guest_addr: u64,
    pub size: u64,
}
