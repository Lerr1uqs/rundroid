//! guest 内存权限。
//!
//! 与 Unicorn 的 `Protection` 一一对应，但本 crate 不依赖 Unicorn 类型，
//! 让 backend 实现负责映射，避免抽象层泄漏具体引擎。

/// guest 内存段的读 / 写 / 执行 / 已映射权限位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemPerms(u8);

impl MemPerms {
    pub const NONE: Self = Self(0);
    pub const READ: Self = Self(1);
    pub const WRITE: Self = Self(2);
    pub const EXEC: Self = Self(4);
    /// bootstrap 中代码段常用：可读 + 可执行。
    pub const READ_EXEC: Self = Self(Self::READ.0 | Self::EXEC.0);
    /// bootstrap 中数据段常用：可读 + 可写。
    pub const READ_WRITE: Self = Self(Self::READ.0 | Self::WRITE.0);
    /// 全开，仅用于 bootstrap smoke 阶段，正式运行时应按段精确设置。
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

    /// 按三个布尔 flag 组合权限。loader 据此把 ELF p_flags 翻译成 MemPerms。
    pub fn from_flags(read: bool, write: bool, execute: bool) -> Self {
        let mut v = 0u8;
        if read {
            v |= Self::READ.0;
        }
        if write {
            v |= Self::WRITE.0;
        }
        if execute {
            v |= Self::EXEC.0;
        }
        Self(v)
    }
}
