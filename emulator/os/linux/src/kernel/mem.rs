//! 内存管理语义方法（kernel 域）。
//!
//! `impl LinuxRuntime` 的内存管理：alloc_mmap_addr / brk / munmap / device_mmap。
//! 地址分配只推进 `next_mmap` 状态，**不碰目标侧映射**——目标侧 mem_map 由
//! syscall 层经 [`crate::memory_bridge::MemoryBridge::map`] 落地。

use super::fd_io::FdOpError;
use super::LinuxRuntime;
use crate::fd::{mmap_from_fd, Fd};

/// device-backed mmap 的 kernel 结果：地址 + 初始内容 + prot。
///
/// syscall 层据此完成目标侧 map + 内容显式回写
/// （先 [`MemoryBridge::map`]，再 [`MemoryBridge::write`]，不依赖 map 的隐式写）。
///
/// [`MemoryBridge::map`]: crate::memory_bridge::MemoryBridge::map
/// [`MemoryBridge::write`]: crate::memory_bridge::MemoryBridge::write
#[derive(Debug)]
pub struct MmapOutcome {
    /// 映射起始地址（设备 hint 或 runtime 分配）。
    pub addr: u64,
    /// 设备返回的初始内容字节。
    pub content: Vec<u8>,
    /// 区域权限（POSIX `PROT_*` 位掩码）。
    pub prot: i32,
}

impl LinuxRuntime {
    /// 为匿名 mmap 分配地址。
    ///
    /// bootstrap 使用固定 1MiB 步长推进 `next_mmap`（与请求长度无关），
    /// 返回分配到的地址。`length` 参数保留供未来按页对齐分配使用。
    /// 纯状态推进——目标侧映射由 syscall 层经 `MemoryBridge::map` 落地。
    pub fn alloc_mmap_addr(&mut self, _length: usize) -> u64 {
        let addr = self.next_mmap;
        self.next_mmap = self.next_mmap.checked_add(0x10_0000).unwrap_or(addr);
        addr
    }

    /// brk 当前值（bootstrap：固定返回当前 brk，不增长）。
    pub fn brk(&self) -> u64 {
        self.brk
    }

    /// munmap：bootstrap 空实现（恒成功，返回 0）。
    pub fn munmap(&mut self, _addr: u64, _length: usize) -> isize {
        0
    }

    /// device-backed mmap：取设备 region 并决定地址。
    ///
    /// 推进 `next_mmap`（当 region 无 hint 时），返回 [`MmapOutcome`]。
    /// **不碰目标侧内存**——内容字节由 syscall 层经 `MemoryBridge` 显式 map + write 落地。
    ///
    /// 返回：
    /// - `Ok(Some)`：设备支持 mmap，返回 region（syscall 映射成功）。
    /// - `Ok(None)`：设备/文件不支持 mmap（syscall 映射 ENOTTY）。
    /// - `Err(BadFd)`：fd 无效（syscall 映射 EBADF）。
    /// - `Err(Io(_))`：mmap 底座错误（syscall 映射 EINVAL）。
    pub fn device_mmap(
        &mut self,
        fd: Fd,
        length: usize,
        offset: u64,
        prot: i32,
        flags: i32,
    ) -> Result<Option<MmapOutcome>, FdOpError> {
        let entry = self.fds.lookup(fd).ok_or(FdOpError::BadFd)?;
        match mmap_from_fd(entry, length, offset, prot, flags) {
            Ok(Some(region)) => {
                let addr = if region.hint_addr != 0 {
                    region.hint_addr
                } else {
                    // 无 hint：按 region 内容长度推进 next_mmap（与匿名 1MiB 步长不同）。
                    let a = self.next_mmap;
                    self.next_mmap = self
                        .next_mmap
                        .checked_add((region.content.len() as u64).max(0x1000))
                        .unwrap_or(a);
                    a
                };
                Ok(Some(MmapOutcome {
                    addr,
                    content: region.content,
                    prot: region.prot,
                }))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(FdOpError::Io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// alloc_mmap_addr 地址递增（1MiB 步长），且不碰目标侧内存。
    #[test]
    fn alloc_mmap_addr_increments_and_pure_state() {
        let mut rt = LinuxRuntime::new();
        let a1 = rt.alloc_mmap_addr(0x1000);
        let a2 = rt.alloc_mmap_addr(0x1000);
        assert!(a1 >= 0x7F_0000_0000, "首地址应在 mmap 区间");
        assert_eq!(a2, a1 + 0x10_0000, "步长应为 1MiB");
    }

    /// brk 返回固定当前值（bootstrap 不增长）。
    #[test]
    fn brk_returns_current_value() {
        let rt = LinuxRuntime::new();
        assert_eq!(rt.brk(), 0x7E_0000_0000);
    }

    /// munmap 空实现恒返回 0。
    #[test]
    fn munmap_is_noop_success() {
        let mut rt = LinuxRuntime::new();
        assert_eq!(rt.munmap(0xdead_beef, 0x1000), 0);
    }

    /// device_mmap 对无效 fd 返回 BadFd。
    #[test]
    fn device_mmap_bad_fd_returns_bad_fd() {
        let mut rt = LinuxRuntime::new();
        assert!(matches!(
            rt.device_mmap(999, 0x1000, 0, 3, 0),
            Err(FdOpError::BadFd)
        ));
    }
}
