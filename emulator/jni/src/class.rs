//! JNI class 定义与构建器。
//!
//! `JClassDef` 包含一个 Java class 的完整 shim 定义：
//! class name、instance/static method 表和 instance/static field 表。
//!
//! `ClassBuilder` 提供链式 API 方便构造。

use crate::dispatch::MethodImpl;
use crate::error::JniError;
use crate::field::FieldAccess;
use crate::registry::JniRegistry;
use crate::types::{FieldSig, MethodSig};
use std::collections::HashMap;

/// JNI class shim 定义。
///
/// 包含该 class 的所有已注册 method 和 field 的定义。
#[derive(Debug)]
pub struct JClassDef {
    /// slash-separated class name（如 `"android/content/pm/Signature"`）。
    pub name: String,
    /// instance method 表（key = MethodSig）。
    pub methods: HashMap<MethodSig, JMethodDef>,
    /// static method 表。
    pub static_methods: HashMap<MethodSig, JMethodDef>,
    /// instance field 表（key = FieldSig）。
    pub fields: HashMap<FieldSig, JFieldDef>,
    /// static field 表。
    pub static_fields: HashMap<FieldSig, JFieldDef>,
}

impl JClassDef {
    /// 创建 class 定义（仅有 name，method/field 为空）。
    pub fn new(name: String) -> Self {
        Self {
            name,
            methods: HashMap::new(),
            static_methods: HashMap::new(),
            fields: HashMap::new(),
            static_fields: HashMap::new(),
        }
    }

    /// 添加 instance method。重复签名立即失败。
    pub fn add_method(&mut self, sig: MethodSig, is_static: bool, imp: MethodImpl) -> Result<(), JniError> {
        let def = JMethodDef { sig: sig.clone(), is_static, imp };
        let target = if is_static { &mut self.static_methods } else { &mut self.methods };
        if target.contains_key(&sig) {
            return Err(JniError::DuplicateRegistration(format!("method: {sig}")));
        }
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
}

/// 已注册的 method 定义。
#[derive(Debug)]
pub struct JMethodDef {
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
    /// 任何签名冲突都会导致立即失败。
    pub fn finish(self) -> Result<(), JniError> {
        // 先注册空 class（如果是新 class）
        if !self.registry.classes.contains_key(&self.class_name) {
            self.registry.classes.insert(
                self.class_name.clone(),
                JClassDef::new(self.class_name.clone()),
            );
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
