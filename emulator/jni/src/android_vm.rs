//! Android VM 状态模型 — JNI-facing Java world 的聚合根。
//!
//! [`AndroidVM`] 聚合了所有 JNI 路径上的核心状态：
//! class registry、object store、ref table、exception state 和 APK context。
//!
//! 这是 `Emulator` 持有的唯一 VM authority，
//! 后续 JNI、framework、Python shim 都复用这一套状态模型。
//!
//! # 设计原则
//!
//! - class 为聚合根，method/field 归属 class
//! - object store 分层存储（string / wrapper / array / stub instance）
//! - ref table 显式区分 local/global/weak
//! - APK context 一等存在，framework 从这里取值
//! - 不包含 bytecode engine（Dalvik/ART）

use crate::apk_context::ApkContext;
use crate::exception::ExceptionState;
use crate::native_registry::NativeRegistry;
use crate::object_store::ObjectStore;
use crate::refs::RefTable;
use crate::registry::JniRegistry;
use crate::types::IdAllocator;
use std::sync::{Arc, Mutex};

// ============================================================================
// AndroidVM
// ============================================================================

/// Android VM 状态容器。
///
/// 聚合 class registry、object store、ref table、exception state、
/// native registry 和可选的 APK context。这是 Rust 侧 VM authority 的单一入口。
///
/// # 与 `JavaVMSurface` 的关系
///
/// `JavaVMSurface` 是 foundation 阶段的最小 JNI surface，
/// 仅持有 `JniRegistry` + `RefTable`。
/// `AndroidVM` 是完整的状态模型，预期在后续 change 中
/// `JavaVMSurface` 退化为对 `AndroidVM` 的引用视图。
#[derive(Debug)]
pub struct AndroidVM {
    /// class / method / field 注册表。
    pub classes: JniRegistry,
    /// 对象存储（ObjectId → class_name + storage）。
    ///
    /// 用 `Arc<Mutex<>>` 包装，支持多个闭包（如 Python shim handler、
    /// JNI trampoline hook）共享访问对象池。
    pub objects: Arc<Mutex<ObjectStore>>,
    /// 引用表（handle → ObjectId）。
    pub refs: RefTable,
    /// 当前线程的异常状态。
    pub exceptions: ExceptionState,
    /// Native 方法注册表 — RegisterNatives 绑定 + Java_* fallback 查找。
    pub natives: NativeRegistry,
    /// ObjectId 分配器（共享）。
    ///
    /// 与 `objects` 一样用 `Arc<Mutex<>>` 包装，供 Python binding 的 marshalling
    /// 闭包（如 `wrap_python_method`）捕获并共享——当 Python `str`/`bytes` 自动
    /// coercion 成 Java `String`/`byte[]` 对象时，需要分配 `ObjectId` 并写入
    /// `ObjectStore`。所有对象 ID（Python 实例 / marshalling 产物 / 显式 wrapper）
    /// 统一经此分配器分配，互不冲突。
    ///
    /// 注意：class ID 仍由 `JniRegistry` 内部的 `IdAllocator` 分配（独立计数器），
    /// 与此对象分配器分离——两者类型不同（`ClassId` vs `ObjectId`）、存储不同
    /// （registry 按 name 索引 vs ObjectStore 按 ObjectId 索引），不会真正冲突。
    pub object_id_alloc: Arc<Mutex<IdAllocator>>,
    /// APK context（如果当前上下文绑定到某个 APK）。
    /// framework 场景下（如调用 `PackageManager`）应为 `Some`，
    /// 纯 native 场景下可以为 `None`。
    pub apk: Option<ApkContext>,
}

impl AndroidVM {
    /// 创建空的 AndroidVM。
    pub fn new() -> Self {
        Self {
            classes: JniRegistry::new(),
            objects: Arc::new(Mutex::new(ObjectStore::new())),
            refs: RefTable::new(),
            exceptions: ExceptionState::new(),
            natives: NativeRegistry::new(),
            object_id_alloc: Arc::new(Mutex::new(IdAllocator::new())),
            apk: None,
        }
    }

    /// 绑定 APK context。
    ///
    /// 设置后，framework stub 可以通过 VM 读取 package/signature/asset 数据。
    pub fn with_apk(mut self, apk: ApkContext) -> Self {
        self.apk = Some(apk);
        self
    }
}

impl Default for AndroidVM {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apk_context::ApkContext;
    use crate::class::{ClassKind, JClassDef};
    use crate::object_store::ObjectStorage;
    use crate::types::{ClassId, ObjectId};

    #[test]
    fn new_vm_is_empty() {
        let vm = AndroidVM::new();
        assert!(vm.classes.classes.is_empty());
        assert!(vm.objects.lock().unwrap().is_empty());
        assert!(vm.refs.is_empty());
        assert!(!vm.exceptions.occurred());
        assert!(vm.apk.is_none());
    }

    #[test]
    fn vm_with_apk() {
        let apk = ApkContext::new("com.test.app".into());
        let vm = AndroidVM::new().with_apk(apk);

        assert!(vm.apk.is_some());
        assert_eq!(vm.apk.as_ref().unwrap().package_name, "com.test.app");
    }

    #[test]
    fn vm_class_registration_flow() {
        let mut vm = AndroidVM::new();

        // 注册一个 class
        let mut class_def = JClassDef::new(ClassId(0), "test/MyClass".into());
        class_def.kind = ClassKind::Class;
        vm.classes.register_class(class_def).unwrap();

        assert!(vm.classes.find_class("test/MyClass").is_some());
    }

    #[test]
    fn vm_object_lifecycle() {
        let mut vm = AndroidVM::new();
        let obj_id = ObjectId(1);

        // 在 object store 中创建对象
        vm.objects.lock().unwrap().insert(
            obj_id,
            "java/lang/String".into(),
            ObjectStorage::String("hello vm".into()),
        ).unwrap();

        // 创建 local ref
        let handle = vm.refs.new_local(obj_id);
        assert_eq!(vm.refs.resolve(handle), Some(obj_id));

        // clear frame 后 local ref 消失，但对象仍在 store 中
        vm.refs.clear_frame();
        assert_eq!(vm.refs.resolve(handle), None);
        assert!(vm.objects.lock().unwrap().contains(obj_id));
    }

    #[test]
    fn vm_exception_flow() {
        let mut vm = AndroidVM::new();
        assert!(!vm.exceptions.occurred());

        vm.exceptions.set(crate::exception::ExceptionRecord::new(
            ObjectId(1),
            "java/lang/RuntimeException".into(),
            "test error".into(),
        ));
        assert!(vm.exceptions.occurred());

        vm.exceptions.clear();
        assert!(!vm.exceptions.occurred());
    }

    /// 完整 class lifecycle：superclass / interfaces 元数据在注册后正确保留。
    ///
    /// （从已删除的 `android_runtime.rs` 迁移——验证 class hierarchy 语义，
    /// 此前 android_vm 测试未覆盖。）
    #[test]
    fn vm_full_class_lifecycle() {
        let mut vm = AndroidVM::new();

        // 注册 class，带 superclass + interfaces
        let mut class_def = JClassDef::new(ClassId(0), "java/util/HashMap".into());
        class_def.kind = ClassKind::Class;
        class_def.superclass = Some("java/util/AbstractMap".into());
        class_def.interfaces = vec!["java/util/Map".into(), "java/lang/Cloneable".into()];
        vm.classes.register_class(class_def).unwrap();

        // 验证 class hierarchy
        let cls = vm.classes.find_class("java/util/HashMap").unwrap();
        assert_eq!(cls.name, "java/util/HashMap");
        assert_eq!(cls.superclass, Some("java/util/AbstractMap".into()));
        assert_eq!(cls.interfaces.len(), 2);
        assert!(cls.interfaces.contains(&"java/util/Map".to_string()));
    }

    /// global ref 在 frame cleanup 后仍存活（survive），local ref 消失。
    ///
    /// （从已删除的 `android_runtime.rs` 迁移——验证 global ref 生命周期，
    /// `vm_object_lifecycle` 只覆盖了 local ref。）
    #[test]
    fn vm_global_ref_survives_frame_clear() {
        let mut vm = AndroidVM::new();

        // 创建对象
        let obj_id = ObjectId(1);
        vm.objects.lock().unwrap().insert(
            obj_id,
            "java/lang/Integer".into(),
            ObjectStorage::Wrapper {
                jtype: crate::types::JType::Int,
                value: crate::types::JValue::Int(42),
            },
        ).unwrap();

        // global ref 应 survive frame cleanup
        let handle = vm.refs.new_global(obj_id);
        vm.refs.clear_frame();
        assert_eq!(vm.refs.resolve(handle), Some(obj_id));
    }
}
