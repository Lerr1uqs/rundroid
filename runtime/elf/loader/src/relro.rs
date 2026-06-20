//! RELRO 区域记账。
//!
//! PT_GNU_RELRO 指示一段需要"装载后改只读"的区段。
//! loader 在装载时仍按可写映射（因为有重定位要写），
//! 真正的权限收紧在 linker 完成 relocation 写回之后由 runtime 调用 backend 完成。
//!
//! 这里只提供一个轻量的"区域描述"类型，便于 runtime 记住待 RELRO 的范围。

/// 一段 RELRO 区域（guest 绝对地址）。
#[derive(Debug, Clone, Copy)]
pub struct RelroRegion {
    pub start: u64,
    pub end: u64,
}

impl RelroRegion {
    pub fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }

    pub fn size(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }
}
