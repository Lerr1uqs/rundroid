//! JNI method / field 统一分发。
//!
//! 所有 method 调用和 field 访问都通过此模块的 dispatch 函数完成。
//! 无论 handler 是 Rust-native 还是 Python-shim，
//! 查找和分派走同一条主线——先查 registry，再按实现类型分发。

use crate::args::JniArgs;
use crate::error::JniError;
use crate::field::FieldAccess;
use crate::refs::RefTable;
use crate::registry::JniRegistry;
use crate::types::{FieldSig, JValue, MethodSig};
use std::sync::Arc;

/// Method 实现来源。
///
/// 与 `FieldAccess` 对称，区分 Rust-native 和 Python-shim 两种 handler。
pub enum MethodImpl {
    /// Rust 侧实现的 method handler。
    RustNative(Arc<dyn Fn(&JniArgs) -> Result<JValue, JniError> + Send + Sync>),
    /// Python shim 实现的 method（仅存 ID，由 bridge 回调）。
    PythonShim(u64),
}

impl std::fmt::Debug for MethodImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MethodImpl::RustNative(_) => write!(f, "RustNative(..)"),
            MethodImpl::PythonShim(id) => write!(f, "PythonShim({id})"),
        }
    }
}

impl Clone for MethodImpl {
    fn clone(&self) -> Self {
        match self {
            MethodImpl::RustNative(h) => MethodImpl::RustNative(Arc::clone(h)),
            MethodImpl::PythonShim(id) => MethodImpl::PythonShim(*id),
        }
    }
}

// ============================================================================
// Method dispatch
// ============================================================================

/// 分发 instance method 调用。
///
/// 从 registry 查找对应 method（instance），检查参数数量匹配，
/// 按 `MethodImpl` 分发到具体 handler。
pub(crate) fn dispatch_call(
    registry: &JniRegistry,
    sig: &MethodSig,
    args: &JniArgs,
    _refs: &mut RefTable,
) -> Result<JValue, JniError> {
    let cls = registry.classes.get(&sig.class)
        .ok_or_else(|| JniError::ClassNotFound(sig.class.clone()))?;

    let method = cls.methods.get(sig)
        .ok_or_else(|| JniError::MethodNotFound(sig.clone()))?;

    if method.is_static {
        return Err(JniError::StaticOnly(sig.to_string()));
    }

    dispatch_method_impl(&method.imp, args)
}

/// 分发 static method 调用。
pub(crate) fn dispatch_static_call(
    registry: &JniRegistry,
    sig: &MethodSig,
    args: &JniArgs,
    _refs: &mut RefTable,
) -> Result<JValue, JniError> {
    let cls = registry.classes.get(&sig.class)
        .ok_or_else(|| JniError::ClassNotFound(sig.class.clone()))?;

    let method = cls.static_methods.get(sig)
        .ok_or_else(|| JniError::MethodNotFound(sig.clone()))?;

    if !method.is_static {
        return Err(JniError::InstanceOnly(sig.to_string()));
    }

    dispatch_method_impl(&method.imp, args)
}

/// 执行 method handler（Rust-native 或 Python-shim）。
fn dispatch_method_impl(
    imp: &MethodImpl,
    args: &JniArgs,
) -> Result<JValue, JniError> {
    match imp {
        MethodImpl::RustNative(handler) => handler(args),
        MethodImpl::PythonShim(_id) => {
            // foundation 阶段 Python shim 还未接入完整回调链，
            // 此分支在 Python bridge 就绪后由 emulator/bindings/python 替换实现。
            Err(JniError::Internal(
                "Python shim method 尚未接入——需要 emulator/bindings/python 桥".to_string()
            ))
        }
    }
}

// ============================================================================
// Field dispatch
// ============================================================================

/// 分发 instance field get。
pub(crate) fn dispatch_field_get(
    registry: &JniRegistry,
    sig: &FieldSig,
) -> Result<JValue, JniError> {
    let cls = registry.classes.get(&sig.class)
        .ok_or_else(|| JniError::ClassNotFound(sig.class.clone()))?;

    let field = cls.fields.get(sig)
        .ok_or_else(|| JniError::FieldNotFound(sig.class.clone(), sig.name.clone()))?;

    if field.is_static {
        return Err(JniError::StaticOnly(format!("{sig}")));
    }

    dispatch_field_access_get(&field.access)
}

/// 分发 instance field set。
///
/// `RustFieldHandler` 使用内部可变性，所以这里只需要 `&JniRegistry`。
pub(crate) fn dispatch_field_set(
    registry: &JniRegistry,
    sig: &FieldSig,
    val: JValue,
) -> Result<(), JniError> {
    let cls = registry.classes.get(&sig.class)
        .ok_or_else(|| JniError::ClassNotFound(sig.class.clone()))?;

    let field = cls.fields.get(sig)
        .ok_or_else(|| JniError::FieldNotFound(sig.class.clone(), sig.name.clone()))?;

    if field.is_static {
        return Err(JniError::StaticOnly(format!("{sig}")));
    }

    dispatch_field_access_set(&field.access, val)
}

/// 分发 static field get。
pub(crate) fn dispatch_static_field_get(
    registry: &JniRegistry,
    sig: &FieldSig,
) -> Result<JValue, JniError> {
    let cls = registry.classes.get(&sig.class)
        .ok_or_else(|| JniError::ClassNotFound(sig.class.clone()))?;

    let field = cls.static_fields.get(sig)
        .ok_or_else(|| JniError::FieldNotFound(sig.class.clone(), sig.name.clone()))?;

    if !field.is_static {
        return Err(JniError::InstanceOnly(format!("{sig}")));
    }

    dispatch_field_access_get(&field.access)
}

/// 分发 static field set。
pub(crate) fn dispatch_static_field_set(
    registry: &JniRegistry,
    sig: &FieldSig,
    val: JValue,
) -> Result<(), JniError> {
    let cls = registry.classes.get(&sig.class)
        .ok_or_else(|| JniError::ClassNotFound(sig.class.clone()))?;

    let field = cls.static_fields.get(sig)
        .ok_or_else(|| JniError::FieldNotFound(sig.class.clone(), sig.name.clone()))?;

    if !field.is_static {
        return Err(JniError::InstanceOnly(format!("{sig}")));
    }

    dispatch_field_access_set(&field.access, val)
}

fn dispatch_field_access_get(access: &FieldAccess) -> Result<JValue, JniError> {
    match access {
        FieldAccess::RustNative(handler) => Ok(handler.get()),
        FieldAccess::PythonShim(_id) => {
            Err(JniError::Internal(
                "Python shim field 尚未接入——需要 emulator/bindings/python 桥".to_string()
            ))
        }
    }
}

fn dispatch_field_access_set(access: &FieldAccess, val: JValue) -> Result<(), JniError> {
    match access {
        FieldAccess::RustNative(handler) => {
            // handler 使用内部可变性（Mutex），共享引用即可 set。
            handler.set(val)
        }
        FieldAccess::PythonShim(_id) => {
            Err(JniError::Internal(
                "Python shim field 尚未接入——需要 emulator/bindings/python 桥".to_string()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::JniArgs;
    use crate::class::JClassDef;
    use crate::field::{FieldAccess, RustFieldHandler, SharedField};
    use crate::types::{ClassId, FieldSig, JType, JValue, MethodSig};
    use std::sync::Arc;

    fn make_sig(class: &str, name: &str) -> MethodSig {
        MethodSig {
            class: class.to_string(),
            name: name.to_string(),
            args: vec![],
            ret: JType::Int,
        }
    }

    #[test]
    fn dispatch_rust_native_method() {
        let mut registry = JniRegistry::new();
        let class_name = "test/Class";

        let sig = make_sig(class_name, "getValue");

        let mut class_def = JClassDef::new(ClassId(0), class_name.into());
        class_def.add_method(sig.clone(), false, MethodImpl::RustNative(Arc::new(|_args| {
            Ok(JValue::Int(42))
        }))).unwrap();
        registry.register_class(class_def).unwrap();

        let args = JniArgs::new();
        let mut refs = RefTable::new();
        let result = dispatch_call(&registry, &sig, &args, &mut refs).unwrap();
        assert_eq!(result, JValue::Int(42));
    }

    #[test]
    fn dispatch_method_not_found() {
        let registry = JniRegistry::new();
        let sig = make_sig("test/Class", "missingMethod");
        let args = JniArgs::new();
        let mut refs = RefTable::new();
        let result = dispatch_call(&registry, &sig, &args, &mut refs);
        assert!(matches!(result, Err(JniError::ClassNotFound(_))));
    }

    #[test]
    fn duplicate_method_fails() {
        let class_name = "test/Class";
        let sig = make_sig(class_name, "foo");

        let mut class_def = JClassDef::new(ClassId(0), class_name.into());
        class_def.add_method(sig.clone(), false, MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(1))))).unwrap();
        let result = class_def.add_method(sig.clone(), false, MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(2)))));
        assert!(result.is_err());
    }

    #[test]
    fn dispatch_field_simple() {
        let mut registry = JniRegistry::new();
        let class_name = "test/FieldClass";
        let field_sig = FieldSig {
            class: class_name.into(),
            name: "count".into(),
            ty: JType::Int,
        };

        let field_access = FieldAccess::RustNative(Arc::new(SharedField::new(JValue::Int(100))));
        let mut class_def = JClassDef::new(ClassId(0), class_name.into());
        class_def.add_field(field_sig.clone(), false, field_access).unwrap();
        registry.register_class(class_def).unwrap();

        let val = dispatch_field_get(&registry, &field_sig).unwrap();
        assert_eq!(val, JValue::Int(100));
    }

    // ============================================================================
    // 联动测试：method 与 field 共享状态
    // ============================================================================

    /// 模拟 `android/content/pm/Signature` 类：
    /// - 一个 static field `signatureCount:I`，初始 = 0
    /// - 一个 static method `incrementAndGet()I`，每次调用把 field +1 然后返回新值
    /// - method 和 field 通过同一个 `Arc<SharedField>` 共享状态
    #[test]
    fn field_and_method_share_state_static() {
        let class_name = "android/content/pm/Signature";

        // 1. 创建共享的 field handler（method handler 也能读写它）
        let count_field: Arc<SharedField> = Arc::new(SharedField::new(JValue::Int(0)));

        // 2. 构造 static field signatureCount:I
        let field_sig = FieldSig {
            class: class_name.into(),
            name: "signatureCount".into(),
            ty: JType::Int,
        };
        let field_access = FieldAccess::RustNative(count_field.clone());

        // 3. 构造 static method incrementAndGet()I
        let method_sig = MethodSig {
            class: class_name.into(),
            name: "incrementAndGet".into(),
            args: vec![],
            ret: JType::Int,
        };
        // method handler 通过 clone 的 Arc 直接读写 field
        let count_field_for_method = count_field.clone();
        let method_impl = MethodImpl::RustNative(Arc::new(move |_args: &JniArgs| {
            let val = count_field_for_method.get();
            if let JValue::Int(n) = val {
                let new_val = JValue::Int(n + 1);
                count_field_for_method.set(new_val.clone())?;
                Ok(new_val)
            } else {
                Err(JniError::Internal("类型错误".into()))
            }
        }));

        // 4. 注册 class + field + method
        let mut registry = JniRegistry::new();
        let mut class_def = JClassDef::new(ClassId(0), class_name.into());
        class_def.add_field(field_sig.clone(), true, field_access).unwrap();
        class_def.add_method(method_sig.clone(), true, method_impl).unwrap();
        registry.register_class(class_def).unwrap();

        // 5. 验证初始值
        let v = registry.dispatch_static_field_get(&field_sig).unwrap();
        assert_eq!(v, JValue::Int(0));

        // 6. 第一次调用 incrementAndGet → 返回 1
        let args = JniArgs::new();
        let mut refs = RefTable::new();
        let result = registry.dispatch_static(&method_sig, &args, &mut refs).unwrap();
        assert_eq!(result, JValue::Int(1));

        // 7. 再读 field → 值已变成 1
        let v = registry.dispatch_static_field_get(&field_sig).unwrap();
        assert_eq!(v, JValue::Int(1));

        // 8. 再调用 incrementAndGet 三次 → 2, 3, 4
        for expected in [2, 3, 4] {
            let result = registry.dispatch_static(&method_sig, &args, &mut refs).unwrap();
            assert_eq!(result, JValue::Int(expected));
        }

        // 9. 最终 field 值 = 4
        assert_eq!(registry.dispatch_static_field_get(&field_sig).unwrap(), JValue::Int(4));
    }

    /// 模拟一个带 instance field 的类：
    /// - instance field `counter:I`，初始 = 0
    /// - instance method `getAndIncrement()I`，返回当前值然后 +1
    #[test]
    fn field_and_method_share_state_instance() {
        let class_name = "java/util/concurrent/atomic/AtomicInt";

        let counter: Arc<SharedField> = Arc::new(SharedField::new(JValue::Int(0)));

        let field_sig = FieldSig {
            class: class_name.into(),
            name: "counter".into(),
            ty: JType::Int,
        };
        let field_access = FieldAccess::RustNative(counter.clone());

        let method_sig = MethodSig {
            class: class_name.into(),
            name: "getAndIncrement".into(),
            args: vec![],
            ret: JType::Int,
        };
        let counter_for_method = counter.clone();
        let method_impl = MethodImpl::RustNative(Arc::new(move |_args: &JniArgs| {
            let current = counter_for_method.get();
            if let JValue::Int(n) = &current {
                counter_for_method.set(JValue::Int(n + 1))?;
                Ok(current)
            } else {
                Err(JniError::Internal("类型错误".into()))
            }
        }));

        let mut registry = JniRegistry::new();
        let mut class_def = JClassDef::new(ClassId(0), class_name.into());
        class_def.add_field(field_sig.clone(), false, field_access).unwrap();
        class_def.add_method(method_sig.clone(), false, method_impl).unwrap();
        registry.register_class(class_def).unwrap();

        // 初始 counter = 0
        assert_eq!(registry.dispatch_field_get(&field_sig).unwrap(), JValue::Int(0));

        let args = JniArgs::new();
        let mut refs = RefTable::new();

        // getAndIncrement: 返回 0, counter 变 1
        let r = registry.dispatch_call(&method_sig, &args, &mut refs).unwrap();
        assert_eq!(r, JValue::Int(0));
        assert_eq!(registry.dispatch_field_get(&field_sig).unwrap(), JValue::Int(1));

        // getAndIncrement: 返回 1, counter 变 2
        let r = registry.dispatch_call(&method_sig, &args, &mut refs).unwrap();
        assert_eq!(r, JValue::Int(1));
        assert_eq!(registry.dispatch_field_get(&field_sig).unwrap(), JValue::Int(2));
    }

    /// 模拟更真实的 Android shim 类：`android/content/pm/Signature`
    /// - static field `CREATOR:I` = 0xABCD
    /// - instance method `hashCode()I` → 返回固定值 0x12345678
    /// - instance method `describeContents()I` → 返回 0
    /// - 验证多次 dispatch 结果一致
    #[test]
    fn signature_class_full_shim() {
        let class_name = "android/content/pm/Signature";

        // static field: CREATOR:I = 0xABCD
        let creator_sig = FieldSig {
            class: class_name.into(),
            name: "CREATOR".into(),
            ty: JType::Int,
        };
        let creator = FieldAccess::RustNative(Arc::new(SharedField::new(JValue::Int(0xABCD))));

        // instance field: mHash:I = 0x12345678
        let hash_sig = FieldSig {
            class: class_name.into(),
            name: "mHash".into(),
            ty: JType::Int,
        };
        let hash_val: Arc<SharedField> = Arc::new(SharedField::new(JValue::Int(0x12345678)));

        // hashCode()I → 返回 mHash 的值
        let hashcode_sig = MethodSig {
            class: class_name.into(),
            name: "hashCode".into(),
            args: vec![],
            ret: JType::Int,
        };
        let hash_val_for_hashcode = hash_val.clone();
        let hashcode_impl = MethodImpl::RustNative(Arc::new(move |_args: &JniArgs| {
            Ok(hash_val_for_hashcode.get())
        }));

        // describeContents()I → 返回 0
        let describe_sig = MethodSig {
            class: class_name.into(),
            name: "describeContents".into(),
            args: vec![],
            ret: JType::Int,
        };
        let describe_impl = MethodImpl::RustNative(Arc::new(|_args| Ok(JValue::Int(0))));

        // setHash(I)V → 设置 mHash 的值
        let set_hash_sig = MethodSig {
            class: class_name.into(),
            name: "setHash".into(),
            args: vec![JType::Int],
            ret: JType::Void,
        };
        let hash_val_for_set = hash_val.clone();
        let set_hash_impl = MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let n = args.int_at(0)?;
            hash_val_for_set.set(JValue::Int(n))?;
            Ok(JValue::Void)
        }));

        // 注册全部
        let mut registry = JniRegistry::new();
        let mut class_def = JClassDef::new(ClassId(0), class_name.into());
        class_def.add_field(creator_sig.clone(), true, creator).unwrap();
        class_def.add_field(hash_sig.clone(), false, FieldAccess::RustNative(hash_val)).unwrap();
        class_def.add_method(hashcode_sig.clone(), false, hashcode_impl).unwrap();
        class_def.add_method(describe_sig.clone(), false, describe_impl).unwrap();
        class_def.add_method(set_hash_sig.clone(), false, set_hash_impl).unwrap();
        registry.register_class(class_def).unwrap();

        let mut refs = RefTable::new();

        // 1. 读 static field CREATOR
        assert_eq!(
            registry.dispatch_static_field_get(&creator_sig).unwrap(),
            JValue::Int(0xABCD)
        );

        // 2. hashCode() → 0x12345678
        assert_eq!(
            registry.dispatch_call(&hashcode_sig, &JniArgs::new(), &mut refs).unwrap(),
            JValue::Int(0x12345678)
        );

        // 3. describeContents() → 0
        assert_eq!(
            registry.dispatch_call(&describe_sig, &JniArgs::new(), &mut refs).unwrap(),
            JValue::Int(0)
        );

        // 4. setHash(999) → mHash 变成 999
        registry.dispatch_call(
            &set_hash_sig,
            &JniArgs::from_vec(vec![JValue::Int(999)]),
            &mut refs,
        ).unwrap();
        assert_eq!(
            registry.dispatch_field_get(&hash_sig).unwrap(),
            JValue::Int(999)
        );

        // 5. hashCode() → 现在返回 999
        assert_eq!(
            registry.dispatch_call(&hashcode_sig, &JniArgs::new(), &mut refs).unwrap(),
            JValue::Int(999)
        );

        // 6. 读 static field 仍然不变
        assert_eq!(
            registry.dispatch_static_field_get(&creator_sig).unwrap(),
            JValue::Int(0xABCD)
        );
    }

    /// 验证 ClassBuilder 链式 API 构建完整 class（field + method 一步注册）。
    #[test]
    fn class_builder_field_and_method() {
        let mut registry = JniRegistry::new();
        let class_name = "test/Counter";

        let counter: Arc<SharedField> = Arc::new(SharedField::new(JValue::Int(10)));

        let counter_for_method = counter.clone();
        registry.build_class(class_name)
            .add_field("count:I", true, FieldAccess::RustNative(counter))
            .add_method("increment()I", true, MethodImpl::RustNative(Arc::new(move |_| {
                let current = counter_for_method.get();
                if let JValue::Int(n) = &current {
                    counter_for_method.set(JValue::Int(n + 1))?;
                    Ok(current)
                } else {
                    Err(JniError::Internal("类型错误".into()))
                }
            })))
            .finish()
            .unwrap();

        let field_sig = FieldSig { class: class_name.into(), name: "count".into(), ty: JType::Int };
        let method_sig = MethodSig { class: class_name.into(), name: "increment".into(), args: vec![], ret: JType::Int };

        let mut refs = RefTable::new();

        let r = registry.dispatch_static(&method_sig, &JniArgs::new(), &mut refs).unwrap();
        assert_eq!(r, JValue::Int(10));
        assert_eq!(registry.dispatch_static_field_get(&field_sig).unwrap(), JValue::Int(11));
    }
}
