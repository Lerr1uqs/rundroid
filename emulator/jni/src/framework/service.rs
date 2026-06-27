//! Service registry —— `getSystemService` 风格行为的统一收敛点。
//!
//! 真实 Android 中 `Context.getSystemService(name)` 按 name 返回对应的系统 service
//! （TelephonyManager / WifiManager / SensorManager …）。本 registry 把这种行为
//! 从散落的 switch 收敛成「name → service stub ObjectId」的映射。
//!
//! # 与 VM authority 的关系
//!
//! `ServiceRegistry` 自身只持有 service name → stub ObjectId 的映射，
//! **不**持有 service stub 的对象数据——对象数据仍在 VM 的 `ObjectStore` 中。
//! `getSystemService` 的 handler 在 dispatch 时查此 registry 拿到 stub oid 返回。

use crate::types::ObjectId;
use std::collections::HashMap;

// ============================================================================
// ServiceEntry / ServiceRegistry
// ============================================================================

/// 一条 service 注册项。
#[derive(Debug, Clone, Copy)]
pub struct ServiceEntry {
    /// service 名称（如 `"phone"`、`"wifi"`）。
    pub name: &'static str,
    /// service stub 对象的 ObjectId。
    pub stub_oid: ObjectId,
}

/// service 注册表——`getSystemService` 查询入口。
#[derive(Debug, Default)]
pub struct ServiceRegistry {
    services: HashMap<String, ObjectId>,
}

impl ServiceRegistry {
    /// 创建空 registry。
    pub fn new() -> Self {
        Self { services: HashMap::new() }
    }

    /// 注册一个 service（name → stub oid）。重复 name 覆盖旧值（最后一次注册生效）。
    pub fn register(&mut self, name: &str, stub_oid: ObjectId) {
        self.services.insert(name.to_string(), stub_oid);
    }

    /// 按 name 查询 service stub oid。未注册返回 None。
    ///
    /// 真实 Android 中未知的 service name 返回 null，这里以 None 表达，
    /// 由调用方决定如何映射成 `JValue::Null` 或错误。
    pub fn lookup(&self, name: &str) -> Option<ObjectId> {
        self.services.get(name).copied()
    }

    /// 是否已注册某个 service。
    pub fn contains(&self, name: &str) -> bool {
        self.services.contains_key(name)
    }

    /// 已注册的 service 数量。
    pub fn len(&self) -> usize {
        self.services.len()
    }

    /// registry 是否为空。
    pub fn is_empty(&self) -> bool {
        self.services.is_empty()
    }

    /// 已注册的全部 service name（用于调试/telemetry）。
    pub fn names(&self) -> Vec<&str> {
        self.services.keys().map(|s| s.as_str()).collect()
    }
}

// ============================================================================
// 默认 service 集合
// ============================================================================

/// `getSystemService` 接受的最小 service name 集合（tasks.md 要求）。
///
/// 这些 name 是 Android `Context` 上标准的 service key：
/// phone / wifi / connectivity / sensor / activity / window / audio。
pub const DEFAULT_SERVICE_NAMES: &[&str] = &[
    "phone",
    "wifi",
    "connectivity",
    "sensor",
    "activity",
    "window",
    "audio",
];

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_lookup() {
        let mut reg = ServiceRegistry::new();
        reg.register("phone", ObjectId(10));
        assert_eq!(reg.lookup("phone"), Some(ObjectId(10)));
        assert!(reg.contains("phone"));
        assert!(!reg.contains("wifi"));
    }

    #[test]
    fn lookup_unknown_returns_none() {
        let reg = ServiceRegistry::new();
        assert_eq!(reg.lookup("nope"), None);
    }

    #[test]
    fn register_overrides() {
        let mut reg = ServiceRegistry::new();
        reg.register("wifi", ObjectId(1));
        reg.register("wifi", ObjectId(2));
        assert_eq!(reg.lookup("wifi"), Some(ObjectId(2)), "重复注册以最后一次为准");
    }

    #[test]
    fn names_and_len() {
        let mut reg = ServiceRegistry::new();
        assert!(reg.is_empty());
        reg.register("phone", ObjectId(1));
        reg.register("wifi", ObjectId(2));
        assert_eq!(reg.len(), 2);
        let mut names = reg.names();
        names.sort();
        assert_eq!(names, vec!["phone", "wifi"]);
    }

    #[test]
    fn default_service_names_covers_required_set() {
        // tasks.md 明确要求的最小集合都必须在默认表里
        for required in ["phone", "wifi", "connectivity", "sensor", "activity", "window", "audio"] {
            assert!(
                DEFAULT_SERVICE_NAMES.contains(&required),
                "默认 service 集合缺少 `{required}`"
            );
        }
    }
}
