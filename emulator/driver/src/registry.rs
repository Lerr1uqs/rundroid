//! 设备注册表。
//!
//! [`DeviceRegistry`] 维护"虚拟路径 → 设备工厂"的映射。
//! 每次 `open` 系统调用在 VFS 解析到 device node 后，
//! 从 registry 取出 factory 创建 per-open device instance。

use crate::device::{DeviceError, VirtualDevice};
use std::collections::HashMap;
use std::sync::Arc;

/// 设备工厂：无参闭包，每次调用生成一个新的 device instance。
///
/// Factory 本身不负责挂载——挂载由 VFS mount table 负责。
/// Registry 只保存已挂载路径对应的 factory。
pub type DeviceFactory = Arc<dyn Fn() -> Box<dyn VirtualDevice> + Send + Sync>;

/// 设备挂载 ID。用于在 VFS 节点中引用 registry 中的 factory。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceMountId(pub u64);

/// 设备注册表。
///
/// # 职责
///
/// - 保存已挂载设备的 factory 引用
/// - 按 VFS 解析到的 [`DeviceMountId`] 创建 device instance
///
/// # 不负责
///
/// - 路径解析（由 VFS 层负责）
/// - 路径冲突检测（由 VFS mount table 负责）
pub struct DeviceRegistry {
    /// mount_id → factory 的映射。
    factories: HashMap<DeviceMountId, DeviceFactory>,
    /// 下一个挂载 ID 的计数器。
    next_id: u64,
}

impl DeviceRegistry {
    /// 创建一个空注册表。
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
            next_id: 1,
        }
    }

    /// 注册一个设备工厂，返回对应的 [`DeviceMountId`]。
    ///
    /// 本方法不检查路径冲突——由 VFS 层在 mount 前检查。
    pub fn register(&mut self, factory: DeviceFactory) -> DeviceMountId {
        let id = DeviceMountId(self.next_id);
        self.next_id += 1;
        self.factories.insert(id, factory);
        id
    }

    /// 按 mount_id 创建新的 device instance。
    ///
    /// 返回的 device 是 per-open 实例，状态独立。
    pub fn create_instance(
        &self,
        mount_id: DeviceMountId,
    ) -> Result<Box<dyn VirtualDevice>, DeviceError> {
        let factory = self
            .factories
            .get(&mount_id)
            .ok_or(DeviceError::Internal("mount_id not found in registry".into()))?;
        Ok(factory())
    }
}

impl Default for DeviceRegistry {
    fn default() -> Self {
        Self::new()
    }
}
