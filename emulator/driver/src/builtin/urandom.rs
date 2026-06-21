//! `/dev/urandom` / `/dev/random` 内建设备。
//!
//! 确定性伪随机设备：使用 xorshift64 算法，确保不同 host 上结果一致。
//! `/dev/urandom` 和 `/dev/random` 行为完全一致。

use crate::context::{
    DeviceCloseContext, DeviceIoContext, DeviceIoctlContext, DeviceMmapContext, DeviceMmapRequest,
    DeviceOpenContext, DeviceStatContext, SyntheticStat,
};
use crate::device::{DeviceError, VirtualDevice};

/// 确定性 PRNG 设备。
///
/// 使用 xorshift64 算法生成伪随机字节，保证同一种子下的可复现性。
pub struct UrandomDevice {
    /// xorshift64 状态。
    rng: u64,
}

impl UrandomDevice {
    /// 用指定种子创建新设备实例。
    ///
    /// 种子为 0 时会自动替换为非零值（0 种子会让 xorshift 退化）。
    pub fn new(seed: u64) -> Self {
        Self {
            rng: if seed == 0 { 0xDEAD_BEEF } else { seed },
        }
    }

    /// xorshift64 推进并返回一个字节。
    fn next_byte(&mut self) -> u8 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x & 0xFF) as u8
    }
}

impl VirtualDevice for UrandomDevice {
    fn open(&mut self, _ctx: &mut DeviceOpenContext) -> Result<(), DeviceError> {
        Ok(())
    }

    fn read(&mut self, ctx: &mut DeviceIoContext, len: usize) -> Result<Vec<u8>, DeviceError> {
        let _ = ctx;
        let mut buf = Vec::with_capacity(len);
        for _ in 0..len {
            buf.push(self.next_byte());
        }
        Ok(buf)
    }

    fn write(&mut self, ctx: &mut DeviceIoContext, data: &[u8]) -> Result<usize, DeviceError> {
        // `/dev/urandom` 不接受写入：返回错误。
        let _ = (ctx, data);
        Err(DeviceError::NotSupported)
    }

    fn ioctl(
        &mut self,
        ctx: &mut DeviceIoctlContext,
        request: u64,
        argp: u64,
    ) -> Result<i64, DeviceError> {
        let _ = (ctx, request, argp);
        Err(DeviceError::NotSupported)
    }

    fn mmap(
        &mut self,
        ctx: &mut DeviceMmapContext,
        req: &DeviceMmapRequest,
    ) -> Result<Option<crate::context::DeviceMappedRegion>, DeviceError> {
        let _ = (ctx, req);
        Ok(None)
    }

    fn fstat(&self, _ctx: &DeviceStatContext) -> Result<SyntheticStat, DeviceError> {
        Ok(SyntheticStat {
            st_mode: 0x2190, // S_IFCHR | 0666
            st_size: 0,
            st_dev: 0x0101, // makedev(1, 1)
            st_ino: 1,
        })
    }

    fn close(&mut self, _ctx: &mut DeviceCloseContext) -> Result<(), DeviceError> {
        Ok(())
    }
}

/// 创建 `/dev/urandom` 或 `/dev/random` 的工厂函数。
///
/// `seed` 决定 PRNG 的初始状态，由 LinuxRuntime 在注册时捕获。
/// 返回的 factory 可多次调用，每次生成一个独立的 `UrandomDevice` 实例（但共享同一种子）。
pub fn urandom_factory(seed: u64) -> crate::registry::DeviceFactory {
    std::sync::Arc::new(move || Box::new(UrandomDevice::new(seed)))
}
