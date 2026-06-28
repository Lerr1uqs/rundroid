//! Framework registry —— Android framework stub 的 builtin source catalog + 装配入口。
//!
//! [`FrameworkRegistry`] 持有：
//! - builtin class spec catalog（声明式，物化后注册进 `JniRegistry`）
//! - `ServiceRegistry`（`getSystemService` 的运行时映射）
//! - APK context 共享句柄（package/signature stub 的数据源）
//!
//! # 收敛主线（关键）
//!
//! `install()` 把每个 builtin spec 物化成 `JClassDef`，经 `register_or_merge_class`
//! 写入 `AndroidVM` 持有的 `JniRegistry`——这是**唯一 runtime authority**。
//! Python shim override 走同一条 `register_or_merge_class`，与 Rust builtin 共享
//! 同一套 `JClassDef`/method/field 数据模型。`FrameworkRegistry.classes` 仅供
//! catalog 自省/重装，**不**参与 dispatch。
//!
//! # APK 依赖
//!
//! package/signature 相关 stub 优先读 `ApkContext`；APK 未提供时走 mock 数据路径
//! （见 `catalog::MOCK_PACKAGE_NAME`），保证无 APK 也能运行。

use crate::android_vm::AndroidVM;
use crate::apk_context::ApkContext;
use crate::error::JniError;
use crate::framework::catalog::{self, ApplicationInfoData};
use crate::framework::context::FrameworkCtx;
use crate::framework::service::{ServiceRegistry, DEFAULT_SERVICE_NAMES};
use crate::framework::spec::FrameworkClassSpec;
use crate::object_store::ObjectStorage;
use crate::types::ObjectId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

// ============================================================================
// 默认 service → manager class 名映射
// ============================================================================

/// 默认 service name 对应的 manager class 名（用于构造 service stub 的 class_name 元数据）。
///
/// 这些 manager class 本身**不**作为 builtin spec 注册（不在 design.md 初始 class 集），
/// 仅作为 service stub 的类型标签。guest 若需其行为，由后续 change 或 Python shim 补齐。
const DEFAULT_SERVICE_CLASSES: &[(&str, &str)] = &[
    ("phone", "android/telephony/TelephonyManager"),
    ("wifi", "android/net/wifi/WifiManager"),
    ("connectivity", "android/net/ConnectivityManager"),
    ("sensor", "android/hardware/SensorManager"),
    ("activity", "android/app/ActivityManager"),
    ("window", "android/view/WindowManager"),
    ("audio", "android/media/AudioManager"),
];

// ============================================================================
// FrameworkRegistry
// ============================================================================

/// Android framework stub registry。
#[derive(Debug)]
pub struct FrameworkRegistry {
    /// builtin class spec catalog（class_name → spec）。
    classes: HashMap<String, FrameworkClassSpec>,
    /// service registry（与 `getSystemService` handler 共享）。
    services: Arc<Mutex<ServiceRegistry>>,
    /// APK context 共享句柄（与 package/signature handler 共享）。
    apk: Arc<RwLock<Option<ApkContext>>>,
}

impl Default for FrameworkRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkRegistry {
    /// 创建空的 framework registry。
    pub fn new() -> Self {
        Self {
            classes: HashMap::new(),
            services: Arc::new(Mutex::new(ServiceRegistry::new())),
            apk: Arc::new(RwLock::new(None)),
        }
    }

    /// 设置 APK context（链式）。支持 install 前 / 后 live 更新（mock 数据路径）。
    pub fn with_apk(self, apk: ApkContext) -> Self {
        self.set_apk(apk);
        self
    }

    /// 设置 APK context。
    pub fn set_apk(&self, apk: ApkContext) {
        *self.apk.write().unwrap() = Some(apk);
    }

    /// service registry 的共享句柄（handler / 测试用）。
    pub fn services(&self) -> Arc<Mutex<ServiceRegistry>> {
        Arc::clone(&self.services)
    }

    /// APK context 的共享句柄。
    pub fn apk_handle(&self) -> Arc<RwLock<Option<ApkContext>>> {
        Arc::clone(&self.apk)
    }

    /// 已安装的 builtin class catalog（class_name → spec）。
    pub fn classes(&self) -> &HashMap<String, FrameworkClassSpec> {
        &self.classes
    }

    /// 装配全部 builtin framework class + service 到 `AndroidVM`。
    ///
    /// # 步骤
    ///
    /// 1. APK 同步：若本 registry 的 apk 句柄为空，从 `vm` 取一份。
    /// 2. 构建 [`FrameworkCtx`]（共享 vm 的对象池 / id 分配器 + 自有 apk/service 句柄）。
    /// 3. 注册默认 service（phone/wifi/…），每个分配稳定 stub ObjectId。
    /// 4. 分配 framework 单例（PackageManager / ApplicationInfo / AssetManager）。
    /// 5. 构建 builtin class specs，物化后经 `register_or_merge_class` 写入 `JniRegistry`。
    ///
    /// 可重复调用：spec 物化走 merge 语义，不会因重复注册失败。
    pub fn install(&mut self, vm: &mut AndroidVM) -> Result<(), JniError> {
        // 1. APK 同步
        if self.apk.read().unwrap().is_none() {
            if let Some(apk) = vm.apk.as_ref() {
                *self.apk.write().unwrap() = Some(apk.clone());
            }
        }

        // 2. 构建 ctx（共享 vm 对象池 + 自有 apk/service）
        let ctx = FrameworkCtx::new(
            Arc::clone(&vm.objects),
            Arc::clone(&vm.object_id_alloc),
            Arc::clone(&self.apk),
            Arc::clone(&self.services),
        );

        // 3. 注册默认 service
        self.register_default_services(&ctx)?;

        // 4. 分配单例
        self.allocate_singletons(&ctx)?;

        // 5. 构建 + 物化 + 注册全部 builtin class
        let specs = catalog::build_all(&ctx);
        self.classes.clear();
        for spec in specs {
            let name = spec.class_name.clone();
            let class_def = spec.materialize()?;
            vm.classes.register_or_merge_class(class_def)?;
            self.classes.insert(name, spec);
        }

        Ok(())
    }

    /// 注册默认 service 集合：每个 name 分配一个稳定 stub ObjectId。
    fn register_default_services(&self, ctx: &FrameworkCtx) -> Result<(), JniError> {
        let mut services = self.services.lock().unwrap();
        for &name in DEFAULT_SERVICE_NAMES {
            // service name → manager class 名（仅作 stub 的 class_name 标签）
            let class_name = DEFAULT_SERVICE_CLASSES
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, c)| *c)
                .unwrap_or("java/lang/Object");
            let oid = ctx.intern_stub(class_name, ObjectStorage::StubInstance { data: Box::new(()) })?;
            services.register(name, oid);
        }
        Ok(())
    }

    /// 分配 framework 单例 stub（PackageManager / ApplicationInfo / AssetManager），
    /// 写入 ctx.singletons 供 Context getter 返回。
    fn allocate_singletons(&self, ctx: &FrameworkCtx) -> Result<(), JniError> {
        let package_name = {
            let apk = ctx.apk.read().unwrap();
            apk.as_ref()
                .map(|a| a.package_name.clone())
                .unwrap_or_else(|| catalog::mock_package_name().to_string())
        };

        let pm_oid = ctx.intern_stub(
            "android/content/pm/PackageManager",
            ObjectStorage::StubInstance { data: Box::new(()) },
        )?;
        let ai_oid = ctx.intern_stub(
            "android/content/pm/ApplicationInfo",
            ObjectStorage::StubInstance {
                data: Box::new(ApplicationInfoData { package_name }),
            },
        )?;
        let am_oid = ctx.intern_stub(
            "android/content/res/AssetManager",
            ObjectStorage::StubInstance { data: Box::new(()) },
        )?;

        let mut s = ctx.singletons.lock().unwrap();
        s.package_manager = Some(pm_oid);
        s.application_info = Some(ai_oid);
        s.asset_manager = Some(am_oid);
        Ok(())
    }

    // —— harness 辅助：构造 framework stub 实例 —— //

    /// 构造一个空的 framework stub 实例（StubInstance{()}），返回其 ObjectId。
    ///
    /// 供 harness 创建 Context / PackageManager 等 instance，再经 `dispatch_call` 调用方法。
    /// 对象池/id 分配器取自 `AndroidVM`，与 install 时同一空间。
    pub fn new_stub_instance(&self, vm: &AndroidVM, class_name: &str) -> Result<ObjectId, JniError> {
        let oid = vm.object_id_alloc.lock().unwrap().object();
        vm.objects
            .lock()
            .unwrap()
            .insert(oid, class_name.to_string(), ObjectStorage::StubInstance { data: Box::new(()) })
            .map_err(|_| JniError::Internal(format!("new_stub_instance 时 ObjectId {oid} 已存在")))?;
        Ok(oid)
    }

    /// 构造一个 `Signature` 实例（持有原始签名字节），返回其 ObjectId。
    ///
    /// 供 harness 直接创建 `Signature` 调 `hashCode()` / `toByteArray()` 等。
    /// 等价于 `new Signature(byte[])`，但跳过 `<init>` dispatch（构造语义见
    /// native-jni-lifecycle change）。
    pub fn new_signature(&self, vm: &AndroidVM, bytes: Vec<u8>) -> Result<ObjectId, JniError> {
        let oid = vm.object_id_alloc.lock().unwrap().object();
        vm.objects
            .lock()
            .unwrap()
            .insert(
                oid,
                "android/content/pm/Signature".to_string(),
                ObjectStorage::StubInstance { data: Box::new(bytes) },
            )
            .map_err(|_| JniError::Internal(format!("new_signature 时 ObjectId {oid} 已存在")))?;
        Ok(oid)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apk_context::ApkContext;
    use crate::class::ClassKind;

    #[test]
    fn new_registry_is_empty() {
        let reg = FrameworkRegistry::new();
        assert!(reg.classes().is_empty());
        assert!(reg.services().lock().unwrap().is_empty());
        assert!(reg.apk_handle().read().unwrap().is_none());
    }

    #[test]
    fn set_apk_updates_shared_handle() {
        let reg = FrameworkRegistry::new();
        reg.set_apk(ApkContext::new("com.test".into()));
        assert_eq!(
            reg.apk_handle().read().unwrap().as_ref().unwrap().package_name,
            "com.test"
        );
    }

    #[test]
    fn install_registers_all_builtins_and_services() {
        let mut vm = AndroidVM::new();
        let mut reg = FrameworkRegistry::new();
        reg.install(&mut vm).unwrap();

        // builtin class 全部进了 JniRegistry（统一 authority）
        for required in [
            "android/content/Context",
            "android/content/pm/PackageManager",
            "android/content/pm/Signature",
            "java/util/ArrayList",
        ] {
            assert!(vm.classes.find_class(required).is_some(), "install 后 {required} 应在 JniRegistry");
        }
        // 默认 service 全部注册
        let services_handle = reg.services();
        let services = services_handle.lock().unwrap();
        for &name in DEFAULT_SERVICE_NAMES {
            assert!(services.contains(name), "默认 service `{name}` 应已注册");
        }
        // catalog 也记录了这些 class
        assert!(reg.classes().contains_key("android/content/Context"));
    }

    #[test]
    fn install_syncs_apk_from_runtime() {
        let mut vm = AndroidVM::new().with_apk(ApkContext::new("com.from.rt".into()));
        let mut reg = FrameworkRegistry::new();
        // 不在 reg 上设 apk，install 应从 vm 同步
        reg.install(&mut vm).unwrap();
        assert_eq!(
            reg.apk_handle().read().unwrap().as_ref().unwrap().package_name,
            "com.from.rt"
        );
    }

    #[test]
    fn install_is_idempotent_via_merge() {
        let mut vm = AndroidVM::new();
        let mut reg = FrameworkRegistry::new();
        reg.install(&mut vm).unwrap();
        // 二次 install 不应失败（merge 语义）
        reg.install(&mut vm).unwrap();
        assert!(vm.classes.find_class("android/content/Context").is_some());
    }

    #[test]
    fn new_stub_instance_creates_object() {
        let mut vm = AndroidVM::new();
        let mut reg = FrameworkRegistry::new();
        reg.install(&mut vm).unwrap();

        let oid = reg.new_stub_instance(&vm, "android/content/Context").unwrap();
        let objects = Arc::clone(&vm.objects);
        let store = objects.lock().unwrap();
        assert_eq!(store.class_name(oid), Some("android/content/Context"));
        assert!(matches!(store.storage(oid), Some(ObjectStorage::StubInstance { .. })));
    }

    #[test]
    fn new_signature_stores_bytes() {
        let mut vm = AndroidVM::new();
        let mut reg = FrameworkRegistry::new();
        reg.install(&mut vm).unwrap();

        let oid = reg.new_signature(&vm, vec![0xCA, 0xFE]).unwrap();
        let objects = Arc::clone(&vm.objects);
        let store = objects.lock().unwrap();
        match store.storage(oid) {
            Some(ObjectStorage::StubInstance { data }) => {
                let bytes: &Vec<u8> = data.downcast_ref().unwrap();
                assert_eq!(*bytes, vec![0xCA, 0xFE]);
            }
            other => panic!("expected StubInstance, got {other:?}"),
        }
    }

    #[test]
    fn interface_shells_registered_as_interface() {
        let mut vm = AndroidVM::new();
        let mut reg = FrameworkRegistry::new();
        reg.install(&mut vm).unwrap();
        let cls = vm.classes.find_class("java/util/List").unwrap();
        assert_eq!(cls.kind, ClassKind::Interface);
    }
}
