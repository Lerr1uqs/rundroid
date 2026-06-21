//! guest 地址区间跟踪。
//!
//! [`RegionTracker`] 维护一个按起始地址排序的 region 列表，
//! 既能 O(log n) 查询，又能在装载阶段快速检测重叠。
//!
//! 这里只记账，不持有 backend 句柄——
//! 真正的 `mem_map` 由 loader 在调用 backend 之后再 `register` 进来，
//! 一旦 backend 失败就不应记账，所以顺序是"先 backend、后记账"。

use crate::error::MemoryError;

/// guest 内存的来源分类，便于 telemetry 与调试时区分"这块内存为什么存在"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionOrigin {
    /// ELF 镜像的某个 PT_LOAD 段。
    ELFSegment,
    /// 主线程栈。
    Stack,
    /// TLS 模板区。
    TLS,
    /// 装载器 / runtime 自用的 scratch 区（例如 trampoline）。 暂时的
    RuntimeScratch,
}

/// 一段连续的 guest 内存区间。
#[derive(Debug, Clone, Copy)]
pub struct MemoryRegion {
    pub addr: u64,
    pub size: u64,
    pub origin: RegionOrigin,
}

impl MemoryRegion {
    /// 区间是否覆盖 `addr`。
    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.addr && addr < self.addr.saturating_add(self.size)
    }

    /// 区间的结束地址（exclusive）。
    pub fn end(&self) -> u64 {
        self.addr.checked_add(self.size).unwrap_or(u64::MAX)
    }
}

/// guest 地址空间的 region 账本。
#[derive(Debug, Default)]
pub struct RegionTracker {
    // 维护"按 addr 排序且不重叠"的不变量；
    // bootstrap 阶段 region 数量有限，线性插入足够，避免引入 BTreeMap 复杂度。
    regions: Vec<MemoryRegion>,
}

impl RegionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前已记录的 region 数量。
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// 已记录 region 的只读切片（按 addr 升序）。
    pub fn regions(&self) -> &[MemoryRegion] {
        &self.regions
    }

    /// 注册一段已映射的 region。
    ///
    /// 调用时机：backend `mem_map` 成功之后。
    /// 重叠 / 溢出 / 大小非法时返回错误，且不修改账本。
    pub fn register(
        &mut self,
        addr: u64,
        size: u64,
        origin: RegionOrigin,
    ) -> Result<(), MemoryError> {
        if size == 0 {
            return Err(MemoryError::InvalidSize {
                size,
                reason: "size must be non-zero",
            });
        }
        let _ = addr.checked_add(size).ok_or(MemoryError::Overflow { addr, size })?;

        // 与现有 region 检测重叠。
        for (idx, existing) in self.regions.iter().enumerate() {
            if overlaps(addr, size, existing.addr, existing.size) {
                return Err(MemoryError::Overlap {
                    addr,
                    size,
                    existing: idx,
                });
            }
        }

        // 保持按 addr 升序插入，方便后续区间查询。
        let pos = self
            .regions
            .partition_point(|r| r.addr < addr);
        self.regions.insert(
            pos,
            MemoryRegion { addr, size, origin },
        );
        Ok(())
    }

    /// 查找覆盖 `addr` 的 region。
    pub fn find(&self, addr: u64) -> Option<&MemoryRegion> {
        // 二分定位到首个起始地址 > addr 的位置，往前一格即为候选。
        let pos = self.regions.partition_point(|r| r.addr <= addr);
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
}

/// 判断 `[a_addr, a_addr+a_size)` 与 `[b_addr, b_addr+b_size)` 是否相交。
fn overlaps(a_addr: u64, a_size: u64, b_addr: u64, b_size: u64) -> bool {
    let a_end = a_addr.checked_add(a_size).unwrap_or(u64::MAX);
    let b_end = b_addr.checked_add(b_size).unwrap_or(u64::MAX);
    a_addr < b_end && b_addr < a_end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_find() {
        let mut t = RegionTracker::new();
        t.register(0x1000, 0x1000, RegionOrigin::ELFSegment).unwrap();
        t.register(0x5000, 0x2000, RegionOrigin::Stack).unwrap();
        assert_eq!(t.len(), 2);
        assert!(t.find(0x1500).is_some());
        assert!(t.find(0x6fff).is_some());
        assert!(t.find(0x3000).is_none());
    }

    #[test]
    fn rejects_overlap() {
        let mut t = RegionTracker::new();
        t.register(0x1000, 0x1000, RegionOrigin::Stack).unwrap();
        let err = t
            .register(0x1500, 0x1000, RegionOrigin::TLS)
            .unwrap_err();
        assert!(matches!(err, MemoryError::Overlap { .. }));
    }

    #[test]
    fn rejects_zero_size() {
        let mut t = RegionTracker::new();
        assert!(matches!(
            t.register(0x1000, 0, RegionOrigin::Stack),
            Err(MemoryError::InvalidSize { .. })
        ));
    }
}
