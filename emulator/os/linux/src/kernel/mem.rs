//! 内存管理语义方法（kernel 域）。
//!
//! `impl LinuxRuntime` 的内存管理只产出 mmap 语义请求与 backing 内容。
//! guest 具体地址选择、冲突检测、materialize 统一由共享
//! [`MemoryAddressSpace`](rundroid_memory::MemoryAddressSpace) authority 负责。

use super::fd_io::FdOpError;
use super::LinuxRuntime;
use crate::fd::{mmap_from_fd, Fd};
use rundroid_memory::{DynamicArena, MemoryPerms, MemoryUsage};

/// 统一的 mmap 语义请求。
#[derive(Debug, Clone)]
pub struct MmapRequest {
    /// 请求长度。
    pub length: usize,
    /// POSIX `PROT_*` 位掩码。
    pub prot: i32,
    /// guest hint 地址（0 表示无 hint）。
    pub hint_addr: u64,
    /// 统一动态分配 arena。
    pub arena: DynamicArena,
    /// 账本权限视图。
    pub perms: MemoryPerms,
    /// usage/source 元数据。
    pub usage: MemoryUsage,
    /// 可选初始内容（fd/device-backed mmap）。
    pub content: Vec<u8>,
}

/// device/file mmap 的语义结果。
#[derive(Debug)]
pub struct MmapOutcome {
    /// 设备建议地址（0 表示交给共享 authority 动态找洞）。
    pub addr: u64,
    /// backing 内容。
    pub content: Vec<u8>,
    /// POSIX `PROT_*` 位掩码。
    pub prot: i32,
    /// usage/source 元数据。
    pub usage: MemoryUsage,
}

impl LinuxRuntime {
    /// 构造匿名 mmap 请求。
    pub fn anonymous_mmap_request(
        &mut self,
        hint_addr: u64,
        length: usize,
        prot: i32,
    ) -> MmapRequest {
        MmapRequest {
            length,
            prot,
            hint_addr,
            arena: DynamicArena::new(0x7F_0000_0000, 0x7F_F000_0000),
            perms: MemoryPerms::from_flags((prot & 1) != 0, (prot & 2) != 0, (prot & 4) != 0),
            usage: MemoryUsage::AnonymousMmap,
            content: Vec::new(),
        }
    }

    /// brk 当前值（bootstrap：固定返回当前 brk，不增长）。
    pub fn brk(&self) -> u64 {
        self.brk
    }

    /// fd-backed mmap：普通文件与设备都在这里产出 backing 语义，
    /// 由 syscall 层完成统一地址分配。
    pub fn fd_mmap(
        &mut self,
        fd: Fd,
        length: usize,
        offset: u64,
        prot: i32,
        flags: i32,
    ) -> Result<Option<MmapOutcome>, FdOpError> {
        let entry = self.fds.lookup(fd).ok_or(FdOpError::BadFd)?;
        let usage = match entry.kind {
            crate::fd::FdKind::File => MemoryUsage::FileMmap,
            crate::fd::FdKind::Device => MemoryUsage::DeviceMmap,
            _ => return Ok(None),
        };
        match mmap_from_fd(entry, length, offset, prot, flags) {
            Ok(Some(region)) => Ok(Some(MmapOutcome {
                addr: region.hint_addr,
                content: region.content,
                prot: region.prot,
                usage,
            })),
            Ok(None) => Ok(None),
            Err(e) => Err(FdOpError::Io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// anonymous mmap 请求携带共享 authority 所需的 arena/perms/usage 元数据。
    #[test]
    fn anonymous_mmap_request_has_shared_authority_metadata() {
        let mut rt = LinuxRuntime::new();
        let req = rt.anonymous_mmap_request(0, 0x1000, 3);
        assert_eq!(req.length, 0x1000);
        assert_eq!(req.usage, MemoryUsage::AnonymousMmap);
        assert_eq!(req.arena.start, 0x7F_0000_0000);
        assert!(req.perms.readable());
        assert!(req.perms.writable());
    }

    /// brk 返回固定当前值（bootstrap 不增长）。
    #[test]
    fn brk_returns_current_value() {
        let rt = LinuxRuntime::new();
        assert_eq!(rt.brk(), 0x7E_0000_0000);
    }

    /// fd_mmap 对无效 fd 返回 BadFd。
    #[test]
    fn fd_mmap_bad_fd_returns_bad_fd() {
        let mut rt = LinuxRuntime::new();
        assert!(matches!(
            rt.fd_mmap(999, 0x1000, 0, 3, 0),
            Err(FdOpError::BadFd)
        ));
    }
}
