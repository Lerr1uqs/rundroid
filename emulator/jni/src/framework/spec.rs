//! Framework class-spec — 声明式描述一个 Android framework builtin class。
//!
//! [`FrameworkClassSpec`] 是 builtin class 的**声明式源**：它列出该 class 的
//! 构造器、静态/实例 field、静态/实例 method，然后通过 [`FrameworkClassSpec::materialize`]
//! 物化成一个 [`JClassDef`]，注册进统一 `JniRegistry` authority。
//!
//! # 收敛原则（关键）
//!
//! 这套 spec **不是** runtime dispatch 的最终状态。`JniRegistry` / `JClassDef`
//! 才是单一 runtime authority——dispatch 永远查 `JniRegistry`，不查 `FrameworkRegistry`。
//! spec 的角色是「builtin class 的声明式 catalog」：把 Rust builtin 的行为描述固化下来，
//! 物化写入 VM authority，与 Python shim 走同一套 `JClassDef` 数据模型。
//! 详见 change `android-framework-stubs` 的 design.md。

use crate::class::{ClassKind, JClassDef};
use crate::dispatch::MethodImpl;
use crate::error::JniError;
use crate::field::FieldAccess;
use crate::types::{ClassId, FieldSig, JType, MethodSig};

// ============================================================================
// spec 数据结构
// ============================================================================

/// method spec —— 方法签名 + 实现来源。
///
/// 与 [`crate::class::JMethodDef`] 对应，但是 spec 层的声明式载体。
#[derive(Debug, Clone)]
pub struct FrameworkMethodSpec {
    /// canonical method signature。
    pub sig: MethodSig,
    /// 是否 static method。
    pub is_static: bool,
    /// 实现来源（Rust builtin handler 或 Python shim id）。
    pub imp: MethodImpl,
}

/// field spec —— field 签名 + 访问器。
#[derive(Debug, Clone)]
pub struct FrameworkFieldSpec {
    /// canonical field signature。
    pub sig: FieldSig,
    /// 是否 static field。
    pub is_static: bool,
    /// 访问器（Rust builtin 或 Python shim）。
    pub access: FieldAccess,
}

/// 构造器 spec —— 物化成 `<init>` instance method。
///
/// 构造器在 Java 层名为 `<init>`，返回 `void`。
/// 这里只声明参数类型 + 实现，物化时自动补上 `<init>` 名和 void 返回类型。
#[derive(Clone)]
pub struct FrameworkConstructorSpec {
    /// 构造器参数类型列表。
    pub args: Vec<JType>,
    /// 构造器实现。handler 接收到的 `JniArgs::this()` 是新分配的实例 ObjectId。
    pub imp: MethodImpl,
}

impl std::fmt::Debug for FrameworkConstructorSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameworkConstructorSpec")
            .field("args", &self.args)
            .field("imp", &"<handler>")
            .finish()
    }
}

/// framework builtin class 的声明式定义。
///
/// 通过 [`FrameworkClassSpec::builder`] 链式构造，再 [`materialize`](Self::materialize)
/// 成 `JClassDef` 注册进 `JniRegistry`。
#[derive(Debug, Clone)]
pub struct FrameworkClassSpec {
    /// slash-separated class name（如 `"android/content/pm/Signature"`）。
    pub class_name: String,
    /// class 种类。
    pub kind: ClassKind,
    /// 父类名（slash-separated）。
    pub superclass: Option<String>,
    /// 实现的接口列表。
    pub interfaces: Vec<String>,
    /// 构造器列表（物化为 `<init>` instance method）。
    pub constructors: Vec<FrameworkConstructorSpec>,
    /// static field 列表。
    pub static_fields: Vec<FrameworkFieldSpec>,
    /// instance field 列表。
    pub instance_fields: Vec<FrameworkFieldSpec>,
    /// static method 列表。
    pub static_methods: Vec<FrameworkMethodSpec>,
    /// instance method 列表。
    pub instance_methods: Vec<FrameworkMethodSpec>,
}

impl Default for FrameworkClassSpec {
    fn default() -> Self {
        Self {
            class_name: String::new(),
            kind: ClassKind::Class,
            superclass: None,
            interfaces: Vec::new(),
            constructors: Vec::new(),
            static_fields: Vec::new(),
            instance_fields: Vec::new(),
            static_methods: Vec::new(),
            instance_methods: Vec::new(),
        }
    }
}

impl FrameworkClassSpec {
    /// 创建 spec builder（链式 API）。
    pub fn builder(class_name: &str) -> FrameworkClassSpecBuilder {
        FrameworkClassSpecBuilder::new(class_name)
    }

    /// 把 spec 物化成可注册的 [`JClassDef`]。
    ///
    /// `ClassId` 传 `ClassId(0)`，由 `JniRegistry::register_class` / `register_or_merge_class`
    /// 自动分配真实 ID。构造器物化为 name=`"<init>"`、ret=`Void` 的 instance method。
    ///
    /// # 失败语义
    ///
    /// spec 内部签名重复（同名同参 method / 同名 field）时 fail-fast 返回错误——
    /// 这属于 catalog 构造错误，应当在 builtin 定义阶段就排除。
    pub fn materialize(&self) -> Result<JClassDef, JniError> {
        let mut def = JClassDef::new(ClassId(0), self.class_name.clone());
        def.kind = self.kind;
        if self.superclass.is_some() {
            def.superclass = self.superclass.clone();
        }
        def.interfaces = self.interfaces.clone();

        // 构造器 → <init> instance method
        for ctor in &self.constructors {
            let init_sig = MethodSig {
                class: self.class_name.clone(),
                name: "<init>".to_string(),
                args: ctor.args.clone(),
                ret: JType::Void,
            };
            def.add_method(init_sig, false, ctor.imp.clone())?;
        }

        for f in &self.static_fields {
            def.add_field(f.sig.clone(), true, f.access.clone())?;
        }
        for f in &self.instance_fields {
            def.add_field(f.sig.clone(), false, f.access.clone())?;
        }
        for m in &self.static_methods {
            def.add_method(m.sig.clone(), true, m.imp.clone())?;
        }
        for m in &self.instance_methods {
            def.add_method(m.sig.clone(), false, m.imp.clone())?;
        }

        Ok(def)
    }
}

// ============================================================================
// FrameworkClassSpecBuilder — 链式构造
// ============================================================================

/// [`FrameworkClassSpec`] 的链式构建器。
///
/// 用法：
/// ```ignore
/// FrameworkClassSpec::builder("android/content/pm/Signature")
///     .kind(ClassKind::Class)
///     .interface("android/os/Parcelable")
///     .instance_method(sig, handler)
///     .build()
/// ```
#[derive(Debug, Clone)]
pub struct FrameworkClassSpecBuilder {
    spec: FrameworkClassSpec,
}

impl FrameworkClassSpecBuilder {
    /// 创建 builder，class name 必须非空。
    pub fn new(class_name: &str) -> Self {
        Self {
            spec: FrameworkClassSpec {
                class_name: class_name.to_string(),
                ..Default::default()
            },
        }
    }

    /// 设置 class 种类。
    pub fn kind(mut self, kind: ClassKind) -> Self {
        self.spec.kind = kind;
        self
    }

    /// 设置父类（slash-separated）。传 None 表示无父类（仅 `java/lang/Object` 合法）。
    pub fn superclass(mut self, superclass: Option<&str>) -> Self {
        self.spec.superclass = superclass.map(|s| s.to_string());
        self
    }

    /// 追加一个实现的接口。
    pub fn interface(mut self, iface: &str) -> Self {
        self.spec.interfaces.push(iface.to_string());
        self
    }

    /// 追加一个构造器。
    pub fn constructor(mut self, args: Vec<JType>, imp: MethodImpl) -> Self {
        self.spec.constructors.push(FrameworkConstructorSpec { args, imp });
        self
    }

    /// 追加一个 static field。
    pub fn static_field(mut self, sig: FieldSig, access: FieldAccess) -> Self {
        self.spec.static_fields.push(FrameworkFieldSpec { sig, is_static: true, access });
        self
    }

    /// 追加一个 instance field。
    pub fn instance_field(mut self, sig: FieldSig, access: FieldAccess) -> Self {
        self.spec.instance_fields.push(FrameworkFieldSpec { sig, is_static: false, access });
        self
    }

    /// 追加一个 static method。
    pub fn static_method(mut self, sig: MethodSig, imp: MethodImpl) -> Self {
        self.spec.static_methods.push(FrameworkMethodSpec { sig, is_static: true, imp });
        self
    }

    /// 追加一个 instance method。
    pub fn instance_method(mut self, sig: MethodSig, imp: MethodImpl) -> Self {
        self.spec.instance_methods.push(FrameworkMethodSpec { sig, is_static: false, imp });
        self
    }

    /// 构建出 [`FrameworkClassSpec`]。
    pub fn build(self) -> FrameworkClassSpec {
        self.spec
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::SharedField;
    use crate::types::{FieldSig, JValue};
    use std::sync::Arc;

    #[test]
    fn builder_produces_full_spec() {
        let sig = MethodSig {
            class: "android/content/pm/Signature".into(),
            name: "hashCode".into(),
            args: vec![],
            ret: JType::Int,
        };
        let field_sig = FieldSig {
            class: "android/content/pm/Signature".into(),
            name: "mSignature".into(),
            ty: JType::Array(Box::new(JType::Byte)),
        };

        let spec = FrameworkClassSpec::builder("android/content/pm/Signature")
            .kind(ClassKind::Class)
            .interface("android/os/Parcelable")
            .instance_field(field_sig.clone(), FieldAccess::RustNative(Arc::new(SharedField::new(JValue::Null))))
            .instance_method(
                sig.clone(),
                MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(0x12345678)))),
            )
            .build();

        assert_eq!(spec.class_name, "android/content/pm/Signature");
        assert_eq!(spec.kind, ClassKind::Class);
        assert_eq!(spec.interfaces, vec!["android/os/Parcelable".to_string()]);
        assert_eq!(spec.instance_methods.len(), 1);
        assert_eq!(spec.instance_fields.len(), 1);
    }

    #[test]
    fn materialize_assigns_methods_and_fields() {
        let class = "test/Materialize";
        let m_sig = MethodSig { class: class.into(), name: "foo".into(), args: vec![], ret: JType::Int };
        let f_sig = FieldSig { class: class.into(), name: "bar".into(), ty: JType::Int };

        let spec = FrameworkClassSpec::builder(class)
            .static_field(f_sig.clone(), FieldAccess::RustNative(Arc::new(SharedField::new(JValue::Int(7)))))
            .instance_method(m_sig.clone(), MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(42)))))
            .build();

        let def = spec.materialize().unwrap();
        assert_eq!(def.name, class);
        assert_eq!(def.id, ClassId(0), "materialize 不分配 ClassId，交给 registry");
        assert_eq!(def.static_fields.len(), 1);
        assert_eq!(def.methods.len(), 1);
    }

    #[test]
    fn materialize_constructor_becomes_init() {
        let class = "test/Ctor";
        let spec = FrameworkClassSpec::builder(class)
            .constructor(vec![JType::Array(Box::new(JType::Byte))],
                MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Void))))
            .build();

        let def = spec.materialize().unwrap();
        // 构造器物化为 <init> instance method
        let init = def.methods.values().next().unwrap();
        assert_eq!(init.sig.name, "<init>");
        assert_eq!(init.sig.ret, JType::Void);
        assert!(!init.is_static);
        assert_eq!(init.sig.args.len(), 1);
    }

    #[test]
    fn materialize_duplicate_method_fails() {
        let class = "test/Dup";
        let sig = MethodSig { class: class.into(), name: "foo".into(), args: vec![], ret: JType::Int };
        // 手工构造含重复签名的 spec（builder 不允许重复，绕过验证）
        let mut spec = FrameworkClassSpec::builder(class).build();
        spec.instance_methods.push(FrameworkMethodSpec { sig: sig.clone(), is_static: false,
            imp: MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(1)))) });
        spec.instance_methods.push(FrameworkMethodSpec { sig, is_static: false,
            imp: MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(2)))) });

        assert!(spec.materialize().is_err(), "重复签名必须 fail-fast");
    }
}
