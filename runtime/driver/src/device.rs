//! 虚拟设备 trait。
//!
//! 所有设备（builtin 或后续 custom）都必须实现 [`VirtualDevice`]。
//! 设备只描述自身行为，不负责目标侧内存回写——回写由 syscall 层在拿到返回数据后执行。

use crate::context::{
    DeviceCloseContext, DeviceIoContext, DeviceIoctlContext, DeviceMmapContext, DeviceMmapRequest,
    DeviceMappedRegion, DeviceOpenContext, DeviceStatContext, SyntheticStat,
};
use thiserror::Error;

/// 设备操作错误。
///
/// 每个变体对应一个 POSIX errno 语义；syscall 层据此决定返回给 guest 的负值。
/// 设备实现不应直接把 errno 数值写进返回值——那属于 syscall 层的职责。
#[derive(Debug, Error)]
pub enum DeviceError {
    /// 不支持的操作（ENOTTY / ENODEV）。
    #[error("operation not supported")]
    NotSupported,
    /// 无效参数（EINVAL）。
    #[error("invalid argument")]
    InvalidArgument,
    /// 缓冲长度不足以完成 ioctl（ENOSPC）。
    #[error("buffer too small for ioctl response")]
    BufferTooSmall,
    /// 设备内部错误。
    #[error("internal device error: {0}")]
    Internal(String),
}

/// 虚拟设备接口。
///
/// 设备定义（builtin 实现或 custom 实现）不直接持有 backend / engine。
/// 操作返回值由 syscall 层解释并回写到目标侧内存。
///
/// # 生命周期
///
/// - factory / class 是设备定义
/// - 实现本 trait 的实例是 per-open 的会话状态
/// - syscall 层通过 [`DeviceRegistry`](crate::registry::DeviceRegistry) 查找 factory，
///   每次 open 创建一个新实例
pub trait VirtualDevice: Send {
    /// 设备被 open 时调用一次。
    ///
    /// 可用于初始化 per-fd 状态。
    /// 无状态设备可以返回 `Ok(())`。
    fn open(&mut self, ctx: &mut DeviceOpenContext) -> Result<(), DeviceError>;

    /// 从设备读取最多 `len` 字节。
    ///
    /// 返回的字节由 syscall 层负责回写到目标侧内存。
    /// 返回空 `Vec` 表示 EOF。
    fn read(&mut self, ctx: &mut DeviceIoContext, len: usize) -> Result<Vec<u8>, DeviceError>;

    /// 向设备写入数据。
    ///
    /// 返回实际写入的字节数。
    /// 对于 `/dev/null` 之类会丢弃所有写入的设备，返回 `data.len()` 但不存储。
    fn write(&mut self, ctx: &mut DeviceIoContext, data: &[u8]) -> Result<usize, DeviceError>;

    /// 执行 ioctl 请求。
    ///
    /// `request` 是 ioctl 命令号，`argp` 是目标侧参数指针。
    /// 返回值写入目标侧 x0（i64 语义）。
    ///
    /// 默认返回 `NotSupported`。
    fn ioctl(
        &mut self,
        ctx: &mut DeviceIoctlContext,
        request: u64,
        argp: u64,
    ) -> Result<i64, DeviceError> {
        let _ = (ctx, request, argp);
        Err(DeviceError::NotSupported)
    }

    /// 请求 mmap 映射描述。
    ///
    /// 设备返回一个 [`DeviceMappedRegion`]，由 syscall 层负责在目标侧建立真实映射。
    /// 不支持 mmap 的设备返回 `Ok(None)`。
    ///
    /// 默认返回 `None`（不支持 mmap）。
    fn mmap(
        &mut self,
        ctx: &mut DeviceMmapContext,
        req: &DeviceMmapRequest,
    ) -> Result<Option<DeviceMappedRegion>, DeviceError> {
        let _ = (ctx, req);
        Ok(None)
    }

    /// 返回合成 stat 信息。
    ///
    /// 默认返回一个字符设备 stat（st_mode = S_IFCHR | 0666）。
    fn fstat(&self, ctx: &DeviceStatContext) -> Result<SyntheticStat, DeviceError> {
        let _ = ctx;
        Ok(SyntheticStat {
            st_mode: 0x2190, // S_IFCHR | 0666
            st_size: 0,
            st_dev: 0,
            st_ino: 0,
        })
    }

    /// 设备被 close 时调用。
    ///
    /// 可用于释放 per-fd 资源。
    /// 注意：close 后仍可能存在 dup 创建的共享 handle。
    fn close(&mut self, ctx: &mut DeviceCloseContext) -> Result<(), DeviceError> {
        let _ = ctx;
        Ok(())
    }
}
