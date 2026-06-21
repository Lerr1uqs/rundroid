//! APK context — 为 framework 行为提供统一的 APK 元数据。
//!
//! [`ApkContext`] 收敛 package name、version、manifest、signatures、assets 等
//! 所有与 APK 绑定的信息，framework stub 通过此结构取值，
//! 不允许散落在多个 helper 中的隐式状态。
//!
//! # 当前阶段
//!
//! 提供字段建模和基础构造，不包含：
//! - 真实的 AndroidManifest.xml 解析
//! - 签名校验算法
//! - assets 文件系统的实际 IO

// ============================================================================
// ApkContext
// ============================================================================

/// 统一 APK context。
///
/// framework stub（如 `PackageManager`、`ActivityThread`）通过此结构
/// 获取 package/signature/asset 数据，不再通过散落的隐式状态取值。
#[derive(Debug, Clone)]
pub struct ApkContext {
    /// APK 包名（如 `"com.example.app"`）。
    pub package_name: String,
    /// 版本名（如 `"1.0.0"`），可选。
    pub version_name: Option<String>,
    /// 版本号（`android:versionCode`），可选。
    pub version_code: Option<i32>,
    /// 原始 `AndroidManifest.xml` 字节（可后续解析），可选。
    pub manifest: Option<Vec<u8>>,
    /// 签名数据列表（每个签名是原始字节）。
    pub signatures: Vec<SignatureData>,
    /// assets 列表（文件名）。实际内容通过 `AssetProvider` trait 获取。
    pub asset_names: Vec<String>,
}

/// 签名数据。
#[derive(Debug, Clone)]
pub struct SignatureData {
    /// 签名的原始字节。
    pub bytes: Vec<u8>,
}

impl ApkContext {
    /// 创建最小 APK context（仅包名）。
    pub fn new(package_name: String) -> Self {
        Self {
            package_name,
            version_name: None,
            version_code: None,
            manifest: None,
            signatures: Vec::new(),
            asset_names: Vec::new(),
        }
    }

    /// 设置版本信息。
    pub fn with_version(mut self, name: Option<String>, code: Option<i32>) -> Self {
        self.version_name = name;
        self.version_code = code;
        self
    }

    /// 添加一条签名数据。
    pub fn with_signature(mut self, bytes: Vec<u8>) -> Self {
        self.signatures.push(SignatureData { bytes });
        self
    }

    /// 设置 manifest 字节。
    pub fn with_manifest(mut self, manifest: Vec<u8>) -> Self {
        self.manifest = Some(manifest);
        self
    }

    /// 添加 asset 文件名。
    pub fn with_asset(mut self, name: String) -> Self {
        self.asset_names.push(name);
        self
    }
}

impl SignatureData {
    /// 从原始字节创建签名数据。
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    /// 签名字节长度。
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// 签名是否为空。
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_apk_context() {
        let ctx = ApkContext::new("com.example.app".into());
        assert_eq!(ctx.package_name, "com.example.app");
        assert!(ctx.version_name.is_none());
        assert!(ctx.version_code.is_none());
        assert!(ctx.manifest.is_none());
        assert!(ctx.signatures.is_empty());
        assert!(ctx.asset_names.is_empty());
    }

    #[test]
    fn apk_context_with_version() {
        let ctx = ApkContext::new("com.example.app".into())
            .with_version(Some("1.2.3".into()), Some(42));
        assert_eq!(ctx.version_name, Some("1.2.3".into()));
        assert_eq!(ctx.version_code, Some(42));
    }

    #[test]
    fn apk_context_with_signatures() {
        let sig_bytes = vec![0x01, 0x02, 0x03, 0x04];
        let ctx = ApkContext::new("com.test".into())
            .with_signature(sig_bytes.clone());

        assert_eq!(ctx.signatures.len(), 1);
        assert_eq!(ctx.signatures[0].bytes, sig_bytes);
    }

    #[test]
    fn apk_context_with_multiple_signatures() {
        let ctx = ApkContext::new("com.test".into())
            .with_signature(vec![1, 2, 3])
            .with_signature(vec![4, 5, 6]);

        assert_eq!(ctx.signatures.len(), 2);
    }

    #[test]
    fn apk_context_with_manifest() {
        let manifest = vec![0xAA, 0xBB, 0xCC];
        let ctx = ApkContext::new("com.test".into())
            .with_manifest(manifest.clone());

        assert_eq!(ctx.manifest, Some(manifest));
    }

    #[test]
    fn apk_context_with_assets() {
        let ctx = ApkContext::new("com.test".into())
            .with_asset("foo/bar.txt".into())
            .with_asset("baz/qux.dat".into());

        assert_eq!(ctx.asset_names.len(), 2);
        assert!(ctx.asset_names.contains(&"foo/bar.txt".to_string()));
        assert!(ctx.asset_names.contains(&"baz/qux.dat".to_string()));
    }

    #[test]
    fn signature_data_utilities() {
        let sig = SignatureData::new(vec![1, 2, 3]);
        assert_eq!(sig.len(), 3);
        assert!(!sig.is_empty());

        let empty_sig = SignatureData::new(vec![]);
        assert_eq!(empty_sig.len(), 0);
        assert!(empty_sig.is_empty());
    }

    #[test]
    fn full_apk_context_chained() {
        let ctx = ApkContext::new("com.example.full".into())
            .with_version(Some("2.0.0".into()), Some(100))
            .with_manifest(vec![0xDE, 0xAD, 0xBE, 0xEF])
            .with_signature(vec![0xCA, 0xFE])
            .with_asset("assets/icon.png".into())
            .with_asset("assets/config.json".into());

        assert_eq!(ctx.package_name, "com.example.full");
        assert_eq!(ctx.version_name, Some("2.0.0".into()));
        assert_eq!(ctx.version_code, Some(100));
        assert!(ctx.manifest.is_some());
        assert_eq!(ctx.signatures.len(), 1);
        assert_eq!(ctx.asset_names.len(), 2);
    }
}
