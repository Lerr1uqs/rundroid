//! 虚拟文件系统。
//!
//! bootstrap 阶段只覆盖 smoke case 用到的少量"设备文件"：
//! - `/dev/urandom`：返回确定性伪随机字节（用 PRNG，保证 case 可 replay）
//! - `/dev/null`：吸收所有 write、read 返回 EOF
//!
//! 真正的文件系统挂载、rootfs、proc 等都留给后续 task。

/// 虚拟文件句柄类型。
#[derive(Debug, Clone, Copy)]
pub enum VfsSource {
    /// 确定性 urandom。字节由 [`LinuxRuntime`](crate::syscall::LinuxRuntime) 持有的 PRNG 产生。
    Urandom,
    /// `/dev/null`。
    Null,
}

/// 文件路径 → VfsSource 解析。
///
/// 路径必须精确匹配；bootstrap 不做前缀匹配 / 挂载点解析。
pub fn resolve(path: &str) -> Option<VfsSource> {
    match path {
        "/dev/urandom" | "/dev/random" => Some(VfsSource::Urandom),
        "/dev/null" => Some(VfsSource::Null),
        _ => None,
    }
}
