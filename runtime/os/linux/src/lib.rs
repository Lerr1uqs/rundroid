//! `rundroid-linux`
//!
//! Linux 用户态运行时：负责 syscall 分发、fd 生命周期、
//! VFS 虚拟路径挂载和设备注册。
//!
//! # 体系结构
//!
//! - [`syscall`]：ARM64 syscall 分发器（`LinuxRuntime`）
//! - [`vfs`]：虚拟文件系统挂载表
//! - [`file_descriptor_table`]：fd 句柄表，统一管理所有 fd 来源
//! - [`file_descriptor_entry`]：单个 fd 槽位，持有 FdHandle 引用
//!
//! 设备行为定义在 `rundroid-driver` crate 中；
//! 本 crate 负责把设备操作结果落地到目标侧内存。

#![forbid(unsafe_code)]

pub mod fd;
pub mod syscall;
pub mod vfs;

pub use fd::{
    FdHandle, FdKind, FileDescriptorEntry,
    file_read, file_write, FileReadError, FileWriteError,
    read_from_fd, write_to_fd, DupError, Fd, FdReadWriteError, FileDescriptorTable,
};
pub use syscall::{LinuxRuntime, SyscallResult, ENOSYS, EBADF, EFAULT};
pub use vfs::{VfsError, VfsMountTable, VfsNode};
