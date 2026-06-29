//! `RegionTracker` 兼容层。
//!
//! 新实现已经迁移到 [`crate::address_space::MemoryAddressSpace`]；
//! 这里保留旧名称，便于上层分阶段迁移。

use crate::address_space::{
    AllocationMode, MemoryAddressSpace, MemoryPerms, MemoryRegion, MemoryUsage,
};
use crate::error::MemoryError;

/// 旧的来源枚举，过渡期映射到新的 [`MemoryUsage`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionOrigin {
    ELFSegment,
    Stack,
    TLS,
    RuntimeScratch,
}

impl RegionOrigin {
    fn usage(self) -> MemoryUsage {
        match self {
            Self::ELFSegment => MemoryUsage::ELFImage,
            Self::Stack => MemoryUsage::Stack,
            Self::TLS => MemoryUsage::Tls,
            Self::RuntimeScratch => MemoryUsage::Scratch,
        }
    }
}

/// 兼容旧接口的 region 账本。
#[derive(Debug, Default, Clone)]
pub struct RegionTracker {
    inner: MemoryAddressSpace,
}

impl RegionTracker {
    /// 创建空账本。
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前 region 数量。
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// 返回只读 region 切片。
    pub fn regions(&self) -> &[MemoryRegion] {
        self.inner.regions()
    }

    /// 注册一段已物化区间。
    ///
    /// 兼容旧语义：仅记账，不触碰 backend。
    pub fn register(
        &mut self,
        addr: u64,
        size: u64,
        origin: RegionOrigin,
    ) -> Result<(), MemoryError> {
        self.inner.register_region(MemoryRegion {
            addr,
            size,
            perms: MemoryPerms::ALL,
            mode: AllocationMode::Reserved,
            usage: origin.usage(),
        })
    }

    /// 查找覆盖该地址的区间。
    pub fn find(&self, addr: u64) -> Option<&MemoryRegion> {
        self.inner.find(addr)
    }

    /// 暴露底层地址空间，供迁移阶段复用。
    pub fn address_space(&self) -> &MemoryAddressSpace {
        &self.inner
    }

    /// 暴露可变底层地址空间，供迁移阶段复用。
    pub fn address_space_mut(&mut self) -> &mut MemoryAddressSpace {
        &mut self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_find() {
        let mut tracker = RegionTracker::new();
        tracker
            .register(0x1000, 0x1000, RegionOrigin::ELFSegment)
            .unwrap();
        tracker.register(0x5000, 0x2000, RegionOrigin::Stack).unwrap();
        assert_eq!(tracker.len(), 2);
        assert!(tracker.find(0x1500).is_some());
        assert!(tracker.find(0x6FFF).is_some());
        assert!(tracker.find(0x3000).is_none());
    }

    #[test]
    fn rejects_overlap() {
        let mut tracker = RegionTracker::new();
        tracker.register(0x1000, 0x2000, RegionOrigin::Stack).unwrap();
        let err = tracker
            .register(0x2000, 0x1000, RegionOrigin::TLS)
            .unwrap_err();
        assert!(matches!(err, MemoryError::Overlap { .. }));
    }
}
