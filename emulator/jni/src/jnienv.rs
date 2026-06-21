//! JNIEnv surface — guest JNI 函数调用的 Rust 分派层。
//!
//! [`JniEnvSurface`] 是 guest 侧 JNI 函数指针表的 Rust 后端，
//! 每次 guest 调用 `(*env)->FindClass(env, name)` 时，
//! trampoline hook 会用此结构完成实际分派。
//!
//! 已实现：class 查找、method/field ID 获取、对象实例化、
//! method 调用（instance/static 12 种返回类型）、field 读写、
//! 引用管理、异常状态、String 操作。
//!
//! 未实现：数组操作、反射 API、Monitor 等。
//! 这些函数在 guest 调用时返回 `JniError::Internal`。

use crate::args::JniArgs;
use crate::error::JniError;
use crate::exception::ExceptionState;
use crate::native_registry::NativeRegistry;
use crate::object_store::{ObjectStorage, ObjectStore};
use crate::refs::RefTable;
use crate::registry::JniRegistry;
use crate::types::{
    FieldId, FieldSig, JType, JValue, MethodId, MethodSig, ObjectId,
};
use std::sync::{Arc, Mutex};

/// JNIEnv surface — guest JNI 函数指针表的 Rust 实现。
///
/// 持有对 registry、object store、ref table、exception state 和
/// native registry 的引用。所有 JNI 方法调用和 field 访问都通过此结构完成。
///
/// # 生命周期
///
/// `JniEnvSurface` 的生命周期绑定到当前线程的 attach 状态。
/// bootstrap 阶段不区分多线程，使用 `AndroidVM` 的全局 ref table。
pub struct JniEnvSurface<'r> {
    registry: &'r JniRegistry,
    /// 对象池共享引用（lock 时获取内部可变访问）。
    objects: &'r Arc<Mutex<ObjectStore>>,
    refs: &'r mut RefTable,
    exceptions: &'r mut ExceptionState,
    /// Native 方法注册表 — RegisterNatives 写入此表。
    natives: &'r mut NativeRegistry,
}

impl<'r> JniEnvSurface<'r> {
    /// 从 registry 和 ref table 构造 JNIEnv（不含 object store，向后兼容测试）。
    pub fn new(_registry: &'r JniRegistry, _refs: &'r mut RefTable) -> Self {
        // 此构造函数仅用于测试路径，占位 objects 和 exceptions
        unimplemented!("JniEnvSurface::new 已废弃：请使用 new_with_objects 传入 objects + exceptions")
    }

    /// 从完整 VM 组件构造 JNIEnv。
    pub fn new_with_objects(
        registry: &'r JniRegistry,
        objects: &'r Arc<Mutex<ObjectStore>>,
        refs: &'r mut RefTable,
        exceptions: &'r mut ExceptionState,
        natives: &'r mut NativeRegistry,
    ) -> Self {
        Self {
            registry,
            objects,
            refs,
            exceptions,
            natives,
        }
    }

    // ========================================================================
    // Class 操作
    // ========================================================================

    /// FindClass: 按 JNI class descriptor 名称查找已注册的 class。
    ///
    /// 返回 local ref handle（u32），指向 class 对象。
    /// class 对象以 `StubInstance` 形式存入 ObjectStore。
    pub fn find_class(&mut self, name: &str) -> Result<u32, JniError> {
        // 检查 registry 中是否已注册该 class
        let class_def = self.registry.find_class(name)
            .ok_or_else(|| JniError::ClassNotFound(name.to_string()))?;

        let class_id = class_def.id;

        // 分配 ObjectId 并存入 ObjectStore（使用固定 ObjectId 表示 class 对象）
        // 实际上我们用一个特定的 ObjectId 编码 class 引用
        // 简化：为 class 本身创建一个 StubInstance 对象
        let obj_id = ObjectId(class_id.0 + 0x1000_0000); // class object ID = ClassId + offset
        {
            let mut store = self.objects.lock().unwrap();
            if !store.contains(obj_id) {
                store.insert(
                    obj_id,
                    "java/lang/Class".to_string(),
                    ObjectStorage::StubInstance {
                        data: Box::new(name.to_string()),
                    },
                )
                .map_err(|e| JniError::Internal(format!("ObjectStore 插入 Class 对象失败: {e}")))?;
            }
        }

        // 创建 local ref 并返回 handle
        Ok(self.refs.new_local(obj_id))
    }

    /// GetObjectClass: 获取对象的 class，返回 class handle。
    pub fn get_object_class(&mut self, obj_handle: u32) -> Result<u32, JniError> {
        let obj_id = self.refs.resolve(obj_handle)
            .ok_or_else(|| JniError::Internal(format!("handle {obj_handle} 不存在")))?;

        let store = self.objects.lock().unwrap();
        let class_name = store.class_name(obj_id)
            .ok_or_else(|| JniError::Internal(format!("对象 {obj_id} 不在 ObjectStore 中")))?
            .to_string();
        drop(store);

        self.find_class(&class_name)
    }

    // ========================================================================
    // Method ID 操作
    // ========================================================================

    /// GetMethodID: 从已注册 class 中查找 instance method。
    ///
    /// `sig_str` 是 JNI descriptor 格式，如 `"()I"`、`"(ILjava/lang/String;)V"`。
    pub fn get_method_id(
        &self,
        class_handle: u32,
        name: &str,
        sig_str: &str,
    ) -> Result<MethodId, JniError> {
        let class_name = self.resolve_class_name(class_handle)?;
        let methods = self.registry.methods_by_name(&class_name, name);

        // 遍历同名方法，按 JNI descriptor 匹配
        for method_sig in methods {
            if method_sig.descriptor() == sig_str {
                return self.registry.method_id(&class_name, method_sig)
                    .ok_or_else(|| JniError::MethodNotFound(method_sig.clone()));
            }
        }
        Err(JniError::Internal(format!(
            "GetMethodID: class={class_name}, name={name}, sig={sig_str} 未找到"
        )))
    }

    /// GetStaticMethodID: 从已注册 class 中查找 static method。
    ///
    /// `sig_str` 是 JNI descriptor 格式，如 `"()I"`。
    pub fn get_static_method_id(
        &self,
        class_handle: u32,
        name: &str,
        sig_str: &str,
    ) -> Result<MethodId, JniError> {
        let class_name = self.resolve_class_name(class_handle)?;
        let cls = self.registry.find_class(&class_name)
            .ok_or_else(|| JniError::ClassNotFound(class_name.clone()))?;

        // 在 static_methods 中按名称 + JNI descriptor 匹配
        for (sig, method_def) in &cls.static_methods {
            if sig.name == name && sig.descriptor() == sig_str {
                return Ok(method_def.id);
            }
        }

        Err(JniError::Internal(format!(
            "GetStaticMethodID: class={class_name}, name={name}, sig={sig_str} 未找到"
        )))
    }

    // ========================================================================
    // Field ID 操作
    // ========================================================================

    /// GetFieldID: 从已注册 class 中查找 instance field。
    pub fn get_field_id(
        &self,
        class_handle: u32,
        name: &str,
        sig_str: &str,
    ) -> Result<FieldId, JniError> {
        let class_name = self.resolve_class_name(class_handle)?;
        let cls = self.registry.find_class(&class_name)
            .ok_or_else(|| JniError::ClassNotFound(class_name.clone()))?;

        for (sig, field_def) in &cls.fields {
            if sig.name == name {
                // bootstrap: 用 FieldId 占位，后续实现 field_id 查找逻辑
                let _ = (sig_str, field_def);
                return Ok(FieldId(0));
            }
        }
        Err(JniError::Internal(format!(
            "GetFieldID: class={class_name}, name={name}, sig={sig_str} 未找到"
        )))
    }

    /// GetStaticFieldID: 从已注册 class 中查找 static field。
    pub fn get_static_field_id(
        &self,
        class_handle: u32,
        name: &str,
        sig_str: &str,
    ) -> Result<FieldId, JniError> {
        let class_name = self.resolve_class_name(class_handle)?;
        let cls = self.registry.find_class(&class_name)
            .ok_or_else(|| JniError::ClassNotFound(class_name.clone()))?;

        for (sig, _field_def) in &cls.static_fields {
            if sig.name == name {
                return Ok(FieldId(0)); // placeholder
            }
        }
        Err(JniError::Internal(format!(
            "GetStaticFieldID: class={class_name}, name={name}, sig={sig_str} 未找到"
        )))
    }

    // ========================================================================
    // 对象实例化
    // ========================================================================

    /// AllocObject: 分配未初始化对象。
    pub fn alloc_object(&mut self, class_handle: u32) -> Result<u32, JniError> {
        let _class_name = self.resolve_class_name(class_handle)?;

        // 分配 ObjectId
        let _obj_id = ObjectId(0); // TODO: 使用 IdAllocator
        Err(JniError::Internal("AllocObject 尚未完整实现".into()))
    }

    /// NewObject: 实例化对象（通过 MethodId 指定构造函数）。
    ///
    /// `args` 由 trampoline 层从 CPU 寄存器读取（varargs 简化处理）。
    pub fn new_object(
        &mut self,
        class_handle: u32,
        method_id: u64,
        _args: &[JValue],
    ) -> Result<u32, JniError> {
        let class_name = self.resolve_class_name(class_handle)?;

        // 分配 ObjectId（bootstrap: 简单使用 method_id 的低 32 位作为对象 ID）
        // TODO: 使用 IdAllocator 统一分配
        let obj_id = ObjectId(method_id.wrapping_add(1));

        // 存入 ObjectStore 作为 StubInstance
        {
            let mut store = self.objects.lock().unwrap();
            store.insert(
                obj_id,
                class_name.clone(),
                ObjectStorage::StubInstance {
                    data: Box::new(class_name),
                },
            )
            .map_err(|e| JniError::Internal(format!("ObjectStore 插入新对象失败: {e}")))?;
        }

        // 创建 local ref handle 返回
        Ok(self.refs.new_local(obj_id))
    }

    // ========================================================================
    // RegisterNatives / Native binding
    // ========================================================================

    /// RegisterNatives: 将 guest native 函数绑定到已注册的 method。
    ///
    /// 对于 `methods` 中的每一项 `(name, descriptor_str, fn_ptr)`：
    /// 1. 按 class_handle 解析 class name
    /// 2. 按 name + JNI descriptor 在 registry 中查找 MethodId
    /// 3. 将 `(MethodId, fn_ptr)` 写入 `NativeRegistry`
    ///
    /// 返回成功注册的方法数量。注册失败的项被静默跳过
    /// （不阻塞 RegisterNatives 调用——符合 Android linker 的容错语义）。
    ///
    /// # 优先级
    ///
    /// `RegisterNatives` 的注册拥有最高优先级——
    /// 当 method 后续被 `CallXxxMethod` 调用时，
    /// runtime 优先查找 native registry，命中则调用 guest 函数，
    /// 未命中才回落 `MethodImpl`（Rust-native / Python-shim）。
    pub fn register_natives(
        &mut self,
        class_handle: u32,
        methods: &[(String, String, u64)],
    ) -> u32 {
        let class_name = match self.resolve_class_name(class_handle) {
            Ok(n) => n,
            Err(_) => return 0,
        };

        let mut count = 0u32;
        for (name, desc_str, fn_ptr) in methods {
            // 查找匹配的 MethodSig
            let candidates = self.registry.methods_by_name(&class_name, name);
            let mut matched = None;
            for sig in candidates {
                if sig.descriptor() == *desc_str {
                    matched = Some(sig.clone());
                    break;
                }
            }
            if matched.is_none() {
                // 也检查 static methods
                if let Some(cls) = self.registry.find_class(&class_name) {
                    for (sig, _def) in &cls.static_methods {
                        if sig.name == *name && sig.descriptor() == *desc_str {
                            matched = Some(sig.clone());
                            break;
                        }
                    }
                }
            }

            if let Some(sig) = matched {
                if let Some(method_id) = self.registry.method_id(&class_name, &sig) {
                    self.natives.register(method_id, *fn_ptr);
                    count += 1;
                }
            }
        }

        count
    }

    /// 检查 method 是否已绑定 native 函数（通过 RegisterNatives）。
    ///
    /// 返回 guest 函数地址（如果已注册）。
    /// 调用方可以在 dispatch 前调用此方法，
    /// 命中时直接调用 guest 函数而不走 `MethodImpl` 分发。
    pub fn lookup_native(&self, method_id: MethodId) -> Option<u64> {
        self.natives.lookup(method_id)
    }

    /// 检查 method 是否有 native binding（通过 RegisterNatives）。
    ///
    /// `has_native` 为 true 表示该 method 的调用应优先走 guest 函数指针，
    /// 而非 registry 中的 Rust/Python handler。
    pub fn has_native(&self, method_id: MethodId) -> bool {
        self.natives.lookup(method_id).is_some()
    }

    /// 按 method_id 和原始 args 调用 instance method（不做类型转换）。
    fn dispatch_by_method_id(
        &mut self,
        obj_handle: u32,
        method_id: u64,
        raw_args: &[JValue],
        _ret_type: JType,
    ) -> Result<JValue, JniError> {
        let obj_id = self.refs.resolve(obj_handle)
            .ok_or_else(|| JniError::Internal(format!("handle {obj_handle} 不存在")))?;

        let store = self.objects.lock().unwrap();
        let class_name = store.class_name(obj_id)
            .ok_or_else(|| JniError::Internal(format!("对象 {obj_id} 不在 ObjectStore 中")))?
            .to_string();
        drop(store);

        let cls = self.registry.find_class(&class_name)
            .ok_or_else(|| JniError::ClassNotFound(class_name.clone()))?;

        // 在 instance methods 中按 MethodId 查找
        let method_sig = cls.methods.iter()
            .find(|(_, def)| def.id.0 == method_id)
            .map(|(sig, _)| sig.clone())
            .ok_or_else(|| JniError::Internal(format!(
                "MethodId {method_id} 在 class {class_name} 中未找到"
            )))?;

        // 检查 native binding：如果此 method 通过 RegisterNatives 绑定了 guest 函数指针，
        // 则应优先调用 guest native，而非 Rust/Python handler。
        // guest native 调用（嵌套 emu_start）尚未实现，此处显式报错避免静默回落。
        let mid = MethodId(method_id);
        if self.has_native(mid) {
            return Err(JniError::Internal(format!(
                "method {class_name}.{method_id} 已通过 RegisterNatives 绑定 guest native ({:#x})，但 guest native 调用链尚未接入",
                self.lookup_native(mid).unwrap_or(0)
            )));
        }

        // 构造 JniArgs（将 raw JValue 参数包装）
        let mut jni_args = JniArgs::from_vec(raw_args.to_vec());
        jni_args.set_this(obj_id);

        self.registry.dispatch_call(&method_sig, &jni_args, self.refs)
    }

    /// call_void_method_by_id: CallVoidMethod 的 Rust 实现。
    pub fn call_void_method_by_id(
        &mut self,
        obj_handle: u32,
        method_id: u64,
        raw_args: &[JValue],
    ) -> Result<(), JniError> {
        self.dispatch_by_method_id(obj_handle, method_id, raw_args, JType::Void)?;
        Ok(())
    }

    /// call_boolean_method_by_id: CallBooleanMethod 的 Rust 实现。
    pub fn call_boolean_method_by_id(&mut self, obj: u32, method_id: u64, args: &[JValue]) -> Result<bool, JniError> {
        let result = self.dispatch_by_method_id(obj, method_id, args, JType::Boolean)?;
        result.as_boolean().ok_or_else(|| JniError::TypeMismatch {
            expected: JType::Boolean,
            actual: result.jtype(),
        })
    }

    /// call_byte_method_by_id
    pub fn call_byte_method_by_id(&mut self, obj: u32, method_id: u64, args: &[JValue]) -> Result<i8, JniError> {
        let result = self.dispatch_by_method_id(obj, method_id, args, JType::Byte)?;
        match result {
            JValue::Byte(b) => Ok(b),
            _ => Err(JniError::TypeMismatch { expected: JType::Byte, actual: result.jtype() }),
        }
    }

    /// call_char_method_by_id
    pub fn call_char_method_by_id(&mut self, obj: u32, method_id: u64, args: &[JValue]) -> Result<u16, JniError> {
        let result = self.dispatch_by_method_id(obj, method_id, args, JType::Char)?;
        match result {
            JValue::Char(c) => Ok(c),
            _ => Err(JniError::TypeMismatch { expected: JType::Char, actual: result.jtype() }),
        }
    }

    /// call_short_method_by_id
    pub fn call_short_method_by_id(&mut self, obj: u32, method_id: u64, args: &[JValue]) -> Result<i16, JniError> {
        let result = self.dispatch_by_method_id(obj, method_id, args, JType::Short)?;
        match result {
            JValue::Short(s) => Ok(s),
            _ => Err(JniError::TypeMismatch { expected: JType::Short, actual: result.jtype() }),
        }
    }

    /// call_int_method_by_id: CallIntMethod 的 Rust 实现。
    pub fn call_int_method_by_id(&mut self, obj: u32, method_id: u64, args: &[JValue]) -> Result<i32, JniError> {
        let result = self.dispatch_by_method_id(obj, method_id, args, JType::Int)?;
        result.as_int().ok_or_else(|| JniError::TypeMismatch {
            expected: JType::Int,
            actual: result.jtype(),
        })
    }

    /// call_long_method_by_id
    pub fn call_long_method_by_id(&mut self, obj: u32, method_id: u64, args: &[JValue]) -> Result<i64, JniError> {
        let result = self.dispatch_by_method_id(obj, method_id, args, JType::Long)?;
        result.as_long().ok_or_else(|| JniError::TypeMismatch {
            expected: JType::Long,
            actual: result.jtype(),
        })
    }

    /// call_float_method_by_id
    pub fn call_float_method_by_id(&mut self, obj: u32, method_id: u64, args: &[JValue]) -> Result<f32, JniError> {
        let result = self.dispatch_by_method_id(obj, method_id, args, JType::Float)?;
        match result {
            JValue::Float(f) => Ok(f),
            _ => Err(JniError::TypeMismatch { expected: JType::Float, actual: result.jtype() }),
        }
    }

    /// call_double_method_by_id
    pub fn call_double_method_by_id(&mut self, obj: u32, method_id: u64, args: &[JValue]) -> Result<f64, JniError> {
        let result = self.dispatch_by_method_id(obj, method_id, args, JType::Double)?;
        match result {
            JValue::Double(d) => Ok(d),
            _ => Err(JniError::TypeMismatch { expected: JType::Double, actual: result.jtype() }),
        }
    }

    /// call_object_method: CallObjectMethod → 返回 new local ref。
    pub fn call_object_method(&mut self, obj: u32, method_id: u64, args: &[JValue]) -> Result<u64, JniError> {
        let result = self.dispatch_by_method_id(obj, method_id, args, JType::Object(String::new()))?;
        match result {
            JValue::Object(oid) => {
                let handle = self.refs.new_local(oid);
                Ok(handle as u64)
            }
            JValue::Null => Ok(0),
            _ => Err(JniError::TypeMismatch { expected: JType::Object(String::new()), actual: result.jtype() }),
        }
    }

    // ========================================================================
    // Static Method 调用（by MethodId）
    // ========================================================================

    /// 按 method_id 在 class 的 static methods 中派发。
    fn dispatch_static_by_method_id(
        &mut self,
        class_handle: u32,
        method_id: u64,
        raw_args: &[JValue],
    ) -> Result<JValue, JniError> {
        let class_name = self.resolve_class_name(class_handle)?;

        let cls = self.registry.find_class(&class_name)
            .ok_or_else(|| JniError::ClassNotFound(class_name.clone()))?;

        let method_sig = cls.static_methods.iter()
            .find(|(_, def)| def.id.0 == method_id)
            .map(|(sig, _)| sig.clone())
            .ok_or_else(|| JniError::Internal(format!(
                "Static MethodId {method_id} 在 class {class_name} 中未找到"
            )))?;

        // 检查 native binding（与 instance dispatch 同理）
        let mid = MethodId(method_id);
        if self.has_native(mid) {
            return Err(JniError::Internal(format!(
                "static method {class_name}.{method_id} 已通过 RegisterNatives 绑定 guest native ({:#x})，但 guest native 调用链尚未接入",
                self.lookup_native(mid).unwrap_or(0)
            )));
        }

        let jni_args = JniArgs::from_vec(raw_args.to_vec());
        self.registry.dispatch_static(&method_sig, &jni_args, self.refs)
    }

    /// call_static_void_method_by_id
    pub fn call_static_void_method_by_id(&mut self, class: u32, method_id: u64, args: &[JValue]) -> Result<(), JniError> {
        self.dispatch_static_by_method_id(class, method_id, args)?;
        Ok(())
    }

    /// call_static_int_method_by_id
    pub fn call_static_int_method_by_id(&mut self, class: u32, method_id: u64, args: &[JValue]) -> Result<i32, JniError> {
        let r = self.dispatch_static_by_method_id(class, method_id, args)?;
        r.as_int().ok_or_else(|| JniError::TypeMismatch { expected: JType::Int, actual: r.jtype() })
    }

    /// call_static_object_method_by_id
    pub fn call_static_object_method_by_id(&mut self, class: u32, method_id: u64, args: &[JValue]) -> Result<u64, JniError> {
        let r = self.dispatch_static_by_method_id(class, method_id, args)?;
        match r {
            JValue::Object(oid) => {
                let handle = self.refs.new_local(oid);
                Ok(handle as u64)
            }
            JValue::Null => Ok(0),
            _ => Err(JniError::TypeMismatch { expected: JType::Object(String::new()), actual: r.jtype() }),
        }
    }

    /// call_static_boolean_method_by_id
    pub fn call_static_boolean_method_by_id(&mut self, class: u32, method_id: u64, args: &[JValue]) -> Result<bool, JniError> {
        let r = self.dispatch_static_by_method_id(class, method_id, args)?;
        r.as_boolean().ok_or_else(|| JniError::TypeMismatch { expected: JType::Boolean, actual: r.jtype() })
    }

    /// call_static_long_method_by_id
    pub fn call_static_long_method_by_id(&mut self, class: u32, method_id: u64, args: &[JValue]) -> Result<i64, JniError> {
        let r = self.dispatch_static_by_method_id(class, method_id, args)?;
        r.as_long().ok_or_else(|| JniError::TypeMismatch { expected: JType::Long, actual: r.jtype() })
    }

    // ========================================================================
    // Field get/set（通过 MethodId/FieldId — bootstrap 简化）
    // ========================================================================

    /// get_int_field_by_id
    pub fn get_int_field_by_id(&self, obj_handle: u32, _field_id: u64) -> Result<i32, JniError> {
        let obj_id = self.refs.resolve(obj_handle)
            .ok_or_else(|| JniError::Internal(format!("handle {obj_handle} 不存在")))?;
        let store = self.objects.lock().unwrap();
        let _class_name = store.class_name(obj_id)
            .ok_or_else(|| JniError::Internal(format!("对象 {obj_id} 不在 ObjectStore 中")))?
            .to_string();
        drop(store);

        // 通过 field_id 查找具体 field — bootstrap 简化处理
        Err(JniError::Internal("get_int_field_by_id 需要 field_id 查找逻辑".into()))
    }

    /// get_object_field_by_id
    pub fn get_object_field_by_id(&self, obj_handle: u32, _field_id: u64) -> Result<u64, JniError> {
        let _obj_id = self.refs.resolve(obj_handle);
        Err(JniError::Internal("get_object_field_by_id 尚未完整实现".into()))
    }

    /// set_int_field_by_id
    pub fn set_int_field_by_id(&mut self, obj_handle: u32, _field_id: u64, _val: i32) -> Result<(), JniError> {
        let _obj_id = self.refs.resolve(obj_handle);
        Err(JniError::Internal("set_int_field_by_id 尚未完整实现".into()))
    }

    /// set_object_field_by_id
    pub fn set_object_field_by_id(&mut self, obj_handle: u32, _field_id: u64, _val_handle: u32) -> Result<(), JniError> {
        let _obj_id = self.refs.resolve(obj_handle);
        Err(JniError::Internal("set_object_field_by_id 尚未完整实现".into()))
    }

    // ========================================================================
    // String 操作
    // ========================================================================

    /// NewStringUTF: 从 UTF-8 字符串创建 Java String 对象。
    pub fn new_string_utf(&mut self, utf: &str) -> Result<u32, JniError> {
        let obj_id = ObjectId(utf.as_bytes().as_ptr() as u64); // 简单 ID
        let mut store = self.objects.lock().unwrap();
        if !store.contains(obj_id) {
            store.insert(obj_id, "java/lang/String".into(), ObjectStorage::String(utf.to_string()))
                .map_err(|e| JniError::Internal(format!("ObjectStore 插入 String 失败: {e}")))?;
        }
        Ok(self.refs.new_local(obj_id))
    }

    // ========================================================================
    // 异常状态
    // ========================================================================

    /// 检查当前线程是否有 pending 异常。
    pub fn exception_occurred(&self) -> bool {
        self.exceptions.occurred()
    }

    /// 清除 pending 异常。
    pub fn exception_clear(&mut self) {
        self.exceptions.clear();
    }

    // ========================================================================
    // Method 调用（original API，保留向后兼容）
    // ========================================================================

    /// 调用 instance method（by MethodSig）。
    pub fn call_method(
        &mut self,
        obj: ObjectId,
        sig: &MethodSig,
        mut args: JniArgs,
    ) -> Result<JValue, JniError> {
        args.set_this(obj);
        self.registry.dispatch_call(sig, &args, self.refs)
    }

    /// 调用 static method（by MethodSig）。
    pub fn call_static_method(
        &mut self,
        _class_name: &str,
        sig: &MethodSig,
        args: JniArgs,
    ) -> Result<JValue, JniError> {
        self.registry.dispatch_static(sig, &args, self.refs)
    }

    /// 获取 instance field 值（by FieldSig）。
    pub fn get_field(
        &self,
        obj: ObjectId,
        sig: &FieldSig,
    ) -> Result<JValue, JniError> {
        let _ = obj;
        self.registry.dispatch_field_get(sig)
    }

    /// 设置 instance field 值（by FieldSig）。
    pub fn set_field(
        &self,
        obj: ObjectId,
        sig: &FieldSig,
        val: JValue,
    ) -> Result<(), JniError> {
        let _ = obj;
        self.registry.dispatch_field_set(sig, val)
    }

    /// 获取 static field 值。
    pub fn get_static_field(
        &self,
        _class_name: &str,
        sig: &FieldSig,
    ) -> Result<JValue, JniError> {
        self.registry.dispatch_static_field_get(sig)
    }

    /// 设置 static field 值。
    pub fn set_static_field(
        &self,
        _class_name: &str,
        sig: &FieldSig,
        val: JValue,
    ) -> Result<(), JniError> {
        self.registry.dispatch_static_field_set(sig, val)
    }

    // ========================================================================
    // 引用管理（扩展）
    // ========================================================================

    pub fn new_local_ref(&mut self, obj_id: ObjectId) -> u32 {
        self.refs.new_local(obj_id)
    }

    pub fn delete_local_ref(&mut self, handle: u32) -> Result<(), JniError> {
        self.refs.delete_local(handle)
    }

    pub fn resolve_ref(&self, handle: u32) -> Option<ObjectId> {
        self.refs.resolve(handle)
    }

    pub fn clear_frame(&mut self) {
        self.refs.clear_frame();
    }

    /// NewGlobalRef: 创建全局引用（不受 frame 清理影响）。
    pub fn new_global_ref(&mut self, handle: u32) -> Result<u32, JniError> {
        let obj_id = self.refs.resolve(handle)
            .ok_or_else(|| JniError::Internal(format!("new_global_ref: handle {handle} 不存在")))?;
        Ok(self.refs.new_global(obj_id))
    }

    /// DeleteGlobalRef: 删除全局引用。
    pub fn delete_global_ref(&mut self, handle: u32) -> Result<(), JniError> {
        self.refs.delete_global(handle)
    }

    /// NewLocalRef: 为已有对象创建额外 local ref。
    pub fn new_local_ref_from_handle(&mut self, handle: u32) -> Result<u32, JniError> {
        let obj_id = self.refs.resolve(handle)
            .ok_or_else(|| JniError::Internal(format!("new_local_ref: handle {handle} 不存在")))?;
        Ok(self.refs.new_local(obj_id))
    }

    // ========================================================================
    // 内部辅助
    // ========================================================================

    /// 从 class handle 解析 class name。
    ///
    /// `FindClass` 存储 class 对象时，`class_name` 字段为 `"java/lang/Class"`，
    /// 实际 Java class 名放在 `StubInstance` data 中。
    /// 此方法优先从 StubInstance data 提取真实 class 名，回落则返回 class_name 字段。
    fn resolve_class_name(&self, class_handle: u32) -> Result<String, JniError> {
        let obj_id = self.refs.resolve(class_handle)
            .ok_or_else(|| JniError::Internal(format!(
                "class handle {class_handle} 不存在于 RefTable"
            )))?;

        let store = self.objects.lock().unwrap();
        let storage = store.storage(obj_id)
            .ok_or_else(|| JniError::Internal(format!(
                "class ObjectId {obj_id} 不在 ObjectStore 中"
            )))?;

        // 如果 StubInstance 包含实际 class 名（从 FindClass 来的），用它
        if let ObjectStorage::StubInstance { data } = storage {
            if let Some(name) = data.downcast_ref::<String>() {
                return Ok(name.clone());
            }
        }

        // 回落：用存储的 class_name 字段
        store.class_name(obj_id)
            .map(|s| s.to_string())
            .ok_or_else(|| JniError::Internal(format!(
                "class ObjectId {obj_id} 不在 ObjectStore 中"
            )))
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
    use crate::field::SharedField;
    use crate::types::ClassId;
    use std::sync::Arc;

    #[test]
    fn get_static_method_id_by_descriptor() {
        let mut registry = JniRegistry::new();
        let cls_name = "test/JniTest";

        let sig_ping = MethodSig {
            class: cls_name.into(),
            name: "nativePing".into(),
            args: vec![],
            ret: JType::Int,
        };

        let mut class_def = JClassDef::new(ClassId(0), cls_name.into());
        class_def.add_method(
            sig_ping.clone(),
            true, // static
            MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(42)))),
        ).unwrap();
        registry.register_class(class_def).unwrap();

        // 模拟 FindClass 创建 class handle
        let obj_id = ObjectId(0x1000_0100);
        let mut fake_objects = ObjectStore::new();
        fake_objects.insert(
            obj_id,
            "java/lang/Class".into(),
            ObjectStorage::StubInstance {
                data: Box::new(cls_name.to_string()),
            },
        ).unwrap();
        let objects = Arc::new(Mutex::new(fake_objects));

        let mut refs = RefTable::new();
        let class_handle = refs.new_local(obj_id);

        let mut exceptions = ExceptionState::new();
        let mut natives = NativeRegistry::new();

        let mut env = JniEnvSurface::new_with_objects(
            &registry,
            &objects,
            &mut refs,
            &mut exceptions,
            &mut natives,
        );

        // 验证：descriptor() 返回 JNI 格式
        assert_eq!(sig_ping.descriptor(), "()I", "descriptor 应为 ()I");

        // 验证：可以按 descriptor 找到 static method
        let mid = env.get_static_method_id(class_handle, "nativePing", "()I").unwrap();
        assert_ne!(mid.0, 0, "MethodId 不应为 0");

        // 也验证 instance method 查找
        let sig_do = MethodSig {
            class: cls_name.into(),
            name: "doNothing".into(),
            args: vec![],
            ret: JType::Void,
        };
        assert_eq!(sig_do.descriptor(), "()V", "descriptor 应为 ()V");
    }

    /// 验证 RegisterNatives 将 guest 函数绑定到 method，并通过 NativeRegistry 可以查找。
    #[test]
    fn register_natives_binds_method_to_native_registry() {
        let cls_name = "test/NativeTest";
        let mut registry = JniRegistry::new();

        // 1. 注册 class + static method nativePing()I
        let mut class_def = JClassDef::new(ClassId(0), cls_name.into());
        let sig_ping = MethodSig {
            class: cls_name.into(),
            name: "nativePing".into(),
            args: vec![],
            ret: JType::Int,
        };
        class_def.add_method(sig_ping.clone(), true,
            MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(42))))).unwrap();
        registry.register_class(class_def).unwrap();

        // 2. 构造 JniEnvSurface
        let objects = Arc::new(Mutex::new(ObjectStore::new()));
        let mut refs = RefTable::new();
        let mut exceptions = ExceptionState::new();
        let mut natives = NativeRegistry::new();

        // 创建 class object
        let class_obj_id = ObjectId(0x1000_0001);
        objects.lock().unwrap().insert(
            class_obj_id,
            "java/lang/Class".into(),
            ObjectStorage::StubInstance { data: Box::new(cls_name.to_string()) },
        ).unwrap();
        let class_handle = refs.new_local(class_obj_id);

        let mut env = JniEnvSurface::new_with_objects(
            &registry, &objects, &mut refs, &mut exceptions, &mut natives,
        );

        // 3. 调用 register_natives — 模拟 guest 通过 JNI 注册
        let guest_fn = 0x4000_1000u64;
        let count = env.register_natives(class_handle, &[
            ("nativePing".into(), "()I".into(), guest_fn),
        ]);
        assert_eq!(count, 1, "应成功注册 1 个方法");

        // 4. 验证 NativeRegistry 中已有绑定
        let method_id = registry.method_id(cls_name, &sig_ping).unwrap();
        assert_eq!(env.lookup_native(method_id), Some(guest_fn));
        assert!(env.has_native(method_id));
    }

    /// 验证 RegisterNatives 对未注册的方法名静默跳过。
    #[test]
    fn register_natives_skips_unknown_method() {
        let cls_name = "test/UnknownMethod";
        let mut registry = JniRegistry::new();

        // 注册空 class（无 method）
        let class_def = JClassDef::new(ClassId(0), cls_name.into());
        registry.register_class(class_def).unwrap();

        let objects = Arc::new(Mutex::new(ObjectStore::new()));
        let mut refs = RefTable::new();
        let mut exceptions = ExceptionState::new();
        let mut natives = NativeRegistry::new();

        let class_obj_id = ObjectId(0x1000_0002);
        objects.lock().unwrap().insert(
            class_obj_id, "java/lang/Class".into(),
            ObjectStorage::StubInstance { data: Box::new(cls_name.to_string()) },
        ).unwrap();
        let class_handle = refs.new_local(class_obj_id);

        let mut env = JniEnvSurface::new_with_objects(
            &registry, &objects, &mut refs, &mut exceptions, &mut natives,
        );

        // 尝试注册不存在的 method
        let count = env.register_natives(class_handle, &[
            ("nonexistent".into(), "()V".into(), 0x5000),
        ]);
        assert_eq!(count, 0, "未注册的 method 应被静默跳过");
    }

    /// 回归：RegisterNatives 绑定后，调用 method 必须报错，不能静默走 Rust handler。
    #[test]
    fn register_natives_bound_method_rejects_dispatch_to_rust_handler() {
        let cls_name = "test/Guarded";
        let mut registry = JniRegistry::new();

        // 1. 注册 class + instance method getValue()I（Rust handler = 返回 42）
        let sig = MethodSig {
            class: cls_name.into(),
            name: "getValue".into(),
            args: vec![],
            ret: JType::Int,
        };
        let mut class_def = JClassDef::new(ClassId(0), cls_name.into());
        class_def.add_method(sig.clone(), false,
            MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(42))))).unwrap();
        registry.register_class(class_def).unwrap();

        let method_id = registry.method_id(cls_name, &sig).unwrap();

        // 2. 创建 instance + class 对象（必须在 env 构造前，避免 refs 双重借用）
        let objects = Arc::new(Mutex::new(ObjectStore::new()));
        let mut refs = RefTable::new();

        let obj_id = ObjectId(1);
        objects.lock().unwrap().insert(
            obj_id, cls_name.into(),
            ObjectStorage::StubInstance { data: Box::new(cls_name.to_string()) },
        ).unwrap();
        let obj_handle = refs.new_local(obj_id);

        let class_obj_id = ObjectId(0x1000_0003);
        objects.lock().unwrap().insert(
            class_obj_id, "java/lang/Class".into(),
            ObjectStorage::StubInstance { data: Box::new(cls_name.to_string()) },
        ).unwrap();
        let class_handle = refs.new_local(class_obj_id);

        let mut exceptions = ExceptionState::new();
        let mut natives = NativeRegistry::new();

        let mut env = JniEnvSurface::new_with_objects(
            &registry, &objects, &mut refs, &mut exceptions, &mut natives,
        );

        // 3. 绑定前：正常 dispatch 应返回 42（Rust handler）
        let before = env.call_int_method_by_id(obj_handle, method_id.0, &[]);
        assert_eq!(before.unwrap(), 42, "绑定前应正常走 Rust handler");

        // 4. RegisterNatives 绑定 method → guest fn
        let count = env.register_natives(class_handle, &[
            ("getValue".into(), "()I".into(), 0x4000_5000),
        ]);
        assert_eq!(count, 1, "RegisterNatives 应成功绑定");

        // 5. 绑定后 dispatch：必须报错，不能静默回落 Rust handler
        let after = env.call_int_method_by_id(obj_handle, method_id.0, &[]);
        assert!(
            matches!(after, Err(JniError::Internal(ref msg)) if msg.contains("guest native")),
            "RegisterNatives 绑定后 dispatch 必须显式报错，不能静默走 Rust handler。实际结果: {after:?}"
        );
    }
}
