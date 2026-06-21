//! JNI class 定义与构建器。
//!
//! `JClassDef` 包含一个 Java class 的完整 shim 定义：
//! class name、superclass、interfaces、class kind、
//! instance/static method 表和 instance/static field 表。
//!
//! `ClassBuilder` 提供链式 API 方便构造。
//!
//! # 与 foundation 阶段的区别
//!
//! foundation 阶段仅定义 name + method/field HashMap。
//! android-vm-state-model 增加：
//! - [`ClassKind`] 枚举（class / interface / enum / annotation / array / primitive）
//! - `superclass` 与 `interfaces` 继承链
//! - typed ID 字段 `ClassId`

use crate::dispatch::MethodImpl;
use crate::error::JniError;
use crate::field::FieldAccess;
use crate::registry::JniRegistry;
use crate::types::{ClassId, FieldSig, MethodId, MethodSig};
use std::collections::HashMap;

/// Java class 的种类。
///
/// 区分普通 class、interface、enum、annotation 等，
/// 影响 method dispatch 和 field 访问语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClassKind {
    /// 普通 class。
    Class,
    /// interface（仅有方法签名，无实现）。
    Interface,
    /// enum 类型。
    Enum,
    /// annotation 类型（`@interface`）。
    Annotation,
    /// 数组类型（如 `int[]`、`String[]`）。
    Array,
    /// primitive 类型（如 `int`、`boolean`）——仅用于类型系统完整性。
    Primitive,
}

impl ClassKind {
    /// 是否为 interface 类型。
    pub fn is_interface(&self) -> bool {
        matches!(self, ClassKind::Interface)
    }

    /// 是否为数组类型。
    pub fn is_array(&self) -> bool {
        matches!(self, ClassKind::Array)
    }

    /// 是否为 primitive 类型。
    pub fn is_primitive(&self) -> bool {
        matches!(self, ClassKind::Primitive)
    }
}

/// JNI class shim 定义。
///
/// 包含该 class 的所有已注册 method 和 field 的定义，
/// 以及继承链信息（superclass、interfaces）。
///
/// # 聚合根
///
/// `JClassDef` 是 method / field 的聚合根——
/// method/field 不作为与 class 并列的顶层权威 registry，
/// 而是归属于 class 成员表。
#[derive(Debug)]
pub struct JClassDef {
    /// typed class ID（Rust 内部权威标识）。
    pub id: ClassId,
    /// slash-separated class name（如 `"android/content/pm/Signature"`）。
    pub name: String,
    /// class 种类。
    pub kind: ClassKind,
    /// 父类名（slash-separated），`java/lang/Object` 的 superclass 为 None。
    pub superclass: Option<String>,
    /// 实现的接口列表（slash-separated）。
    pub interfaces: Vec<String>,
    /// instance method 表（key = MethodSig）。
    pub methods: HashMap<MethodSig, JMethodDef>,
    /// static method 表。
    pub static_methods: HashMap<MethodSig, JMethodDef>,
    /// instance field 表（key = FieldSig）。
    pub fields: HashMap<FieldSig, JFieldDef>,
    /// static field 表。
    pub static_fields: HashMap<FieldSig, JFieldDef>,
    /// method 签名 → MethodId 索引（仅加速查找，非权威）。
    method_index: HashMap<MethodSig, MethodId>,
    /// method name→Vec<MethodSig> 缓存，用于按名称快速查找（支持重载）。
    method_name_cache: HashMap<String, Vec<MethodSig>>,
}

impl JClassDef {
    /// 创建 class 定义（仅有 name + id，method/field 为空）。
    ///
    /// 默认 `kind` 为 `ClassKind::Class`，
    /// `superclass` 为 `"java/lang/Object"`（除 Object 自身外），
    /// `interfaces` 为空。
    pub fn new(id: ClassId, name: String) -> Self {
        let superclass = if name == "java/lang/Object" {
            None
        } else {
            Some("java/lang/Object".to_string())
        };
        Self {
            id,
            name,
            kind: ClassKind::Class,
            superclass,
            interfaces: Vec::new(),
            methods: HashMap::new(),
            static_methods: HashMap::new(),
            fields: HashMap::new(),
            static_fields: HashMap::new(),
            method_index: HashMap::new(),
            method_name_cache: HashMap::new(),
        }
    }

    /// 设置 class 种类。
    pub fn with_kind(mut self, kind: ClassKind) -> Self {
        self.kind = kind;
        self
    }

    /// 设置父类。
    pub fn with_superclass(mut self, superclass: Option<String>) -> Self {
        self.superclass = superclass;
        self
    }

    /// 添加实现的接口。
    pub fn with_interface(mut self, interface: String) -> Self {
        self.interfaces.push(interface);
        self
    }

    /// 添加 method。重复签名立即失败。
    ///
    /// 同时更新 method_index 和 method_name_cache。
    pub fn add_method(&mut self, sig: MethodSig, is_static: bool, imp: MethodImpl) -> Result<(), JniError> {
        let target = if is_static { &mut self.static_methods } else { &mut self.methods };
        if target.contains_key(&sig) {
            return Err(JniError::DuplicateRegistration(format!("method: {sig}")));
        }
        // 分配 MethodId 并维护索引
        let method_id = MethodId(self.method_index.len() as u64 + 1);
        self.method_index.insert(sig.clone(), method_id);
        self.method_name_cache
            .entry(sig.name.clone())
            .or_default()
            .push(sig.clone());

        let def = JMethodDef { id: method_id, sig: sig.clone(), is_static, imp };
        target.insert(sig, def);
        Ok(())
    }

    /// 添加 field。重复签名立即失败。
    pub fn add_field(
        &mut self,
        sig: FieldSig,
        is_static: bool,
        access: FieldAccess,
    ) -> Result<(), JniError> {
        let def = JFieldDef { sig: sig.clone(), is_static, access };
        let target = if is_static { &mut self.static_fields } else { &mut self.fields };
        if target.contains_key(&sig) {
            return Err(JniError::DuplicateRegistration(format!("field: {sig}")));
        }
        target.insert(sig, def);
        Ok(())
    }

    /// 按 method name 查找所有重载的签名列表（从 name cache 索引）。
    ///
    /// 返回该名称的所有已注册 MethodSig（包括 instance 和 static）。
    pub fn find_methods_by_name(&self, name: &str) -> Vec<&MethodSig> {
        self.method_name_cache.get(name).map(|v| v.iter().collect()).unwrap_or_default()
    }

    /// 按 MethodSig 查找对应的 MethodId。
    pub fn method_id(&self, sig: &MethodSig) -> Option<MethodId> {
        self.method_index.get(sig).copied()
    }

    /// 判断此 class 是否为接口。
    pub fn is_interface(&self) -> bool {
        self.kind.is_interface()
    }

    /// 判断此 class 是否为数组类型。
    pub fn is_array(&self) -> bool {
        self.kind.is_array()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::MethodImpl;
    use crate::field::FieldAccess;
    use crate::types::JType;
    use crate::JValue;
    use std::sync::Arc;

    // —— ClassKind 测试 ——

    #[test]
    fn class_kind_predicates() {
        assert!(!ClassKind::Class.is_interface());
        assert!(ClassKind::Interface.is_interface());
        assert!(!ClassKind::Enum.is_interface());

        assert!(ClassKind::Array.is_array());
        assert!(!ClassKind::Class.is_array());

        assert!(ClassKind::Primitive.is_primitive());
        assert!(!ClassKind::Class.is_primitive());
    }

    // —— Class hierarchy 测试 ——

    #[test]
    fn class_def_defaults() {
        let def = JClassDef::new(ClassId(1), "my/pkg/MyClass".into());
        assert_eq!(def.name, "my/pkg/MyClass");
        assert_eq!(def.id, ClassId(1));
        assert_eq!(def.kind, ClassKind::Class);
        assert_eq!(def.superclass, Some("java/lang/Object".into()));
        assert!(def.interfaces.is_empty());
    }

    #[test]
    fn object_class_has_no_superclass() {
        let def = JClassDef::new(ClassId(1), "java/lang/Object".into());
        assert_eq!(def.superclass, None);
    }

    #[test]
    fn class_def_with_kind_and_interfaces() {
        let def = JClassDef::new(ClassId(1), "my/pkg/MyInterface".into())
            .with_kind(ClassKind::Interface)
            .with_interface("java/io/Serializable".into());

        assert!(def.is_interface());
        assert_eq!(def.interfaces.len(), 1);
        assert_eq!(def.interfaces[0], "java/io/Serializable");
    }

    #[test]
    fn class_def_with_custom_superclass() {
        let def = JClassDef::new(ClassId(1), "android/app/Activity".into())
            .with_superclass(Some("android/content/Context".into()));

        assert_eq!(def.superclass, Some("android/content/Context".into()));
    }

    // —— Method name cache 测试 ——

    #[test]
    fn method_name_cache_tracks_overloads() {
        let mut def = JClassDef::new(ClassId(1), "test/Overload".into());

        // 同一个 method 名，不同签名（重载）
        let sig1 = MethodSig { class: "test/Overload".into(), name: "foo".into(), args: vec![], ret: JType::Void };
        let sig2 = MethodSig { class: "test/Overload".into(), name: "foo".into(), args: vec![JType::Int], ret: JType::Void };

        def.add_method(sig1, false, MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Void)))).unwrap();
        def.add_method(sig2, false, MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Void)))).unwrap();

        let found = def.find_methods_by_name("foo");
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn method_name_cache_empty_for_unknown() {
        let def = JClassDef::new(ClassId(1), "test/Empty".into());
        let found = def.find_methods_by_name("nonexistent");
        assert!(found.is_empty());
    }

    // —— MethodId 索引测试 ——

    #[test]
    fn method_id_index_lookup() {
        let mut def = JClassDef::new(ClassId(1), "test/HasMethod".into());
        let sig = MethodSig { class: "test/HasMethod".into(), name: "bar".into(), args: vec![], ret: JType::Int };

        def.add_method(sig.clone(), false, MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(0))))).unwrap();

        let mid = def.method_id(&sig);
        assert!(mid.is_some());
        assert_eq!(mid.unwrap(), MethodId(1));
    }

    // —— 完整 class 构建测试 ——

    #[test]
    fn full_class_with_method_fields_and_hierarchy() {
        let mut def = JClassDef::new(ClassId(1), "android/content/pm/Signature".into())
            .with_kind(ClassKind::Class)
            .with_superclass(Some("java/lang/Object".into()))
            .with_interface("android/os/Parcelable".into());

        // 添加 static field
        def.add_field(
            FieldSig { class: "android/content/pm/Signature".into(), name: "CREATOR".into(), ty: JType::Object("android/os/Parcelable$Creator".into()) },
            true,
            FieldAccess::RustNative(Arc::new(crate::field::SharedField::new(JValue::Null))),
        ).unwrap();

        // 添加 instance method
        def.add_method(
            MethodSig { class: "android/content/pm/Signature".into(), name: "hashCode".into(), args: vec![], ret: JType::Int },
            false,
            MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(0x12345678)))),
        ).unwrap();

        // 验证结构完整性
        assert_eq!(def.name, "android/content/pm/Signature");
        assert_eq!(def.kind, ClassKind::Class);
        assert_eq!(def.superclass, Some("java/lang/Object".into()));
        assert!(def.interfaces.contains(&"android/os/Parcelable".to_string()));
        assert_eq!(def.static_fields.len(), 1);
        assert_eq!(def.methods.len(), 1);
        assert!(!def.is_array());
        assert!(!def.is_interface());
    }

    /// 验证 ClassBuilder::finish() 通过 register_class() 分配了真实 ClassId，
    /// 而不是停留在 ClassId(0)。
    #[test]
    fn class_builder_assigns_real_class_id() {
        let mut registry = JniRegistry::new();

        registry.build_class("test/BuilderClass")
            .add_method("getValue()I", false, MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(42)))))
            .finish()
            .unwrap();

        let cls = registry.find_class("test/BuilderClass").unwrap();
        // ClassId 被 register_class 自动分配，不再是默认值 0
        assert_ne!(cls.id, ClassId(0), "ClassBuilder 必须通过 register_class() 分配真实 ClassId");
        assert_eq!(cls.id.0, 1, "第一个注册的 class 应拿到 ClassId(1)");
    }

    /// 验证通过 builder 连续注册多个 class 时，ClassId 递增。
    #[test]
    fn class_builder_sequential_class_ids() {
        let mut registry = JniRegistry::new();

        registry.build_class("test/First")
            .add_method("a()V", false, MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Void))))
            .finish()
            .unwrap();

        registry.build_class("test/Second")
            .add_method("b()V", false, MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Void))))
            .finish()
            .unwrap();

        let first = registry.find_class("test/First").unwrap();
        let second = registry.find_class("test/Second").unwrap();

        assert_ne!(first.id, ClassId(0));
        assert_ne!(second.id, ClassId(0));
        assert_ne!(first.id, second.id, "不同 class 应拿到不同的 ClassId");
    }
}

/// 已注册的 method 定义。
#[derive(Debug)]
pub struct JMethodDef {
    /// typed method ID。
    pub id: MethodId,
    /// method signature（canonical key）。
    pub sig: MethodSig,
    /// 是否为 static method。
    pub is_static: bool,
    /// 实现来源：Rust-native 或 Python-shim。
    pub imp: MethodImpl,
}

/// 已注册的 field 定义。
#[derive(Debug)]
pub struct JFieldDef {
    /// field signature（canonical key）。
    pub sig: FieldSig,
    /// 是否为 static field。
    pub is_static: bool,
    /// 访问器：Rust-native 或 Python-shim。
    pub access: FieldAccess,
}

// ============================================================================
// ClassBuilder — 链式构建 JClassDef
// ============================================================================

/// [`JClassDef`] 的链式构建器。
///
/// 用法：
/// ```ignore
/// registry.build_class("my/Class")
///     .add_method("foo", "(I)V", my_handler, false)
///     .add_field("bar", "I", my_access, false)
///     .finish()?;
/// ```
pub struct ClassBuilder<'r> {
    registry: &'r mut JniRegistry,
    class_name: String,
    /// 待注册的 method 列表（(sig, is_static, impl)）。
    methods: Vec<(MethodSig, bool, MethodImpl)>,
    /// 待注册的 field 列表（(sig, is_static, access)）。
    fields: Vec<(FieldSig, bool, FieldAccess)>,
}

impl<'r> ClassBuilder<'r> {
    /// 创建新的 ClassBuilder。
    ///
    /// `class_name` 为 slash-separated 格式（如 `"my/pkg/MyClass"`）。
    /// 链式调用 `add_method` / `add_field` 后 `finish()` 注册到 registry。
    pub fn new(registry: &'r mut JniRegistry, class_name: &str) -> Self {
        Self {
            registry,
            class_name: class_name.to_string(),
            methods: Vec::new(),
            fields: Vec::new(),
        }
    }

    /// 添加一个 method（通过 descriptor 字符串 + handler）。
    ///
    /// descriptor 格式与 `parse_method_descriptor` 一致。
    /// class name 可选指定（含 `.` 时从 descriptor 提取，否则用 builder 的 class_name）。
    pub fn add_method(
        mut self,
        desc: &str,
        is_static: bool,
        imp: MethodImpl,
    ) -> Self {
        let mut sig = crate::types::MethodSig::parse(desc)
            .expect("method descriptor 解析失败");
        // 如果 descriptor 不含 class name，用 builder 的 class_name
        if sig.class.is_empty() {
            sig.class = self.class_name.clone();
        }
        self.methods.push((sig, is_static, imp));
        self
    }

    /// 添加一个 field（通过 descriptor 字符串 + access）。
    pub fn add_field(
        mut self,
        desc: &str,
        is_static: bool,
        access: FieldAccess,
    ) -> Self {
        let mut sig = crate::types::FieldSig::parse(desc)
            .expect("field descriptor 解析失败");
        if sig.class.is_empty() {
            sig.class = self.class_name.clone();
        }
        self.fields.push((sig, is_static, access));
        self
    }

    /// 完成构建：注册 class 及所有 method/field 到 registry。
    ///
    /// 先通过 [`JniRegistry::register_class`] 注册空 class（自动分配 `ClassId`），
    /// 再依次添加 method 和 field。任何签名冲突都会导致立即失败。
    pub fn finish(self) -> Result<(), JniError> {
        // 先通过 register_class 注册空 class（自动分配 ClassId），
        // 不能直接 insert——否则 ClassId(0) 语义失效。
        if !self.registry.classes.contains_key(&self.class_name) {
            let class_def = JClassDef::new(ClassId(0), self.class_name.clone());
            self.registry.register_class(class_def)?;
        }

        for (sig, is_static, imp) in self.methods {
            self.registry.register_method(&self.class_name, sig, is_static, imp)?;
        }

        for (sig, is_static, access) in self.fields {
            self.registry.register_field(&self.class_name, sig, is_static, access)?;
        }

        Ok(())
    }
}
