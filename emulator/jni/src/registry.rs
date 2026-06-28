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
use crate::types::{ClassId, FieldSig, IdAllocator, JValue, MethodId, MethodSig, ObjectId};
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

    /// 按方法名查找所有同名方法签名（用于 GetMethodID 的重载解析）。
    ///
    /// 返回所有匹配名称的 `&MethodSig`（instance + static 均包含）。
    pub fn methods_by_name<'a>(&'a self, class_name: &str, method_name: &str) -> Vec<&'a MethodSig> {
        let cls = match self.classes.get(class_name) {
            Some(c) => c,
            None => return Vec::new(),
        };
        cls.find_methods_by_name(method_name)
    }

    /// 按 class 和 signature 查找 MethodId。
    pub fn method_id(&self, class_name: &str, sig: &MethodSig) -> Option<MethodId> {
        let cls = self.classes.get(class_name)?;
        cls.method_id(sig)
    }

    /// 沿 superclass 链向上收集 class（含起始 class，子→父有序）。
    ///
    /// 真实 JNI 的 `GetMethodID` / `Call*Method` 沿继承链查找 instance method——
    /// 子类未定义时回退到 superclass。本方法返回有序 class 引用（子在前），
    /// 带环保护（superclass 自引用或链成环时安全终止），供继承方法解析使用。
    pub fn class_chain(&self, class_name: &str) -> Vec<&JClassDef> {
        let mut chain: Vec<&JClassDef> = Vec::new();
        let mut current = class_name.to_string();
        loop {
            // 环保护：当前 class 已在链中则终止（避免 superclass 成环导致死循环）
            if chain.iter().any(|c| c.name == current) {
                break;
            }
            let cls = match self.classes.get(&current) {
                Some(c) => c,
                None => break,
            };
            chain.push(cls);
            match &cls.superclass {
                Some(s) if !s.is_empty() => current = s.clone(),
                _ => break,
            }
        }
        chain
    }

    /// 沿继承链解析 instance method（真实 JNI 语义：子类未定义时回退 superclass）。
    ///
    /// 从 `class_name` 起向上查 superclass，返回第一个匹配 `name`+`sig_str` 的方法的
    /// owner class name 与 `MethodSig`。返回的 `sig.class` 已指向 owner，调用方用 owner
    /// class 取 `MethodId`，`dispatch_call` 据此定位 handler。
    pub fn resolve_inherited_method(
        &self,
        class_name: &str,
        name: &str,
        sig_str: &str,
    ) -> Option<(String, MethodSig)> {
        for cls in self.class_chain(class_name) {
            for sig in cls.find_methods_by_name(name) {
                if sig.descriptor() == sig_str {
                    return Some((cls.name.clone(), sig.clone()));
                }
            }
        }
        None
    }

    /// 沿继承链按 `MethodId` 查找 instance method signature。
    ///
    /// `Call*Method` 持有的是 `MethodId`（可能继承自 superclass）；沿继承链查找拥有该 id
    /// 的 class，返回其 `MethodSig`（`sig.class` 指向 owner，`dispatch_call` 据此定位 handler）。
    pub fn resolve_method_by_id(&self, class_name: &str, method_id: u64) -> Option<MethodSig> {
        for cls in self.class_chain(class_name) {
            if let Some((sig, _)) = cls.methods.iter().find(|(_, def)| def.id.0 == method_id) {
                return Some(sig.clone());
            }
        }
        None
    }

    /// 分配一个新的 ObjectId。
    ///
    /// 使用内部 `IdAllocator` 统一分配，确保与 class/method/field ID 不冲突。
    pub fn allocate_object_id(&mut self) -> ObjectId {
        self.id_alloc.object()
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

    /// 注册 class definition，若 class 已存在则合并（override 语义）。
    ///
    /// # 合并规则
    ///
    /// - `def` 中的 method/field **替换**同名已有实现（Python override 优先）
    /// - 已有 class 中未被 `def` 覆盖的 method/field **保留**（framework stub 回落）
    /// - class 不存在时，行为与 [`register_class`](Self::register_class) 相同
    ///
    /// 此方法用于实现 **Python override > Rust framework stub > fail-fast** 优先级。
    pub fn register_or_merge_class(&mut self, mut def: JClassDef) -> Result<(), JniError> {
        let name = def.name.clone();
        if let Some(existing) = self.classes.get_mut(&name) {
            // 合并 method：Python override 替换已有，framework stub 保留
            for (sig, method_def) in std::mem::take(&mut def.methods) {
                existing.override_method(sig, method_def.is_static, method_def.imp)?;
            }
            for (sig, method_def) in std::mem::take(&mut def.static_methods) {
                existing.override_method(sig, method_def.is_static, method_def.imp)?;
            }
            // 合并 field
            for (sig, field_def) in std::mem::take(&mut def.fields) {
                existing.override_field(sig, field_def.is_static, field_def.access)?;
            }
            for (sig, field_def) in std::mem::take(&mut def.static_fields) {
                existing.override_field(sig, field_def.is_static, field_def.access)?;
            }
            Ok(())
        } else {
            // class 不存在 → 正常注册
            if def.id == ClassId(0) {
                def.id = self.id_alloc.class();
            }
            // 重排 method id 为全局唯一——避免子类自有 method id 与继承自父类的 id 数值相撞，
            // 导致 Call*Method 沿子类链按裸 id 解析时误命中子类自有方法（继承场景的核心 bug）。
            def.reassign_method_ids(&mut self.id_alloc);
            self.classes.insert(name, def);
            Ok(())
        }
    }

}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::class::JClassDef;
    use crate::dispatch::MethodImpl;
    use crate::field::{FieldAccess, SharedField};
    use crate::types::{ClassId, FieldSig, JType, JValue, MethodSig};
    use std::sync::Arc;

    #[test]
    fn register_or_merge_new_class() {
        let mut registry = JniRegistry::new();
        let def = JClassDef::new(ClassId(0), "test/NewClass".into());
        registry.register_or_merge_class(def).unwrap();
        assert!(registry.find_class("test/NewClass").is_some());
        // ClassId 自动分配
        assert_ne!(registry.find_class("test/NewClass").unwrap().id, ClassId(0));
    }

    #[test]
    fn register_or_merge_existing_class_preserves_unmatched() {
        let mut registry = JniRegistry::new();

        // 先注册 framework stub class（模拟 Rust builtin）
        let mut fw_def = JClassDef::new(ClassId(0), "test/Shared".into());
        let fw_sig = MethodSig {
            class: "test/Shared".into(),
            name: "frameworkOnly".into(),
            args: vec![],
            ret: JType::Int,
        };
        fw_def.add_method(fw_sig.clone(), false,
            MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(100))))).unwrap();
        registry.register_class(fw_def).unwrap();

        // 再注册 Python override class
        let mut py_def = JClassDef::new(ClassId(0), "test/Shared".into());
        py_def.add_method(fw_sig.clone(), false,
            MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(999))))).unwrap();
        let py_new_sig = MethodSig {
            class: "test/Shared".into(),
            name: "pythonOnly".into(),
            args: vec![],
            ret: JType::Int,
        };
        py_def.add_method(py_new_sig, false,
            MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(300))))).unwrap();

        registry.register_or_merge_class(py_def).unwrap();

        let cls = registry.find_class("test/Shared").unwrap();
        assert_eq!(cls.methods.len(), 2);

        // 验证 Python override 生效
        let mut refs = RefTable::new();
        let args = crate::args::JniArgs::new();
        let result = registry.dispatch_call(&fw_sig, &args, &mut refs).unwrap();
        assert_eq!(result, JValue::Int(999), "Python override 应生效");
    }

    #[test]
    fn register_or_merge_field_override() {
        let mut registry = JniRegistry::new();

        let mut fw_def = JClassDef::new(ClassId(0), "test/FieldShared".into());
        let field_sig = FieldSig {
            class: "test/FieldShared".into(),
            name: "count".into(),
            ty: JType::Int,
        };
        fw_def.add_field(field_sig.clone(), true,
            FieldAccess::RustNative(Arc::new(SharedField::new(JValue::Int(0))))).unwrap();
        registry.register_class(fw_def).unwrap();

        let mut py_def = JClassDef::new(ClassId(0), "test/FieldShared".into());
        py_def.add_field(field_sig.clone(), true,
            FieldAccess::RustNative(Arc::new(SharedField::new(JValue::Int(42))))).unwrap();
        registry.register_or_merge_class(py_def).unwrap();

        let cls = registry.find_class("test/FieldShared").unwrap();
        assert_eq!(cls.static_fields.len(), 1);
        let val = registry.dispatch_static_field_get(&field_sig).unwrap();
        assert_eq!(val, JValue::Int(42), "Python override field 应生效");
    }

    #[test]
    fn register_or_merge_preserves_class_id() {
        let mut registry = JniRegistry::new();

        let fw_def = JClassDef::new(ClassId(0), "test/IdPreserve".into());
        registry.register_or_merge_class(fw_def).unwrap();
        let original_id = registry.find_class("test/IdPreserve").unwrap().id;

        let py_def = JClassDef::new(ClassId(0), "test/IdPreserve".into());
        registry.register_or_merge_class(py_def).unwrap();
        let after_merge_id = registry.find_class("test/IdPreserve").unwrap().id;

        assert_eq!(original_id, after_merge_id, "merge 不应改变已有 ClassId");
    }

    // —— 继承链解析（superclass method resolution）—— //

    /// 构造一个两层的继承结构：Parent 定义 `ping()I`，Child extends Parent 不重定义。
    fn inheritance_registry() -> JniRegistry {
        let mut registry = JniRegistry::new();

        let mut parent = JClassDef::new(ClassId(0), "test/Parent".into());
        let ping = MethodSig {
            class: "test/Parent".into(),
            name: "ping".into(),
            args: vec![],
            ret: JType::Int,
        };
        parent
            .add_method(ping, false, MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(7)))))
            .unwrap();
        registry.register_class(parent).unwrap();

        // Child 的 superclass 指向 Parent（JClassDef::new 默认 superclass = java/lang/Object，
        // 这里显式覆盖）。
        let mut child = JClassDef::new(ClassId(0), "test/Child".into());
        child.superclass = Some("test/Parent".into());
        registry.register_class(child).unwrap();

        registry
    }

    #[test]
    fn class_chain_walks_superclass_in_order() {
        let registry = inheritance_registry();
        let chain: Vec<String> = registry.class_chain("test/Child")
            .iter()
            .map(|c| c.name.clone())
            .collect();
        assert_eq!(chain, vec!["test/Child", "test/Parent"], "链应子→父有序");
    }

    #[test]
    fn class_chain_cycle_safe() {
        let mut registry = JniRegistry::new();
        // 构造一个自引用环（A.superclass = A）验证不死循环
        let mut a = JClassDef::new(ClassId(0), "test/A".into());
        a.superclass = Some("test/A".into());
        registry.register_class(a).unwrap();
        let chain = registry.class_chain("test/A");
        assert_eq!(chain.len(), 1, "自引用环应被环保护终止");
    }

    #[test]
    fn resolve_inherited_method_finds_superclass_definition() {
        let registry = inheritance_registry();
        // Child 未定义 ping，应沿链回退到 Parent。
        let (owner, sig) = registry
            .resolve_inherited_method("test/Child", "ping", "()I")
            .expect("继承方法应解析到 Parent");
        assert_eq!(owner, "test/Parent");
        assert_eq!(sig.class, "test/Parent");
    }

    #[test]
    fn resolve_inherited_method_misses_unknown() {
        let registry = inheritance_registry();
        assert!(registry.resolve_inherited_method("test/Child", "nope", "()I").is_none());
    }

    #[test]
    fn resolve_method_by_id_walks_chain() {
        let registry = inheritance_registry();
        // 取 Parent.ping 的 MethodId
        let parent = registry.find_class("test/Parent").unwrap();
        let ping_sig = MethodSig {
            class: "test/Parent".into(),
            name: "ping".into(),
            args: vec![],
            ret: JType::Int,
        };
        let ping_id = parent.method_id(&ping_sig).unwrap().0;

        // 从 Child 解析该 MethodId → 应沿链找到 Parent 的 sig
        let resolved = registry
            .resolve_method_by_id("test/Child", ping_id)
            .expect("MethodId 应沿继承链解析");
        assert_eq!(resolved.class, "test/Parent", "sig.class 指向 owner");

        // dispatch_call 用解析出的 sig（class=Parent）应能定位 handler 并执行
        let mut refs = RefTable::new();
        let args = crate::args::JniArgs::new();
        let result = registry.dispatch_call(&resolved, &args, &mut refs).unwrap();
        assert_eq!(result, JValue::Int(7), "继承方法 dispatch 应命中 Parent handler");
    }
}
