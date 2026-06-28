//! JavaShim bridge — Python callable 到 Rust-native method handler 的桥接。
//!
//! 本模块负责：
//! 1. 将 Python method 函数包装为 Rust-side `MethodImpl::RustNative` 闭包
//! 2. Python 返回值 → JValue 转换
//! 3. 运行时返回值类型校验
//!
//! # 架构
//!
//! 不使用 `MethodImpl::PythonShim(id)` 全局回调表方案——
//! 而是直接将 Python callable 捕获进 `Arc<dyn Fn(...)>` 闭包。
//! 这样 `dispatch.rs` 无需感知 Python 存在，
//! 所有 method dispatch 走同一条 `RustNative` 路径。
//!
//! # 实例绑定
//!
//! 对于 instance method，闭包通过 `JniArgs::this()` 获取 `ObjectId`，
//! 再通过捕获的 `Arc<Mutex<ObjectStore>>` 查找 Python 实例，
//! 将实例绑定为 `self` 后调用 Python 方法。
//! 对于 static method，`this()` 为 `None`，直接调用未绑定函数。
//!
//! # 线程安全性
//!
//! 捕获的 `Py<PyAny>` callable 仅在 `Python::with_gil` 内访问，
//! GIL 提供了同步保证。`PythonCallable` 通过 `unsafe impl Sync`
//! 包装满足 `MethodImpl::RustNative` 的 `Send + Sync` 约束。

use pyo3::prelude::*;
use pyo3::types::PyTuple;
use pyo3::IntoPyObjectExt;
use rundroid_jni::{
    IdAllocator, JniArgs, JniError, JType, JValue, ObjectId, ObjectStorage, ObjectStore,
};
use std::sync::{Arc, Mutex};

// ============================================================================
// PythonCallable — 线程安全的 Python 函数引用
// ============================================================================

/// 线程安全的 Python callable wrapper。
///
/// 满足 `Send + Sync` 的前提是仅在 `Python::with_gil` 内部解引用。
struct PythonCallable {
    inner: Py<PyAny>,
}

// SAFETY: Python::with_gil 提供互斥访问，等价于 Mutex 语义
unsafe impl Sync for PythonCallable {}
unsafe impl Send for PythonCallable {}

// ============================================================================
// 公共 API
// ============================================================================

/// 将 Python method 函数包装为 Rust-native handler。
///
/// 返回的闭包：
/// - 接收 `&JniArgs`（类型化 JNI 参数，含 `this()` 实例 ID）
/// - 若是 instance method（`this()` 为 `Some`）：
///   从 `ObjectStore` 查找 Python 实例，绑定 `self` 后调用
/// - 若是 static method（`this()` 为 `None`）：直接调用未绑定函数
/// - 将 Python 返回值转换回 `JValue`
/// - 校验返回值类型匹配声明类型
///
/// # 参数
/// - `py_fn`: Python callable（可以是 instance method 或普通函数）
/// - `java_name`: 该方法的 Java 名（descriptor 中 `(` 之前的部分）。
///   instance 分派时按此名调用 ``instance.<java_name>``，经 ``JavaObject.__getattr__``
///   复用蓝图 ``__java_dispatch__``（与方向 B 共用，零分歧）。
/// - `sig_ret`: 声明的返回值类型（用于运行时校验）
/// - `objects`: 共享 ObjectStore，用于通过 ObjectId 查找 Python 实例、
///   以及把 `str`/`bytes` 返回值落身份成 Java 对象。
/// - `id_alloc`: 共享 ObjectId 分配器，`str`/`bytes` 自动 coercion 时分配新 ObjectId。
pub fn wrap_python_method(
    py_fn: Py<PyAny>,
    java_name: String,
    sig_ret: JType,
    objects: Arc<Mutex<ObjectStore>>,
    id_alloc: Arc<Mutex<IdAllocator>>,
) -> Arc<dyn Fn(&JniArgs) -> Result<JValue, JniError> + Send + Sync> {
    let callable = PythonCallable { inner: py_fn };
    Arc::new(move |args: &JniArgs| -> Result<JValue, JniError> {
        Python::with_gil(|py| {
            let fn_ref = callable.inner.bind(py);

            // JniArgs → Python tuple（JNI 参数值列表，不含 this）。
            // 方向 A 的入参编组：JValue::Object(oid) 按 storage 还原成 str/bytes/对象，
            // 不再统一退化成 None。
            let py_args = jni_args_to_py_tuple(py, args, &objects)?;

            // instance method（this 非空）：按 Java 名调用 instance.<java_name>(*py_args)，
            // 经 JavaObject.__getattr__ 复用 __java_dispatch__；
            // static method（this 为空）：直接调用未绑定函数 py_fn(*py_args)。
            //
            // 关键：必须在调用 Python 方法**之前**释放 objects 锁——方法体内可能
            // `self._avm.new_object(...)` → `register_java_object` 再次锁 objects，
            // 持锁调用会自锁死锁。故锁内只 clone 出 instance 引用（refcount，廉价），
            // 锁随 block 结束释放，再进 Python 调用。
            let result = if let Some(this_oid) = args.this() {
                // 锁内：仅 clone 出 instance 引用（若有），立即释放锁
                let instance_opt: Option<Py<PyAny>> = {
                    let store = objects.lock().unwrap();
                    match store.storage(this_oid) {
                        Some(ObjectStorage::HostValue { data }) => {
                            data.downcast_ref::<Py<PyAny>>().map(|p| p.clone_ref(py))
                        }
                        _ => None,
                    }
                }; // ← objects 锁在此释放

                match instance_opt {
                    Some(instance) => {
                        // 经 __getattr__(java_name) → __java_dispatch__，py_args 元组展开为位置参数
                        instance.bind(py).call_method1(&java_name, &py_args)
                    }
                    // 实例不在 ObjectStore / 非 HostValue：guest 经 NewObject 创建的对象在
                    // Rust 侧是 StubInstance（单线程仿真下无 Python JavaObject backing）。
                    // 蓝图函数签名含 self，直接 call(py_args) 会因缺 self 而 TypeError——
                    // 故注入 self=None：(None, *py_args)。纯计算 override（不依赖 self）即可
                    // 正常调用；若 override 访问 self 属性，None 上 AttributeError fail-fast
                    //（正确语义：guest 对象无 Python 实例状态可读）。
                    None => {
                        let mut full: Vec<PyObject> = Vec::with_capacity(py_args.len() + 1);
                        full.push(py.None());
                        full.extend(py_args.iter().map(|item| item.unbind()));
                        let full_tuple = PyTuple::new(py, full)
                            .map_err(|e| JniError::Internal(format!("PyTuple 构造失败: {e}")))?;
                        fn_ref.call(&full_tuple, None)
                    }
                }
            } else {
                // static method：直接调用，py_args 元组展开为位置参数
                fn_ref.call(&py_args, None)
            };

            let result = result
                .map_err(|e| JniError::Internal(format!("Python method 调用失败: {e}")))?;

            // Python 返回值 → JValue（str/bytes 自动落身份成 Java 对象，不再吞成 Null）
            let jval = py_to_jvalue(py, &result, &objects, &id_alloc)
                .map_err(|e| JniError::Internal(format!("返回值转换失败: {e}")))?;

            // 运行时校验返回值类型
            validate_return_value(&jval, &sig_ret)?;

            Ok(jval)
        })
    })
}

/// 将 Python 函数包装为不需要参数的 handler（用于 placeholder / simple stub）。
pub fn wrap_python_method_no_args(
    py_fn: Py<PyAny>,
    sig_ret: JType,
    objects: Arc<Mutex<ObjectStore>>,
    id_alloc: Arc<Mutex<IdAllocator>>,
) -> Arc<dyn Fn(&JniArgs) -> Result<JValue, JniError> + Send + Sync> {
    let callable = PythonCallable { inner: py_fn };
    Arc::new(move |_args: &JniArgs| -> Result<JValue, JniError> {
        Python::with_gil(|py| {
            let fn_ref = callable.inner.bind(py);
            let result = fn_ref
                .call0()
                .map_err(|e| JniError::Internal(format!("Python method 调用失败: {e}")))?;
            let jval = py_to_jvalue(py, &result, &objects, &id_alloc)
                .map_err(|e| JniError::Internal(format!("返回值转换失败: {e}")))?;
            validate_return_value(&jval, &sig_ret)?;
            Ok(jval)
        })
    })
}

// ============================================================================
// Python ↔ JNI 值编组（成对规则，进得来回得去）
// ============================================================================
//
// 编组规则集中在此处，Python→JNI 与 JNI→Python 两个方向共用同一套类型语义，
// 避免「进得去、回不来」的单向补丁（见 change python-jni-value-marshalling 决策 5）。
//
// Python → JValue（py_to_jvalue）：
//   None            → Null
//   bool            → Boolean       （必须在 int 之前判断，Python bool ⊂ int）
//   int (i32)       → Int
//   int (>i32)      → Long
//   float           → Double
//   str             → Object(String)  自动 coercion：落 ObjectStore 为 java/lang/String
//   bytes           → Object([B)      自动 coercion：落 ObjectStore 为 byte[]
//   显式 wrapper    → Object(oid)     复用其已有 ObjectId（identity 敏感场景）
//   其它            → Err            fail-fast，绝不静默吞成 Null
//
// JValue → Python（jvalue_to_py）：
//   Void / Null     → None
//   primitive       → 对应 Python 标量
//   Object(String)  → str
//   Object(byte[])  → bytes
//   Object(Wrapper) → 对应 Python 标量
//   Object(HostValue)→ 原 Python 对象（JavaObject 回传）
//   其它 Object     → Err            fail-fast，未覆盖的 storage 直接报错（Req 6）

/// 把 Python `str` 落成 `ObjectStore` 中的 `java/lang/String`，返回其 `ObjectId`。
///
/// 分配 oid 与插入分两步加锁（不嵌套），避免与其它持锁路径互相自锁。
fn intern_string(
    objects: &Arc<Mutex<ObjectStore>>,
    id_alloc: &Arc<Mutex<IdAllocator>>,
    value: String,
) -> Result<ObjectId, String> {
    let oid = id_alloc.lock().unwrap().object();
    objects
        .lock()
        .unwrap()
        .insert(oid, "java/lang/String".to_string(), ObjectStorage::String(value))
        .map_err(|e| format!("ObjectStore 插入 String 失败: {e}"))?;
    Ok(oid)
}

/// 把 Python `bytes` 落成 `ObjectStore` 中的 `byte[]`（`PrimitiveArray(Byte)`），返回其 `ObjectId`。
fn intern_bytes(
    objects: &Arc<Mutex<ObjectStore>>,
    id_alloc: &Arc<Mutex<IdAllocator>>,
    value: Vec<u8>,
) -> Result<ObjectId, String> {
    let oid = id_alloc.lock().unwrap().object();
    let elements = value.into_iter().map(|b| JValue::Byte(b as i8)).collect();
    objects
        .lock()
        .unwrap()
        .insert(
            oid,
            "[B".to_string(),
            ObjectStorage::PrimitiveArray { jtype: JType::Byte, elements },
        )
        .map_err(|e| format!("ObjectStore 插入 byte[] 失败: {e}"))?;
    Ok(oid)
}

/// 将 Python 对象转换为 Rust [`JValue`]（自动 coercion + fail-fast）。
///
/// 见模块顶部编组规则表。`str`/`bytes` 经 `ObjectStore` 落身份成 Java 对象，
/// 不再静默吞成 `Null`；未支持的类型直接返回 `Err`。
pub fn py_to_jvalue(
    _py: Python<'_>,
    obj: &Bound<'_, PyAny>,
    objects: &Arc<Mutex<ObjectStore>>,
    id_alloc: &Arc<Mutex<IdAllocator>>,
) -> Result<JValue, String> {
    // None → Null
    if obj.is_none() {
        return Ok(JValue::Null);
    }
    // 显式 wrapper（JavaString / JavaByteArray / 任何携带 _rundroid_oid 的对象）：
    // 复用其已有 ObjectId，保留对象身份（identity 敏感场景）。
    if let Ok(oid_attr) = obj.getattr("_rundroid_oid") {
        if let Ok(oid) = oid_attr.extract::<u64>() {
            return Ok(JValue::Object(ObjectId(oid)));
        }
    }
    // 注意：bool 必须在 int 之前判断（Python bool 是 int 的子类，extract::<i32>() 也会成功）
    // TODO: 这地方难道没办法改成match范式？
    if let Ok(b) = obj.extract::<bool>() {
        return Ok(JValue::Boolean(b));
    }
    if let Ok(i) = obj.extract::<i32>() {
        return Ok(JValue::Int(i));
    }
    if let Ok(l) = obj.extract::<i64>() {
        return Ok(JValue::Long(l));
    }
    if let Ok(d) = obj.extract::<f64>() {
        return Ok(JValue::Double(d));
    }
    // str → java/lang/String（自动 coercion）
    if let Ok(s) = obj.extract::<String>() {
        let oid = intern_string(objects, id_alloc, s)?;
        return Ok(JValue::Object(oid));
    }
    // bytes → byte[]（自动 coercion）
    if let Ok(b) = obj.extract::<Vec<u8>>() {
        let oid = intern_bytes(objects, id_alloc, b)?;
        return Ok(JValue::Object(oid));
    }
    // 未支持的值类型 → fail-fast（绝不静默降级为 Null）
    let type_name = obj
        .get_type()
        .name()
        .map(|s| s.to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());
    Err(format!("不支持的 Python 值类型（无法编组为 JValue）: {type_name}"))
}

/// 校验运行时 [`JValue`] 与声明的 [`JType`] 是否匹配。
///
/// # 规则
/// - `Void` ← 匹配 `JValue::Void` 和 `JValue::Null`（Python None → void）
/// - `Null` ← 允许在 Object/Array 类型位置
/// - `Null` 出现在 primitive 位置 → `NullNotAllowed`
/// - 类型不匹配 → `TypeMismatch`
pub fn validate_return_value(val: &JValue, expected: &JType) -> Result<(), JniError> {
    match (val, expected) {
        // Void：Python 无返回值时返回 None，所以 Null 也接受
        (JValue::Void, JType::Void) | (JValue::Null, JType::Void) => Ok(()),
        // Null 允许在 Object / Array 位置
        (JValue::Null, JType::Object(_)) | (JValue::Null, JType::Array(_)) => Ok(()),
        // Null 不允许在 primitive 位置
        (JValue::Null, _) => Err(JniError::NullNotAllowed(format!(
            "期望 {expected:?}，得到 null"
        ))),
        // 逐类型匹配
        (JValue::Boolean(_), JType::Boolean) => Ok(()),
        (JValue::Byte(_), JType::Byte) => Ok(()),
        (JValue::Char(_), JType::Char) => Ok(()),
        (JValue::Short(_), JType::Short) => Ok(()),
        (JValue::Int(_), JType::Int) => Ok(()),
        // 无损 widening：Python int 是统一整型,runtime 按值大小落 Int/Long。
        // 用户无法控制一个"小整数"被编组成 Int —— 它出现在声明 Long 的返回/参数位置时
        // 应放行（int→long 无损），否则声明 (J) 的方法几乎只能收刚好 >i32 的值，不可用。
        // 反方向 Long→Int 有损，仍拒绝。
        (JValue::Int(_), JType::Long) => Ok(()),
        (JValue::Long(_), JType::Long) => Ok(()),
        (JValue::Float(_), JType::Float) => Ok(()),
        (JValue::Double(_), JType::Double) => Ok(()),
        // Object 引用既可作 Object 位置，也可作 Array 位置（数组在 JNI 里也是对象，
        // 例如 Python bytes 经 coercion 成的 byte[] 返回值就是 JValue::Object(oid)，
        // 声明类型为 JType::Array(Byte)）。
        (JValue::Object(_), JType::Object(_)) | (JValue::Object(_), JType::Array(_)) => Ok(()),
        // Void 不在允许位置
        (JValue::Void, _) => Err(JniError::TypeMismatch {
            expected: expected.clone(),
            actual: JType::Void,
        }),
        // 其他不匹配
        _ => Err(JniError::TypeMismatch {
            expected: expected.clone(),
            actual: val.jtype(),
        }),
    }
}

// ============================================================================
// 内部辅助函数
// ============================================================================

/// 将 [`JniArgs`] 转换为 Python tuple（方向 A 入参编组）。
///
/// 每个 `JValue` 经 [`jvalue_to_py`] 按 storage 类型还原——`String`→`str`、
/// `byte[]`→`bytes`、primitive→标量、`Null`→`None`，不再统一退化成 `None`。
fn jni_args_to_py_tuple<'py>(
    py: Python<'py>,
    args: &JniArgs,
    objects: &Arc<Mutex<ObjectStore>>,
) -> Result<Bound<'py, PyTuple>, JniError> {
    let items: Vec<PyObject> = args
        .values()
        .iter()
        .map(|v| jvalue_to_py(py, v, objects))
        .collect::<PyResult<Vec<_>>>()
        .map_err(|e| JniError::Internal(format!("JValue→Python 编组失败: {e}")))?;
    PyTuple::new(py, items)
        .map_err(|e| JniError::Internal(format!("PyTuple 构造失败: {e}")))
}

/// 将 Python tuple 参数转换为 [`JniArgs`]（用于 Rust registry dispatch）。
///
/// 逐项经 [`py_to_jvalue`] 编组：`str`/`bytes` 自动落身份成 Java 对象，
/// primitive/None 直转，未支持类型 fail-fast。
pub fn pyargs_to_jniargs(
    py: Python<'_>,
    args: &Bound<'_, PyTuple>,
    objects: &Arc<Mutex<ObjectStore>>,
    id_alloc: &Arc<Mutex<IdAllocator>>,
) -> Result<JniArgs, String> {
    let mut values: Vec<JValue> = Vec::with_capacity(args.len());
    for item in args.iter() {
        let jval = py_to_jvalue(py, &item, objects, id_alloc)?;
        values.push(jval);
    }
    Ok(JniArgs::from_vec(values))
}

/// 锁内从 `ObjectStore` 取出的对象数据（owned，可在锁外构建 Python 对象）。
///
/// 构建 Python 对象（`str`/`bytes`/`clone_ref`）不持有 objects 锁，
/// 避免与重入路径自锁；故先把数据 clone 出来再释放锁。
enum OwnedObject {
    Str(String),
    Bytes(Vec<u8>),
    /// Wrapper（Integer/Boolean 等）的 primitive 值。
    Primitive(JValue),
    /// HostValue 持有的 Python 对象（JavaObject 回传）。
    PyObj(Py<PyAny>),
    /// 未覆盖的 storage / dangling OID —— 携带描述用于 fail-fast 报错。
    Unsupported(String),
}

/// 将单个 [`JValue`] 转换为 Python 对象（storage-aware）。
///
/// 见模块顶部编组规则表。`JValue::Object(oid)` 按 `ObjectStorage` 类型分发，
/// 不再统一退化成 `None`。
pub fn jvalue_to_py(
    py: Python<'_>,
    val: &JValue,
    objects: &Arc<Mutex<ObjectStore>>,
) -> PyResult<PyObject> {
    match val {
        JValue::Void | JValue::Null => Ok(py.None()),
        JValue::Boolean(b) => b.into_py_any(py),
        JValue::Byte(b) => (*b as i64).into_py_any(py),
        JValue::Char(c) => (*c).into_py_any(py),
        JValue::Short(s) => (*s as i64).into_py_any(py),
        JValue::Int(i) => i.into_py_any(py),
        JValue::Long(l) => l.into_py_any(py),
        JValue::Float(f) => f.into_py_any(py),
        JValue::Double(d) => d.into_py_any(py),
        JValue::Object(oid) => jvalue_object_to_py(py, *oid, objects),
    }
}

/// 将 `JValue::Object(oid)` 按 storage 类型还原成 Python 值。
///
/// 对已覆盖的 storage（String / byte[] / Wrapper / HostValue<Py>）按类型分发还原；
/// 对未覆盖的 storage 和 dangling OID，fail-fast 抛异常（Req 6）。
fn jvalue_object_to_py(
    py: Python<'_>,
    oid: ObjectId,
    objects: &Arc<Mutex<ObjectStore>>,
) -> PyResult<PyObject> {
    // 锁内：按 storage clone 出 owned 数据 / 错误描述，立即释放锁再 act。
    let owned = {
        let store = objects.lock().unwrap();
        match store.storage(oid) {
            None => OwnedObject::Unsupported(format!(
                "ObjectId {oid} 不在 ObjectStore 中（dangling OID）"
            )),
            Some(ObjectStorage::String(s)) => OwnedObject::Str(s.clone()),
            Some(ObjectStorage::PrimitiveArray { jtype: JType::Byte, elements }) => {
                // byte[] → bytes（仅 Byte 数组；其它 primitive 数组本 change 不覆盖）
                let bytes: Vec<u8> = elements
                    .iter()
                    .filter_map(|v| match v {
                        JValue::Byte(b) => Some(*b as u8),
                        _ => None,
                    })
                    .collect();
                OwnedObject::Bytes(bytes)
            }
            Some(ObjectStorage::PrimitiveArray { jtype, .. }) => OwnedObject::Unsupported(
                format!("ObjectStorage::PrimitiveArray({jtype:?}) 不支持（仅 byte[] 已实现）")
            ),
            Some(ObjectStorage::Wrapper { value, .. }) => OwnedObject::Primitive(value.clone()),
            Some(ObjectStorage::HostValue { data }) => match data.downcast_ref::<Py<PyAny>>() {
                Some(p) => OwnedObject::PyObj(p.clone_ref(py)),
                None => OwnedObject::Unsupported(
                    "ObjectStorage::HostValue 的 data 非 Py<PyAny>，无法还原为 Python 对象".into()
                ),
            },
            Some(ObjectStorage::ObjectArray { .. }) => OwnedObject::Unsupported(
                "ObjectStorage::ObjectArray 未实现 Python 投影".into()
            ),
            Some(ObjectStorage::StubInstance { .. }) => OwnedObject::Unsupported(
                "ObjectStorage::StubInstance 未实现 Python 投影".into()
            ),
        }
    }; // ← objects 锁在此释放

    match owned {
        OwnedObject::Str(s) => s.into_py_any(py),
        OwnedObject::Bytes(b) => b.into_py_any(py),
        OwnedObject::Primitive(v) => jvalue_to_py(py, &v, objects),
        OwnedObject::PyObj(p) => Ok(p.into()),
        OwnedObject::Unsupported(desc) => {
            // dangling OID 是运行时状态不一致 → RuntimeError；其余是类型未覆盖 → TypeError
            if desc.contains("dangling OID") {
                Err(pyo3::exceptions::PyRuntimeError::new_err(desc))
            } else {
                Err(pyo3::exceptions::PyTypeError::new_err(desc))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 辅助：构造一个注入了指定 storage 的 ObjectStore，返回 (store, oid)。
    fn store_with(oid_val: u64, storage: ObjectStorage) -> (Arc<Mutex<ObjectStore>>, ObjectId) {
        let mut alloc = IdAllocator::new();
        let oid = alloc.object();
        // 确保分配的 oid 与期望一致（构造用）
        assert_eq!(oid.0, oid_val, "IdAllocator 应从 1 递增");
        let mut store = ObjectStore::new();
        store
            .insert(oid, "test/Fake".into(), storage)
            .unwrap();
        (Arc::new(Mutex::new(store)), oid)
    }

    #[test]
    fn test_jvalue_object_dangling_oid_raises_runtime_error() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let store = Arc::new(Mutex::new(ObjectStore::new()));
            let result = jvalue_object_to_py(py, ObjectId(999), &store);
            assert!(result.is_err(), "dangling OID 应抛异常而非返回 None");
            let err = result.unwrap_err();
            assert!(err.is_instance_of::<pyo3::exceptions::PyRuntimeError>(py));
        });
    }

    #[test]
    fn test_jvalue_object_object_array_raises_type_error() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let storage = ObjectStorage::ObjectArray {
                class_name: "java/lang/String".into(),
                elements: vec![],
            };
            let (store, oid) = store_with(1, storage);
            let result = jvalue_object_to_py(py, oid, &store);
            assert!(result.is_err(), "ObjectArray 应抛 TypeError");
            let err = result.unwrap_err();
            assert!(err.is_instance_of::<pyo3::exceptions::PyTypeError>(py));
        });
    }

    #[test]
    fn test_jvalue_object_stub_instance_raises_type_error() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let storage = ObjectStorage::StubInstance {
                data: Box::new("stub data"),
            };
            let (store, oid) = store_with(1, storage);
            let result = jvalue_object_to_py(py, oid, &store);
            assert!(result.is_err(), "StubInstance 应抛 TypeError");
            let err = result.unwrap_err();
            assert!(err.is_instance_of::<pyo3::exceptions::PyTypeError>(py));
        });
    }

    #[test]
    fn test_jvalue_object_non_byte_primitive_array_raises_type_error() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let storage = ObjectStorage::PrimitiveArray {
                jtype: JType::Int,
                elements: vec![JValue::Int(1), JValue::Int(2)],
            };
            let (store, oid) = store_with(1, storage);
            let result = jvalue_object_to_py(py, oid, &store);
            assert!(result.is_err(), "非 Byte 的 PrimitiveArray 应抛 TypeError");
            let err = result.unwrap_err();
            assert!(err.is_instance_of::<pyo3::exceptions::PyTypeError>(py));
        });
    }

    #[test]
    fn test_jvalue_object_host_value_non_py_raises_type_error() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            // HostValue 内部 data 非 Py<PyAny>（放入一个 String）
            let storage = ObjectStorage::HostValue {
                data: Box::new("not a py object".to_string()),
            };
            let (store, oid) = store_with(1, storage);
            let result = jvalue_object_to_py(py, oid, &store);
            assert!(result.is_err(), "非 Py HostValue 应抛 TypeError");
            let err = result.unwrap_err();
            assert!(err.is_instance_of::<pyo3::exceptions::PyTypeError>(py));
        });
    }
}
