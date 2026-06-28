//! Linux errno 常量与底座错误到 errno 的集中映射。
//!
//! 把原本散落在各 syscall handler 内的 `Err(_) => EINVAL/EBADF/...` 规则
//! 收敛到此模块，避免错误码漂移。syscall 层仍负责把
//! [`crate::memory_bridge::MemoryBridge`] 回写失败强制编码为 [`EFAULT`]，
//! 其余 errno 选择经由本模块的常量或映射函数完成。

use crate::fd::FdReadWriteError;

// ============================================================================
// errno 常量（POSIX 风格负数）
// ============================================================================

/// 函数未实现（`ENOSYS`）。
pub const ENOSYS: i64 = -38;
/// 错误的文件描述符（`EBADF`）。
pub const EBADF: i64 = -9;
/// 地址错误／目标侧缓冲未映射（`EFAULT`）。
pub const EFAULT: i64 = -14;
/// 非法参数（`EINVAL`）。
pub const EINVAL: i64 = -22;
/// 不适合该操作的不相关设备／操作不支持（`ENOTTY`）。
pub const ENOTTY: i64 = -25;
/// 权限不足（`EACCES`）。
pub const EACCES: i64 = -13;

// ============================================================================
// 底座错误 → errno 映射
// ============================================================================

/// read / pread64 / write 共用的底座错误→errno 映射。
///
/// - [`FdReadWriteError::NotSupported`] → [`ENOTTY`]：设备/文件不支持该 IO 操作。
/// - [`FdReadWriteError::Internal`] → [`EFAULT`]：底层 IO 出错，视同目标侧不可达。
///
/// 这三个 handler 共享同一规则，集中在此避免重复散落。
pub fn map_fd_rw_error(err: FdReadWriteError) -> i64 {
    match err {
        FdReadWriteError::NotSupported => ENOTTY,
        FdReadWriteError::Internal(_) => EFAULT,
    }
}

/// ioctl 专用的底座错误→errno 映射。
///
/// 与 [`map_fd_rw_error`] 的唯一差别：`Internal` 归为 [`EINVAL`]（参数非法），
/// 而非 IO 路径的 [`EFAULT`]——ioctl 的内部错误更贴近 "request 号/argp 非法" 语义。
pub fn map_ioctl_error(err: FdReadWriteError) -> i64 {
    match err {
        FdReadWriteError::NotSupported => ENOTTY,
        FdReadWriteError::Internal(_) => EINVAL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fd_rw_error_maps_not_supported_to_enotty_and_internal_to_efault() {
        assert_eq!(map_fd_rw_error(FdReadWriteError::NotSupported), ENOTTY);
        assert_eq!(
            map_fd_rw_error(FdReadWriteError::Internal("io".into())),
            EFAULT
        );
    }

    #[test]
    fn ioctl_error_maps_internal_to_einval() {
        assert_eq!(map_ioctl_error(FdReadWriteError::NotSupported), ENOTTY);
        assert_eq!(
            map_ioctl_error(FdReadWriteError::Internal("bad req".into())),
            EINVAL
        );
    }

    /// errno 常量为负数（POSIX 风格），避免回归成正数。
    #[test]
    fn errno_constants_are_negative() {
        for c in [ENOSYS, EBADF, EFAULT, EINVAL, ENOTTY, EACCES] {
            assert!(c < 0, "errno constant {c} should be negative");
        }
    }
}
