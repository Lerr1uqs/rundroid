//! syscall 层访问 guest 目标侧内存的唯一抽象边界。
//!
//! [`MemoryBridge`] 只暴露 **读 / 写 / 映射** 三类能力，底层由装配层（case-runner /
//! Python binding）适配到 engine 的 `GuestCPU::mem_read` / `mem_write` / `mem_map` /
//! `mem_protect` / `mem_unmap`。
//!
//! 刻意保持窄接口：不包含寄存器访问（`reg_read` / `reg_write`）、`stop()` 等
//! hook 视图能力——那些属于本 crate 之外的 backend 关注点，syscall 层不需要。

/// guest 目标侧内存访问边界。
///
/// `dispatch` 与各 `sys_*` handler 接收此 trait，而不是三个独立闭包或完整 `GuestCPU`：
/// - 比三闭包更聚合（handler 只认一个边界对象，`dispatch` 签名更短）；
/// - 比直接传 `GuestCPU` 更窄（syscall 只需内存三能力，不需要 `reg_write`/`stop` 等）。
///
/// 适配示例：装配层把 `&mut dyn GuestCPU` 包成实现此 trait 的薄适配器（见
/// case-runner / Python binding 的 `CpuMemoryBridge`）。
pub trait MemoryBridge {
    /// 从 guest 地址读 `len` 字节。
    ///
    /// 失败（地址未映射／权限不足）时返回 `None`，调用方据此返回 `EFAULT`。
    fn read(&mut self, addr: u64, len: usize) -> Option<Vec<u8>>;

    /// 向 guest 地址写字节。
    ///
    /// 失败时返回 `false`，调用方据此返回 `EFAULT`
    /// （不允许"返回长度但目标缓冲没变"的假成功）。
    fn write(&mut self, addr: u64, data: &[u8]) -> bool;

    /// 在 guest 地址空间建立映射。
    ///
    /// `prot` 为 POSIX `PROT_*` 位掩码（READ=1 / WRITE=2 / EXEC=4）。
    /// 失败（地址已占用等）时返回 `false`，调用方据此返回 `EFAULT`。
    fn map(&mut self, addr: u64, len: usize, prot: i32) -> bool;

    /// 修改 guest 区间权限。
    fn protect(&mut self, addr: u64, len: usize, prot: i32) -> bool;

    /// 释放 guest 区间。
    fn unmap(&mut self, addr: u64, len: usize) -> bool;
}
