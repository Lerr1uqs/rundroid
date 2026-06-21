//! 资源 URI 解析。
//!
//! URI 格式：`resource:<pack>/<path>`，例如 `resource:smoke/libfoo.so`。
//!
//! 解析顺序（design.md "Mitigations" 要求 case 强制使用 resource URI）：
//! 1. 把 `<pack>` 映射到仓库内的 `resources/<pack>/` 目录
//! 2. 在该目录下查 `<path>`，返回字节流
//!
//! bootstrap 不支持绝对路径 / file: URI，强制所有 case 可移植。

use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ResourceError {
    #[error("not a resource URI: {0}")]
    NotAResourceUri(String),
    #[error("resource pack `{pack}` not found under resources/")]
    UnknownPack { pack: String },
    #[error("resource path `{path}` not found in pack `{pack}`")]
    NotFound { pack: String, path: String },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// 解析 resource URI 并读取字节。
///
/// `resources_root` 通常是仓库根的 `resources/` 目录绝对路径。
pub fn resolve_resource(
    uri: &str,
    resources_root: &std::path::Path,
) -> Result<Vec<u8>, ResourceError> {
    let rest = uri
        .strip_prefix("resource:")
        .ok_or_else(|| ResourceError::NotAResourceUri(uri.to_string()))?;
    let (pack, path) = rest
        .split_once('/')
        .ok_or_else(|| ResourceError::NotAResourceUri(uri.to_string()))?;
    if pack.is_empty() || path.is_empty() {
        return Err(ResourceError::NotAResourceUri(uri.to_string()));
    }

    let pack_dir: PathBuf = resources_root.join(pack);
    if !pack_dir.is_dir() {
        return Err(ResourceError::UnknownPack {
            pack: pack.to_string(),
        });
    }
    let full = pack_dir.join(path);
    if !full.is_file() {
        return Err(ResourceError::NotFound {
            pack: pack.to_string(),
            path: path.to_string(),
        });
    }
    Ok(std::fs::read(full)?)
}

/// 解析 resource URI 但不读字节，仅返回本地路径（供 backend 等需要路径的场景）。
pub fn resolve_path(
    uri: &str,
    resources_root: &std::path::Path,
) -> Result<PathBuf, ResourceError> {
    let rest = uri
        .strip_prefix("resource:")
        .ok_or_else(|| ResourceError::NotAResourceUri(uri.to_string()))?;
    let (pack, path) = rest
        .split_once('/')
        .ok_or_else(|| ResourceError::NotAResourceUri(uri.to_string()))?;
    let full = resources_root.join(pack).join(path);
    if !full.is_file() {
        return Err(ResourceError::NotFound {
            pack: pack.to_string(),
            path: path.to_string(),
        });
    }
    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_resource_uri() {
        let err = resolve_resource("/abs/path", std::path::Path::new(".")).unwrap_err();
        assert!(matches!(err, ResourceError::NotAResourceUri(_)));
    }
}
