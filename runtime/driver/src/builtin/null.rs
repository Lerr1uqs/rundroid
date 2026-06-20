//! `/dev/null` 内建设备。
//!
//! 写入的数据被静默丢弃，读取始终返回 EOF。

use crate::context::{
    DeviceCloseContext, DeviceIoContext, DeviceOpenContext, DeviceStatContext, SyntheticStat,
};
use crate::device::{DeviceError, VirtualDevice};

/// `/dev/null` 设备：丢弃所有写入，读取返回 EOF。
pub struct NullDevice;

impl VirtualDevice for NullDevice {
    fn open(&mut self, _ctx: &mut DeviceOpenContext) -> Result<(), DeviceError> {
        Ok(())
    }

    fn read(&mut self, ctx: &mut DeviceIoContext, len: usize) -> Result<Vec<u8>, DeviceError> {
        // read 始终返回 EOF（空向量）。
        let _ = (ctx, len);
        Ok(Vec::new())
    }

    fn write(&mut self, ctx: &mut DeviceIoContext, data: &[u8]) -> Result<usize, DeviceError> {
        // 写入被丢弃，但返回全部长度（写入成功）。
        let _ = ctx;
        Ok(data.len())
    }

    fn fstat(&self, _ctx: &DeviceStatContext) -> Result<SyntheticStat, DeviceError> {
        Ok(SyntheticStat {
            st_mode: 0x2190, // S_IFCHR | 0666
            st_size: 0,
            st_dev: 0x0103, // makedev(1, 3)
            st_ino: 3,
        })
    }

    fn close(&mut self, _ctx: &mut DeviceCloseContext) -> Result<(), DeviceError> {
        Ok(())
    }
}

/// 创建 `/dev/null` 的工厂函数。
pub fn null_factory() -> crate::registry::DeviceFactory {
    std::sync::Arc::new(|| Box::new(NullDevice))
}
