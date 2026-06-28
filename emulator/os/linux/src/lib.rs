//! `rundroid-linux`
//!
//! Linux 用户态运行时：负责 syscall 分发、fd 生命周期、
//! VFS 虚拟路径挂载和设备注册。
//!
//! # 体系结构（kernel / syscall 分层）
//!
//! - [`kernel`]：Linux OS 聚合根（[`kernel::LinuxRuntime`]）+ 各子系统域语义方法
//!   （fd IO / 内存管理 / 随机数）。只产出数据／推进 OS 状态，不碰目标侧内存。
//! - [`syscall`]：syscall ABI 边界（syscall 号 / [`syscall::SyscallResult`] / `dispatch` /
//!   `sys_*` handler），通过 [`memory_bridge::MemoryBridge`] 完成目标侧回写。
//! - [`errno`]：errno 常量与底座错误到 errno 的集中映射。
//! - [`vfs`]：虚拟文件系统挂载表。
//! - [`fd`]：fd 句柄表与底座 read/write/ioctl/fstat/mmap 函数。
//!
//! 设备行为定义在 `rundroid-driver` crate 中；
//! 本 crate 负责把设备操作结果落地到目标侧内存。

#![forbid(unsafe_code)]

pub mod errno;
pub mod fd;
pub mod kernel;
pub mod memory_bridge;
pub mod syscall;
pub mod vfs;

pub use fd::{
    DupError, Fd, FdHandle, FdKind, FdReadWriteError, FileDescriptorEntry, FileDescriptorTable,
    FileReadError, FileWriteError, SharedDevice, file_read, file_write, fstat_from_fd,
    ioctl_on_fd, mmap_from_fd, pread_from_fd, read_from_fd, write_to_fd,
};
// LinuxRuntime 类型定义在 kernel 聚合根，这里 re-export 保持对外路径兼容。
pub use kernel::{LinuxRuntime, FdOpError, MmapOutcome, WriteOutcome};
pub use memory_bridge::MemoryBridge;
pub use syscall::SyscallResult;
pub use vfs::{VfsError, VfsMountTable, VfsNode};

// errno 常量与映射对外可见（syscall 层错误码的权威来源）。
pub use errno::{
    map_fd_rw_error, map_ioctl_error, EACCES, EBADF, EFAULT, EINVAL, ENOSYS, ENOTTY,
};
