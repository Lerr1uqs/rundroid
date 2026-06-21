//! `/dev/zero` 内建设备。
//!
//! 读取返回全零字节，写入被丢弃。

use crate::context::{
    DeviceCloseContext, DeviceIoContext, DeviceOpenContext, DeviceStatContext, SyntheticStat,
};
use crate::device::{DeviceError, VirtualDevice};

/// `/dev/zero` 设备：读取返回零字节，写入丢弃。
pub struct ZeroDevice;

impl VirtualDevice for ZeroDevice {
    fn open(&mut self, _ctx: &mut DeviceOpenContext) -> Result<(), DeviceError> {
        Ok(())
    }

    fn read(&mut self, ctx: &mut DeviceIoContext, len: usize) -> Result<Vec<u8>, DeviceError> {
        let _ = ctx;
        Ok(vec![0u8; len])
    }

    fn write(&mut self, ctx: &mut DeviceIoContext, data: &[u8]) -> Result<usize, DeviceError> {
        let _ = ctx;
        Ok(data.len())
    }

    fn fstat(&self, _ctx: &DeviceStatContext) -> Result<SyntheticStat, DeviceError> {
        Ok(SyntheticStat {
            st_mode: 0x2190, // S_IFCHR | 0666
            st_size: 0,
            st_dev: 0x0105, // makedev(1, 5)
            st_ino: 5,
        })
    }

    fn close(&mut self, _ctx: &mut DeviceCloseContext) -> Result<(), DeviceError> {
        Ok(())
    }
}

/// 创建 `/dev/zero` 的工厂函数。
pub fn zero_factory() -> crate::registry::DeviceFactory {
    std::sync::Arc::new(|| Box::new(ZeroDevice))
}
