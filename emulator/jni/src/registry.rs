//! JNI registry — class / method / field 的集中注册表。
//!
//! 所有 JNI shim 定义都通过此 registry 注册。
//! 新增 class / method / field 不需要编辑中心化 switch-case，
//! 而是通过 registry 的 `register_*` 方法添加。
//!
//! # 关键规则
//!
//! - 重复注册同签名立即失败（fail-fast），不静默覆盖
//! - method key 使用完整 `MethodSig`
//! - field key 使用完整 `FieldSig`

use crate::class::{ClassBuilder, JClassDef};
use crate::dispatch::{MethodImpl, dispatch_call, dispatch_field_get, dispatch_field_set, dispatch_static_call, dispatch_static_field_get, dispatch_static_field_set};
use crate::error::JniError;
use crate::field::FieldAccess;
use crate::refs::RefTable;
use crate::types::{ClassId, FieldSig, IdAllocator, JValue, MethodSig};
use std::collections::HashMap;

// ============================================================================
// JniRegistry
// ============================================================================

/// JNI class / method / field 注册表。
///
/// 注册表不直接接触 backend，只存储元数据和方法实现指针。
/// 所有 guest 可见的对象状态通过 `RefTable` 和 `ObjectStore` 管理。
///
/// # ID 分配
///
/// `JniRegistry` 内部持有 `IdAllocator`，在 `register_class` 时
/// 自动为 `JClassDef` 分配 `ClassId`（如果尚未分配）。
#[derive(Debug, Default)]
pub struct JniRegistry {
    /// 已注册的 class 定义，key 为 slash-separated class name。
    pub classes: HashMap<String, JClassDef>,
    /// ID 分配器（class/object/method/field 统一分配）。
    id_alloc: IdAllocator,
}

impl JniRegistry {
    /// 创建空的注册表。
    pub fn new() -> Self {
        Self {
            classes: HashMap::new(),
            id_alloc: IdAllocator::new(),
        }
    }

    // —— 注册方法 ——

    /// 注册一个完整的 class 定义。
    ///
    /// 如果 class 的 `id` 为默认值（`ClassId(0)`），则自动分配新 ID。
    /// 如果 class 已存在则返回 `DuplicateRegistration` 错误。
    pub fn register_class(&mut self, mut def: JClassDef) -> Result<(), JniError> {
        let name = def.name.clone();
        if self.classes.contains_key(&name) {
            return Err(JniError::DuplicateRegistration(format!("class: {name}")));
        }
        // 自动分配 ClassId（如果尚未分配）
        if def.id == ClassId(0) {
            def.id = self.id_alloc.class();
        }
        self.classes.insert(name, def);
        Ok(())
    }

    /// 为已注册的 class 添加 method。
    ///
    /// class 不存在或 method 已存在则失败。
    pub fn register_method(
        &mut self,
        class_name: &str,
        sig: MethodSig,
        is_static: bool,
        imp: MethodImpl,
    ) -> Result<(), JniError> {
        let cls = self.classes.get_mut(class_name)
            .ok_or_else(|| JniError::ClassNotFound(class_name.to_string()))?;
        cls.add_method(sig, is_static, imp)
    }

    /// 为已注册的 class 添加 field。
    pub fn register_field(
        &mut self,
        class_name: &str,
        sig: FieldSig,
        is_static: bool,
        access: FieldAccess,
    ) -> Result<(), JniError> {
        let cls = self.classes.get_mut(class_name)
            .ok_or_else(|| JniError::ClassNotFound(class_name.to_string()))?;
        cls.add_field(sig, is_static, access)
    }

    // —— 查找方法 ——

    /// 查找 class 定义。
    pub fn find_class(&self, name: &str) -> Option<&JClassDef> {
        self.classes.get(name)
    }

    /// 查找 class 定义（可变引用）。
    pub fn find_class_mut(&mut self, name: &str) -> Option<&mut JClassDef> {
        self.classes.get_mut(name)
    }

    // —— 分发方法 ——

    /// 分发 instance method 调用。
    ///
    /// 从 registry 查找对应 method，按 `MethodImpl` 分发到 Rust-native 或 Python-shim handler。
    pub fn dispatch_call(
        &self,
        sig: &MethodSig,
        args: &crate::args::JniArgs,
        refs: &mut RefTable,
    ) -> Result<JValue, JniError> {
        dispatch_call(self, sig, args, refs)
    }

    /// 分发 static method 调用。
    pub fn dispatch_static(
        &self,
        sig: &MethodSig,
        args: &crate::args::JniArgs,
        refs: &mut RefTable,
    ) -> Result<JValue, JniError> {
        dispatch_static_call(self, sig, args, refs)
    }

    /// 分发 instance field get。
    pub fn dispatch_field_get(
        &self,
        sig: &FieldSig,
    ) -> Result<JValue, JniError> {
        dispatch_field_get(self, sig)
    }

    /// 分发 instance field set。
    pub fn dispatch_field_set(
        &self,
        sig: &FieldSig,
        val: JValue,
    ) -> Result<(), JniError> {
        dispatch_field_set(self, sig, val)
    }

    /// 分发 static field get。
    pub fn dispatch_static_field_get(
        &self,
        sig: &FieldSig,
    ) -> Result<JValue, JniError> {
        dispatch_static_field_get(self, sig)
    }

    /// 分发 static field set。
    pub fn dispatch_static_field_set(
        &self,
        sig: &FieldSig,
        val: JValue,
    ) -> Result<(), JniError> {
        dispatch_static_field_set(self, sig, val)
    }

    /// 注册 class 的简化入口——创建 builder 并注册。
    ///
    /// 使用 [`ClassBuilder`] 可以链式添加 method / field 然后一步注册。
    pub fn build_class(&mut self, name: &str) -> ClassBuilder<'_> {
        ClassBuilder::new(self, name)
    }

}
