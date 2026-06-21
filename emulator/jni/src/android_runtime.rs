//! Android Runtime — Emulator 持有的高级整合点。
//!
//! [`AndroidRuntime`] 封装了 `AndroidVM`，
//! 是 Python decorator / Rust builtin 注册链路的最终同步点。
//!
//! # 收敛规则
//!
//! - Python `@java_class` / `register(...)` 生成的 class definition
//!   和 Rust builtin class declaration 都必须先规整为统一 `JClassDef`，
//!   再通过 `AndroidRuntime` 注册到内部 `AndroidVM`。
//! - `AndroidRuntime` 内部的 `AndroidVM` / `JniRegistry` 持有最终 authority。
//! - Python binding 层的 `class_types`、`java_instances` 等至多是 binding-layer adapter cache，
//!   不能成为最终 VM state。

use crate::android_vm::AndroidVM;
use crate::apk_context::ApkContext;
use crate::class::JClassDef;
use crate::error::JniError;
use crate::object_store::ObjectStore;
use crate::refs::RefTable;
use crate::registry::JniRegistry;
use crate::types::ObjectId;
use std::sync::{Arc, Mutex};

// ============================================================================
// AndroidRuntime
// ============================================================================

/// Android Runtime — JNI state 的高级封装。
///
/// 持有 `AndroidVM`，为 emulator 提供统一的 Android/JNI 环境入口。
/// 后续 JNI function table、framework stub、native lifecycle 都通过它访问 VM 状态。
#[derive(Debug)]
pub struct AndroidRuntime {
    /// 内部 VM 状态容器。
    pub vm: AndroidVM,
}

impl AndroidRuntime {
    /// 创建空的 AndroidRuntime。
    pub fn new() -> Self {
        Self {
            vm: AndroidVM::new(),
        }
    }

    /// 绑定 APK context。
    pub fn with_apk(mut self, apk: ApkContext) -> Self {
        self.vm.apk = Some(apk);
        self
    }

    /// 获取 class registry 的不可变引用。
    pub fn classes(&self) -> &JniRegistry {
        &self.vm.classes
    }

    /// 获取 class registry 的可变引用。
    pub fn classes_mut(&mut self) -> &mut JniRegistry {
        &mut self.vm.classes
    }

    /// 获取 ref table 的不可变引用。
    pub fn refs(&self) -> &RefTable {
        &self.vm.refs
    }

    /// 获取 ref table 的可变引用。
    pub fn refs_mut(&mut self) -> &mut RefTable {
        &mut self.vm.refs
    }

    /// 获取 object store 的共享引用。
    ///
    /// 返回 `Arc<Mutex<ObjectStore>>` 的克隆，供闭包（如 Python shim handler、
    /// JNI trampoline hook）捕获并共享对象池访问。
    pub fn objects(&self) -> Arc<Mutex<ObjectStore>> {
        Arc::clone(&self.vm.objects)
    }

    /// 分配一个新的 ObjectId。
    ///
    /// 使用 `JniRegistry` 内部的 `IdAllocator` 统一分配，
    /// 确保 ObjectId 不与 class/field/method ID 冲突。
    pub fn allocate_object_id(&mut self) -> ObjectId {
        self.vm.classes.allocate_object_id()
    }

    /// 获取 APK context（如果有）。
    pub fn apk(&self) -> Option<&ApkContext> {
        self.vm.apk.as_ref()
    }

    /// 注册一个 class definition。
    ///
    /// 这是 Python decorator 和 Rust builtin 的统一注册入口。
    /// class 的 id 若为默认值会自动分配。
    pub fn register_class(&mut self, def: JClassDef) -> Result<(), JniError> {
        self.vm.classes.register_class(def)
    }

    /// 检查是否有 pending 异常。
    pub fn exception_occurred(&self) -> bool {
        self.vm.exceptions.occurred()
    }

    /// 清除当前异常。
    pub fn exception_clear(&mut self) {
        self.vm.exceptions.clear();
    }
}

impl Default for AndroidRuntime {
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
    use crate::class::{ClassKind, JClassDef};
    use crate::object_store::ObjectStorage;
    use crate::types::{ClassId, ObjectId};

    #[test]
    fn new_runtime_is_empty() {
        let rt = AndroidRuntime::new();
        assert!(rt.classes().classes.is_empty());
        assert!(rt.vm.objects.lock().unwrap().is_empty());
        assert!(rt.apk().is_none());
    }

    #[test]
    fn runtime_with_apk() {
        let apk = ApkContext::new("com.test.app".into());
        let rt = AndroidRuntime::new().with_apk(apk);
        assert!(rt.apk().is_some());
    }

    #[test]
    fn runtime_register_class() {
        let mut rt = AndroidRuntime::new();
        let def = JClassDef::new(ClassId(0), "test/Hello".into());
        rt.register_class(def).unwrap();
        assert!(rt.classes().find_class("test/Hello").is_some());
    }

    #[test]
    fn runtime_exception_tracking() {
        let mut rt = AndroidRuntime::new();
        assert!(!rt.exception_occurred());

        rt.vm.exceptions.set(crate::exception::ExceptionRecord::new(
            ObjectId(1),
            "java/lang/Exception".into(),
            "boom".into(),
        ));
        assert!(rt.exception_occurred());

        rt.exception_clear();
        assert!(!rt.exception_occurred());
    }

    #[test]
    fn runtime_full_class_lifecycle() {
        let mut rt = AndroidRuntime::new();

        // 1. 注册 class
        let mut class_def = JClassDef::new(ClassId(0), "java/util/HashMap".into());
        class_def.kind = ClassKind::Class;
        class_def.superclass = Some("java/util/AbstractMap".into());
        class_def.interfaces = vec!["java/util/Map".into(), "java/lang/Cloneable".into()];
        rt.register_class(class_def).unwrap();

        // 2. 验证 class hierarchy
        let cls = rt.classes().find_class("java/util/HashMap").unwrap();
        assert_eq!(cls.name, "java/util/HashMap");
        assert_eq!(cls.superclass, Some("java/util/AbstractMap".into()));
        assert_eq!(cls.interfaces.len(), 2);
        assert!(cls.interfaces.contains(&"java/util/Map".to_string()));
    }

    #[test]
    fn runtime_object_and_ref_flow() {
        let mut rt = AndroidRuntime::new();

        // 创建对象
        let obj_id = ObjectId(1);
        rt.vm.objects.lock().unwrap().insert(
            obj_id,
            "java/lang/Integer".into(),
            ObjectStorage::Wrapper {
                jtype: crate::types::JType::Int,
                value: crate::types::JValue::Int(42),
            },
        ).unwrap();

        // 创建 global ref（survive frame cleanup）
        let handle = rt.refs_mut().new_global(obj_id);
        rt.refs_mut().clear_frame();
        assert_eq!(rt.refs().resolve(handle), Some(obj_id));
    }
}
