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
use crate::object_store::ObjectStore;
use crate::refs::RefTable;
use crate::registry::JniRegistry;

// ============================================================================
// AndroidVM
// ============================================================================

/// Android VM 状态容器。
///
/// 聚合 class registry、object store、ref table、exception state
/// 和可选的 APK context。这是 Rust 侧 VM authority 的单一入口。
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
    pub objects: ObjectStore,
    /// 引用表（handle → ObjectId）。
    pub refs: RefTable,
    /// 当前线程的异常状态。
    pub exceptions: ExceptionState,
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
            objects: ObjectStore::new(),
            refs: RefTable::new(),
            exceptions: ExceptionState::new(),
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
        assert!(vm.objects.is_empty());
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
        vm.objects.insert(
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
        assert!(vm.objects.contains(obj_id));
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
}
