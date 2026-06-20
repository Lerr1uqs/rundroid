//! `rundroid-linux`
//!
//! Linux 用户态运行时最小集：覆盖 bootstrap smoke case 所需的
//! 基础 fd 表 / VFS / syscall 表面。
//!
//! 实现策略：
//! - [`fd`] 维护一个 host 端的 fd 表（in-memory），把 guest 的"整数 fd"映射到本层句柄
//! - [`vfs`] 提供 `/dev/urandom` 等少量确定性"虚拟文件"
//! - [`syscall`] 把 ARM64 syscall 编号翻译成对 fd/vfs 的调用
//!
//! bootstrap 不追求完整 syscall 覆盖；只覆盖：
//! openat / close / read / write / mmap / munmap / exit_group / brk / getrandom

#![forbid(unsafe_code)]

pub mod fd;
pub mod syscall;
pub mod vfs;

pub use fd::{Fd, FdTable, FdType};
pub use syscall::{LinuxRuntime, SyscallResult};
pub use vfs::{VfsSource, resolve};
