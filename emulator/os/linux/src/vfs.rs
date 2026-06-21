//! 虚拟文件系统挂载表。
//!
//! [`VfsMountTable`] 是路径 → 文件节点/设备节点的映射表。
//! 采用显式挂载模型（类似 qiling 风格）而不引入完整 rootfs。
//!
//! # 两条挂载主线
//!
//! - `mount_file(path, source)`：挂载普通文件节点
//! - `mount_device(path, mount_id)`：挂载设备节点
//!
//! # 冲突规则
//!
//! 同一虚拟路径不允许重复挂载（file 与 file、device 与 device、
//! file 与 device 之间都不允许）。冲突时立即返回错误，不允许静默覆盖。

use rundroid_driver::mapper::VirtFileSource;
use rundroid_driver::registry::DeviceMountId;
use std::collections::HashMap;
use thiserror::Error;

/// VFS 挂载表，保存所有已挂载的虚拟路径。
pub struct VfsMountTable {
    mounts: HashMap<String, VfsNode>,
}

/// VFS 节点：表示一个虚拟路径在挂载后指向什么。
#[derive(Debug, Clone)]
pub enum VfsNode {
    /// 普通文件节点（宿主文件/内存字节/动态 provider）。
    File(VirtFileSource),
    /// 设备节点（mount_id 指向 DeviceRegistry 中的 factory）。
    Device(DeviceMountId),
}

/// VFS 操作错误。
#[derive(Debug, Error)]
pub enum VfsError {
    /// 虚拟路径上已有挂载，不能重复注册。
    #[error("virtual path `{0}` already mounted")]
    AlreadyMounted(String),
    /// 挂载处理过程中的内部错误。
    #[error("vfs internal error: {0}")]
    Internal(String),
}

impl VfsMountTable {
    /// 创建空的挂载表。
    pub fn new() -> Self {
        Self {
            mounts: HashMap::new(),
        }
    }

    /// 挂载一个普通文件节点。
    ///
    /// 如果目标路径已有任何挂载（file 或 device），立即返回 `AlreadyMounted`。
    pub fn mount_file(
        &mut self,
        virtual_path: &str,
        source: VirtFileSource,
    ) -> Result<(), VfsError> {
        if self.mounts.contains_key(virtual_path) {
            return Err(VfsError::AlreadyMounted(virtual_path.to_string()));
        }
        self.mounts
            .insert(virtual_path.to_string(), VfsNode::File(source));
        Ok(())
    }

    /// 挂载一个设备节点。
    ///
    /// `mount_id` 是从 [`DeviceRegistry`] 注册 factory 后拿到的 ID。
    /// 如果目标路径已有任何挂载，立即返回 `AlreadyMounted`。
    pub fn mount_device(
        &mut self,
        virtual_path: &str,
        mount_id: DeviceMountId,
    ) -> Result<(), VfsError> {
        if self.mounts.contains_key(virtual_path) {
            return Err(VfsError::AlreadyMounted(virtual_path.to_string()));
        }
        self.mounts
            .insert(virtual_path.to_string(), VfsNode::Device(mount_id));
        Ok(())
    }

    /// 解析虚拟路径，返回对应的节点（如果存在）。
    ///
    /// 路径必须精确匹配；当前阶段不做前缀匹配 / 挂载点归一化。
    pub fn resolve(&self, path: &str) -> Option<&VfsNode> {
        self.mounts.get(path)
    }

    /// 返回已挂载的路径数量。
    pub fn len(&self) -> usize {
        self.mounts.len()
    }

    /// 挂载表是否为空。
    pub fn is_empty(&self) -> bool {
        self.mounts.is_empty()
    }
}

impl Default for VfsMountTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_file_and_resolve() {
        let mut vfs = VfsMountTable::new();
        let source = VirtFileSource::Bytes(b"hello".to_vec());
        vfs.mount_file("/test/hello.txt", source).unwrap();
        let node = vfs.resolve("/test/hello.txt").unwrap();
        assert!(matches!(node, VfsNode::File(_)));
    }

    #[test]
    fn duplicate_path_returns_error() {
        let mut vfs = VfsMountTable::new();
        let source1 = VirtFileSource::Bytes(b"one".to_vec());
        vfs.mount_file("/dup", source1).unwrap();

        // 同一路径重复 file mount。
        let source2 = VirtFileSource::Bytes(b"two".to_vec());
        let err = vfs.mount_file("/dup", source2).unwrap_err();
        match err {
            VfsError::AlreadyMounted(p) => assert_eq!(p, "/dup"),
            _ => panic!("expected AlreadyMounted"),
        }
    }

    #[test]
    fn file_and_device_path_conflict() {
        let mut vfs = VfsMountTable::new();
        vfs
            .mount_file("/conflict", VirtFileSource::Bytes(b"data".to_vec()))
            .unwrap();

        // 同一路径再 mount device。
        let err = vfs
            .mount_device("/conflict", DeviceMountId(42))
            .unwrap_err();
        match err {
            VfsError::AlreadyMounted(p) => assert_eq!(p, "/conflict"),
            _ => panic!("expected AlreadyMounted"),
        }
    }

    #[test]
    fn unmapped_path_returns_none() {
        let vfs = VfsMountTable::new();
        assert!(vfs.resolve("/nonexistent").is_none());
    }
}
