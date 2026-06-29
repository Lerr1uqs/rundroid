//! guest 地址空间第一权威。
//!
//! [`MemoryAddressSpace`] 统一管理所有 guest VMA：ELF image、匿名/文件/设备 mmap、
//! JNI ABI、trampoline、stack、scratch 等都必须经由它分配、保护、释放。
//! bootstrap 阶段采用 eager materialize：backend 成功后才写账本。

use crate::error::MemoryError;

/// guest 页大小。
pub const PAGE_SIZE: u64 = 0x1000;

/// guest 内存权限视图。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryPerms(u8);

impl MemoryPerms {
    pub const NONE: Self = Self(0);
    pub const READ: Self = Self(1);
    pub const WRITE: Self = Self(2);
    pub const EXEC: Self = Self(4);
    pub const READ_WRITE: Self = Self(Self::READ.0 | Self::WRITE.0);
    pub const READ_EXEC: Self = Self(Self::READ.0 | Self::EXEC.0);
    pub const ALL: Self = Self(Self::READ.0 | Self::WRITE.0 | Self::EXEC.0);

    pub fn readable(self) -> bool {
        self.0 & Self::READ.0 != 0
    }

    pub fn writable(self) -> bool {
        self.0 & Self::WRITE.0 != 0
    }

    pub fn executable(self) -> bool {
        self.0 & Self::EXEC.0 != 0
    }

    pub fn from_flags(read: bool, write: bool, execute: bool) -> Self {
        let mut value = 0u8;
        if read {
            value |= Self::READ.0;
        }
        if write {
            value |= Self::WRITE.0;
        }
        if execute {
            value |= Self::EXEC.0;
        }
        Self(value)
    }
}

/// 分配模式：固定布局或动态找洞。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocationMode {
    Reserved,
    Dynamic,
}

/// VMA 用途元数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryUsage {
    ELFImage,
    AnonymousMmap,
    FileMmap,
    DeviceMmap,
    JNIEnv,
    JavaVM,
    Trampoline,
    Stack,
    Scratch,
    Relro,
    Tls,
}

/// 一段连续的 guest 虚拟内存区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRegion {
    pub addr: u64,
    pub size: u64,
    pub perms: MemoryPerms,
    pub mode: AllocationMode,
    pub usage: MemoryUsage,
}

impl MemoryRegion {
    /// 区间结束地址（exclusive）。
    pub fn end(&self) -> u64 {
        self.addr.checked_add(self.size).unwrap_or(u64::MAX)
    }

    /// 是否覆盖地址。
    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.addr && addr < self.end()
    }

    /// 与另一段范围是否有交集。
    pub fn overlaps(&self, addr: u64, size: u64) -> bool {
        overlaps(self.addr, self.size, addr, size)
    }
}

/// 动态分配的 arena 范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DynamicArena {
    pub start: u64,
    pub end: u64,
}

impl DynamicArena {
    pub const fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }
}

/// 分配请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocationRequest {
    pub size: u64,
    pub align: u64,
    pub perms: MemoryPerms,
    pub usage: MemoryUsage,
    pub placement: AllocationPlacement,
}

/// 分配位置策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocationPlacement {
    Reserved { addr: u64 },
    Dynamic { arena: DynamicArena, hint: u64 },
}

impl AllocationRequest {
    /// 构造固定地址分配请求。
    pub fn reserved(
        addr: u64,
        size: u64,
        align: u64,
        perms: MemoryPerms,
        usage: MemoryUsage,
    ) -> Self {
        Self {
            size,
            align,
            perms,
            usage,
            placement: AllocationPlacement::Reserved { addr },
        }
    }

    /// 构造动态找洞分配请求。
    pub fn dynamic(
        size: u64,
        align: u64,
        perms: MemoryPerms,
        usage: MemoryUsage,
        arena: DynamicArena,
        hint: u64,
    ) -> Self {
        Self {
            size,
            align,
            perms,
            usage,
            placement: AllocationPlacement::Dynamic { arena, hint },
        }
    }
}

/// backend 物化边界。
///
/// `MemoryAddressSpace` 只依赖这个窄接口，不依赖具体 backend crate。
pub trait MemoryMaterializer {
    /// 在 guest 建图。
    fn map(
        &mut self,
        addr: u64,
        size: u64,
        perms: MemoryPerms,
        usage: MemoryUsage,
    ) -> Result<(), MemoryError>;

    /// 修改 guest 区间权限。
    fn protect(&mut self, addr: u64, size: u64, perms: MemoryPerms) -> Result<(), MemoryError>;

    /// 释放 guest 区间。
    fn unmap(&mut self, addr: u64, size: u64) -> Result<(), MemoryError>;
}

/// guest VMA 权威。
#[derive(Debug, Default, Clone)]
pub struct MemoryAddressSpace {
    regions: Vec<MemoryRegion>,
}

impl MemoryAddressSpace {
    /// 创建空地址空间。
    pub fn new() -> Self {
        Self::default()
    }

    /// 已记录 VMA 数量。
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// 返回按地址排序的 VMA 列表。
    pub fn regions(&self) -> &[MemoryRegion] {
        &self.regions
    }

    /// 按地址查找覆盖该地址的 VMA。
    pub fn find(&self, addr: u64) -> Option<&MemoryRegion> {
        let pos = self.regions.partition_point(|region| region.addr <= addr);
        if pos == 0 {
            return None;
        }
        let candidate = &self.regions[pos - 1];
        if candidate.contains(addr) {
            Some(candidate)
        } else {
            None
        }
    }

    /// 校验并物化一个新 VMA。
    ///
    /// 只有 materializer 成功时才写账本，保证 eager materialize 语义。
    pub fn allocate(
        &mut self,
        request: AllocationRequest,
        materializer: &mut dyn MemoryMaterializer,
    ) -> Result<MemoryRegion, MemoryError> {
        let region = self.plan_region(request)?;
        materializer.map(region.addr, region.size, region.perms, region.usage)?;
        self.insert_region(region)?;
        Ok(region)
    }

    /// 仅做地址规划，不触碰 backend。
    pub fn plan_region(&self, request: AllocationRequest) -> Result<MemoryRegion, MemoryError> {
        let align = normalize_align(request.align)?;
        validate_size(request.size)?;
        let size = align_up_checked(request.size, PAGE_SIZE)?;

        let (addr, mode) = match request.placement {
            AllocationPlacement::Reserved { addr } => {
                validate_alignment(addr, align)?;
                (addr, AllocationMode::Reserved)
            }
            AllocationPlacement::Dynamic { arena, hint } => {
                let addr = self.find_gap(size, align, arena, hint)?;
                (addr, AllocationMode::Dynamic)
            }
        };

        let _end = addr
            .checked_add(size)
            .ok_or(MemoryError::Overflow { addr, size })?;
        self.ensure_no_overlap(addr, size)?;

        Ok(MemoryRegion {
            addr,
            size,
            perms: request.perms,
            mode,
            usage: request.usage,
        })
    }

    /// 更新区间权限，并在 backend 成功后回写账本。
    pub fn protect(
        &mut self,
        addr: u64,
        size: u64,
        perms: MemoryPerms,
        materializer: &mut dyn MemoryMaterializer,
    ) -> Result<(), MemoryError> {
        validate_size(size)?;
        let protect_addr = align_down(addr);
        let protect_size = align_up_checked(addr + size - protect_addr, PAGE_SIZE)?;
        let replacement = self.rewrite_covering_range(addr, size, |region, sub_start, sub_size| {
            MemoryRegion {
                addr: sub_start,
                size: sub_size,
                perms,
                mode: region.mode,
                usage: region.usage,
            }
        })?;
        materializer.protect(protect_addr, protect_size, perms)?;
        self.regions = replacement;
        Ok(())
    }

    /// 释放区间，并在 backend 成功后回写账本。
    pub fn unmap(
        &mut self,
        addr: u64,
        size: u64,
        materializer: &mut dyn MemoryMaterializer,
    ) -> Result<(), MemoryError> {
        validate_range(addr, size)?;
        let replacement = self.remove_covering_range(addr, size)?;
        materializer.unmap(addr, size)?;
        self.regions = replacement;
        Ok(())
    }

    /// 兼容旧 `RegionTracker::register` 路径：直接记账，不触碰 backend。
    ///
    /// 仅用于过渡期和纯测试；正常运行期应通过 [`Self::allocate`] 统一流程。
    pub fn register_region(&mut self, region: MemoryRegion) -> Result<(), MemoryError> {
        validate_range(region.addr, region.size)?;
        self.insert_region(region)
    }

    fn insert_region(&mut self, region: MemoryRegion) -> Result<(), MemoryError> {
        self.ensure_no_overlap(region.addr, region.size)?;
        let pos = self.regions.partition_point(|existing| existing.addr < region.addr);
        self.regions.insert(pos, region);
        Ok(())
    }

    fn ensure_no_overlap(&self, addr: u64, size: u64) -> Result<(), MemoryError> {
        for (index, existing) in self.regions.iter().enumerate() {
            if existing.overlaps(addr, size) {
                return Err(MemoryError::Overlap {
                    addr,
                    size,
                    existing: index,
                });
            }
        }
        Ok(())
    }

    fn find_gap(
        &self,
        size: u64,
        align: u64,
        arena: DynamicArena,
        hint: u64,
    ) -> Result<u64, MemoryError> {
        validate_range(arena.start, arena.end.saturating_sub(arena.start))?;
        if hint >= arena.end {
            return self.find_gap_from(size, align, arena, arena.start);
        }

        let start = align_up_checked(hint.max(arena.start), align)?;
        if let Ok(addr) = self.find_gap_from(size, align, arena, start) {
            return Ok(addr);
        }
        self.find_gap_from(size, align, arena, arena.start)
    }

    fn find_gap_from(
        &self,
        size: u64,
        align: u64,
        arena: DynamicArena,
        start: u64,
    ) -> Result<u64, MemoryError> {
        let mut cursor = align_up_checked(start.max(arena.start), align)?;
        let end_limit = arena.end;
        for region in &self.regions {
            if region.end() <= arena.start {
                continue;
            }
            if region.addr >= end_limit {
                break;
            }
            if cursor < region.addr {
                let gap_end = region.addr.min(end_limit);
                if gap_end.saturating_sub(cursor) >= size {
                    return Ok(cursor);
                }
            }
            if cursor < region.end() {
                cursor = align_up_checked(region.end(), align)?;
            }
            if cursor >= end_limit {
                break;
            }
        }

        if end_limit.saturating_sub(cursor) >= size {
            return Ok(cursor);
        }

        Err(MemoryError::NoAvailableGap {
            size,
            align,
            start: arena.start,
            end: arena.end,
        })
    }

    fn rewrite_covering_range(
        &self,
        addr: u64,
        size: u64,
        rewrite: impl Fn(MemoryRegion, u64, u64) -> MemoryRegion,
    ) -> Result<Vec<MemoryRegion>, MemoryError> {
        let end = addr
            .checked_add(size)
            .ok_or(MemoryError::Overflow { addr, size })?;
        let mut replacement = Vec::with_capacity(self.regions.len() + 2);
        let mut cursor = addr;

        for region in &self.regions {
            if region.end() <= addr || region.addr >= end {
                replacement.push(*region);
                continue;
            }

            if cursor < region.addr {
                return Err(MemoryError::RangeNotMapped { addr, size });
            }

            if region.addr < addr {
                replacement.push(MemoryRegion {
                    addr: region.addr,
                    size: addr - region.addr,
                    perms: region.perms,
                    mode: region.mode,
                    usage: region.usage,
                });
            }

            let cover_start = cursor.max(region.addr);
            let cover_end = end.min(region.end());
            replacement.push(rewrite(*region, cover_start, cover_end - cover_start));

            if cover_end < region.end() {
                replacement.push(MemoryRegion {
                    addr: cover_end,
                    size: region.end() - cover_end,
                    perms: region.perms,
                    mode: region.mode,
                    usage: region.usage,
                });
            }
            cursor = cover_end;
        }

        if cursor != end {
            return Err(MemoryError::RangeNotMapped { addr, size });
        }
        Ok(replacement)
    }

    fn remove_covering_range(&self, addr: u64, size: u64) -> Result<Vec<MemoryRegion>, MemoryError> {
        let end = addr
            .checked_add(size)
            .ok_or(MemoryError::Overflow { addr, size })?;
        let mut replacement = Vec::with_capacity(self.regions.len());
        let mut cursor = addr;

        for region in &self.regions {
            if region.end() <= addr || region.addr >= end {
                replacement.push(*region);
                continue;
            }

            if cursor < region.addr {
                return Err(MemoryError::RangeNotMapped { addr, size });
            }

            if region.addr < addr {
                replacement.push(MemoryRegion {
                    addr: region.addr,
                    size: addr - region.addr,
                    perms: region.perms,
                    mode: region.mode,
                    usage: region.usage,
                });
            }

            let cover_end = end.min(region.end());
            if cover_end < region.end() {
                replacement.push(MemoryRegion {
                    addr: cover_end,
                    size: region.end() - cover_end,
                    perms: region.perms,
                    mode: region.mode,
                    usage: region.usage,
                });
            }
            cursor = cover_end;
        }

        if cursor != end {
            return Err(MemoryError::RangeNotMapped { addr, size });
        }
        Ok(replacement)
    }
}

fn normalize_align(align: u64) -> Result<u64, MemoryError> {
    let normalized = if align == 0 { PAGE_SIZE } else { align.max(PAGE_SIZE) };
    if !normalized.is_power_of_two() {
        return Err(MemoryError::InvalidSize {
            size: normalized,
            reason: "alignment must be power-of-two",
        });
    }
    Ok(normalized)
}

fn validate_size(size: u64) -> Result<(), MemoryError> {
    if size == 0 {
        return Err(MemoryError::InvalidSize {
            size,
            reason: "size must be non-zero",
        });
    }
    Ok(())
}

fn validate_alignment(addr: u64, align: u64) -> Result<(), MemoryError> {
    if addr & (align - 1) != 0 {
        return Err(MemoryError::Misaligned { addr, align });
    }
    Ok(())
}

fn validate_range(addr: u64, size: u64) -> Result<(), MemoryError> {
    validate_size(size)?;
    validate_alignment(addr, PAGE_SIZE)?;
    validate_alignment(size, PAGE_SIZE)?;
    let _end = addr
        .checked_add(size)
        .ok_or(MemoryError::Overflow { addr, size })?;
    Ok(())
}

fn align_up_checked(value: u64, align: u64) -> Result<u64, MemoryError> {
    let mask = align - 1;
    let adjusted = value
        .checked_add(mask)
        .ok_or(MemoryError::Overflow {
            addr: value,
            size: mask,
        })?;
    Ok(adjusted & !mask)
}

fn align_down(value: u64) -> u64 {
    value & !(PAGE_SIZE - 1)
}

fn overlaps(a_addr: u64, a_size: u64, b_addr: u64, b_size: u64) -> bool {
    let a_end = a_addr.checked_add(a_size).unwrap_or(u64::MAX);
    let b_end = b_addr.checked_add(b_size).unwrap_or(u64::MAX);
    a_addr < b_end && b_addr < a_end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingMaterializer {
        maps: Vec<(u64, u64, MemoryPerms, MemoryUsage)>,
        protects: Vec<(u64, u64, MemoryPerms)>,
        unmaps: Vec<(u64, u64)>,
        fail_map: bool,
        fail_protect: bool,
        fail_unmap: bool,
    }

    impl MemoryMaterializer for RecordingMaterializer {
        fn map(
            &mut self,
            addr: u64,
            size: u64,
            perms: MemoryPerms,
            usage: MemoryUsage,
        ) -> Result<(), MemoryError> {
            if self.fail_map {
                return Err(MemoryError::MaterializeFailed {
                    op: "map",
                    addr,
                    size,
                    reason: "mock map fail".into(),
                });
            }
            self.maps.push((addr, size, perms, usage));
            Ok(())
        }

        fn protect(&mut self, addr: u64, size: u64, perms: MemoryPerms) -> Result<(), MemoryError> {
            if self.fail_protect {
                return Err(MemoryError::MaterializeFailed {
                    op: "protect",
                    addr,
                    size,
                    reason: "mock protect fail".into(),
                });
            }
            self.protects.push((addr, size, perms));
            Ok(())
        }

        fn unmap(&mut self, addr: u64, size: u64) -> Result<(), MemoryError> {
            if self.fail_unmap {
                return Err(MemoryError::MaterializeFailed {
                    op: "unmap",
                    addr,
                    size,
                    reason: "mock unmap fail".into(),
                });
            }
            self.unmaps.push((addr, size));
            Ok(())
        }
    }

    #[test]
    fn reserved_overlap_rejected_before_materialize() {
        let mut space = MemoryAddressSpace::new();
        let mut materializer = RecordingMaterializer::default();
        space
            .allocate(
                AllocationRequest::reserved(
                    0x4000_0000,
                    0x2000,
                    PAGE_SIZE,
                    MemoryPerms::ALL,
                    MemoryUsage::ELFImage,
                ),
                &mut materializer,
            )
            .unwrap();

        let err = space
            .allocate(
                AllocationRequest::reserved(
                    0x4000_1000,
                    0x1000,
                    PAGE_SIZE,
                    MemoryPerms::READ_WRITE,
                    MemoryUsage::Scratch,
                ),
                &mut materializer,
            )
            .unwrap_err();
        assert!(matches!(err, MemoryError::Overlap { .. }));
        assert_eq!(materializer.maps.len(), 1, "overlap 不应触发第二次 map");
    }

    #[test]
    fn dynamic_allocation_skips_occupied_gap() {
        let mut space = MemoryAddressSpace::new();
        let mut materializer = RecordingMaterializer::default();
        space
            .allocate(
                AllocationRequest::reserved(
                    0x4000_0000,
                    0x3000,
                    PAGE_SIZE,
                    MemoryPerms::ALL,
                    MemoryUsage::ELFImage,
                ),
                &mut materializer,
            )
            .unwrap();

        let region = space
            .allocate(
                AllocationRequest::dynamic(
                    0x2000,
                    PAGE_SIZE,
                    MemoryPerms::READ_WRITE,
                    MemoryUsage::AnonymousMmap,
                    DynamicArena::new(0x4000_0000, 0x4001_0000),
                    0x4000_0000,
                ),
                &mut materializer,
            )
            .unwrap();
        assert_eq!(region.addr, 0x4000_3000);
    }

    #[test]
    fn dynamic_allocation_reuses_gap_after_unmap() {
        let mut space = MemoryAddressSpace::new();
        let mut materializer = RecordingMaterializer::default();
        let first = space
            .allocate(
                AllocationRequest::dynamic(
                    0x2000,
                    PAGE_SIZE,
                    MemoryPerms::READ_WRITE,
                    MemoryUsage::AnonymousMmap,
                    DynamicArena::new(0x7F00_0000_0000, 0x7F00_0010_0000),
                    0x7F00_0000_0000,
                ),
                &mut materializer,
            )
            .unwrap();
        let second = space
            .allocate(
                AllocationRequest::dynamic(
                    0x2000,
                    PAGE_SIZE,
                    MemoryPerms::READ_WRITE,
                    MemoryUsage::AnonymousMmap,
                    DynamicArena::new(0x7F00_0000_0000, 0x7F00_0010_0000),
                    0x7F00_0000_0000,
                ),
                &mut materializer,
            )
            .unwrap();
        assert_ne!(first.addr, second.addr);

        space
            .unmap(first.addr, first.size, &mut materializer)
            .unwrap();

        let reused = space
            .allocate(
                AllocationRequest::dynamic(
                    0x2000,
                    PAGE_SIZE,
                    MemoryPerms::READ_WRITE,
                    MemoryUsage::AnonymousMmap,
                    DynamicArena::new(0x7F00_0000_0000, 0x7F00_0010_0000),
                    0x7F00_0000_0000,
                ),
                &mut materializer,
            )
            .unwrap();
        assert_eq!(reused.addr, first.addr);
    }

    #[test]
    fn protect_updates_permissions_with_split() {
        let mut space = MemoryAddressSpace::new();
        let mut materializer = RecordingMaterializer::default();
        let region = space
            .allocate(
                AllocationRequest::reserved(
                    0x5000_0000,
                    0x3000,
                    PAGE_SIZE,
                    MemoryPerms::ALL,
                    MemoryUsage::ELFImage,
                ),
                &mut materializer,
            )
            .unwrap();

        space
            .protect(
                region.addr + 0x1000,
                0x1000,
                MemoryPerms::READ_EXEC,
                &mut materializer,
            )
            .unwrap();

        assert_eq!(space.regions.len(), 3);
        assert_eq!(space.regions[1].addr, 0x5000_1000);
        assert_eq!(space.regions[1].perms, MemoryPerms::READ_EXEC);
    }

    #[test]
    fn backend_failure_leaves_no_ledger_entry() {
        let mut space = MemoryAddressSpace::new();
        let mut materializer = RecordingMaterializer {
            fail_map: true,
            ..Default::default()
        };
        let err = space
            .allocate(
                AllocationRequest::reserved(
                    0x6000_0000,
                    0x1000,
                    PAGE_SIZE,
                    MemoryPerms::READ_WRITE,
                    MemoryUsage::Scratch,
                ),
                &mut materializer,
            )
            .unwrap_err();
        assert!(matches!(err, MemoryError::MaterializeFailed { op: "map", .. }));
        assert!(space.is_empty());
    }

    #[test]
    fn rejects_misaligned_reserved_address() {
        let space = MemoryAddressSpace::new();
        let err = space
            .plan_region(AllocationRequest::reserved(
                0x4000_0001,
                0x1000,
                PAGE_SIZE,
                MemoryPerms::READ_WRITE,
                MemoryUsage::Scratch,
            ))
            .unwrap_err();
        assert!(matches!(err, MemoryError::Misaligned { .. }));
    }

    #[test]
    fn rejects_invalid_dynamic_hint_without_returning_occupied_address() {
        let space = MemoryAddressSpace::new();
        let err = space
            .plan_region(AllocationRequest::dynamic(
                0x2000,
                PAGE_SIZE,
                MemoryPerms::READ_WRITE,
                MemoryUsage::AnonymousMmap,
                DynamicArena::new(0x1000, 0x2000),
                u64::MAX - 0x800,
            ))
            .unwrap_err();
        assert!(matches!(
            err,
            MemoryError::NoAvailableGap { .. } | MemoryError::Overflow { .. }
        ));
    }
}
