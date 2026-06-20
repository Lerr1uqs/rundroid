//! 普通文件挂载来源。
//!
//! [`VirtFileSource`] 统一表达三种文件数据来源：
//! - 宿主文件路径
//! - 内存字节
//! - 动态 provider
//!
//! 这些来源在 open/read/pread 时共享同一处理主线：
//! 先取字节，再由 syscall 层回写到目标侧。

use std::path::PathBuf;
use std::sync::Arc;

/// 普通文件的数据来源。
///
/// 所有变体在 read 时行为一致：取出字节，交给 syscall 层回写。
/// 差异仅在"字节从哪里来"。
///
/// # Clone 语义
///
/// - `HostPath` / `Bytes`：深克隆
/// - `Dynamic`：Arc 浅克隆（共享同一个 provider）
#[derive(Debug, Clone)]
pub enum VirtFileSource {
    /// 宿主文件来源。open 时尝试打开宿主文件，read 时从宿主文件读取。
    HostPath(PathBuf),
    /// 内存字节来源。read 时直接返回切片。
    Bytes(Vec<u8>),
    /// 动态文件 provider。read 时调用 provider 生成字节。
    /// 使用 Arc 共享：多次 open 或 dup 共享同一个 provider 实例。
    Dynamic(Arc<dyn VirtFileProvider>),
}

/// 动态 regular file 的字节提供者。
///
/// 用于 `/proc/self/maps` 等"内容在运行时决定"的文件。
/// provider 返回的字节仍由 syscall 层回写到目标侧。
///
/// 需要 `Send + Sync` 因为可能在多线程环境中通过 `Arc` 共享。
pub trait VirtFileProvider: Send + Sync + std::fmt::Debug {
    /// 读取整个文件内容的字节。
    fn bytes(&self) -> Result<Vec<u8>, FileProviderError>;
    /// 读取指定偏移和长度的字节片段。
    fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, FileProviderError> {
        let all = self.bytes()?;
        let start = offset as usize;
        if start >= all.len() {
            return Ok(Vec::new());
        }
        let end = (start + len).min(all.len());
        Ok(all[start..end].to_vec())
    }
}

/// 动态文件 provider 的错误类型。
#[derive(Debug, Clone)]
pub struct FileProviderError {
    pub message: String,
}

impl std::fmt::Display for FileProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "file provider error: {}", self.message)
    }
}

impl std::error::Error for FileProviderError {}
