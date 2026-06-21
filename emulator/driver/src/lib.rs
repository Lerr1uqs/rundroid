//! `rundroid-driver`
//!
//! 虚拟设备抽象层。提供：
//! - [`VirtualDevice`] trait：设备行为接口
//! - [`DeviceRegistry`]：设备工厂注册表
//! - [`VirtFileSource`]：普通文件挂载来源（宿主文件/内存字节/动态 provider）
//! - [`builtin`]：内建设备实现（urandom / null / zero）
//!
//! 本 crate 不持有 backend / engine / syscall 语义；
//! 所有目标侧回写与错误传播由上层 OS 层在拿到本 crate 的输出后执行。

pub mod builtin;
pub mod context;
pub mod device;
pub mod mapper;
pub mod registry;

pub use context::{
    DeviceCloseContext, DeviceIoContext, DeviceIoctlContext, DeviceMmapContext, DeviceMmapRequest,
    DeviceOpenContext, DeviceStatContext,
};
pub use device::{DeviceError, VirtualDevice};
pub use mapper::{VirtFileSource, VirtFileProvider};
pub use registry::{DeviceFactory, DeviceMountId, DeviceRegistry};
