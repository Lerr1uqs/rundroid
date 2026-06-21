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
//! # 线程安全性
//!
//! 捕获的 `Py<PyAny>` callable 仅在 `Python::with_gil` 内访问，
//! GIL 提供了同步保证。`PythonCallable` 通过 `unsafe impl Sync`
//! 包装满足 `MethodImpl::RustNative` 的 `Send + Sync` 约束。

use pyo3::prelude::*;
use pyo3::types::PyTuple;
use pyo3::IntoPyObjectExt;
use rundroid_jni::{JniArgs, JniError, JType, JValue};
use std::sync::Arc;

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
/// - 接收 `&JniArgs`（类型化 JNI 参数）
/// - 将 JniArgs 转换为 Python tuple
/// - 通过 `Python::with_gil` 调用 Python 函数
/// - 将 Python 返回值转换回 `JValue`
/// - 校验返回值类型匹配声明类型
///
/// # 参数
/// - `py_fn`: Python callable（可以是 instance method 或普通函数）
/// - `sig_ret`: 声明的返回值类型（用于运行时校验）
pub fn wrap_python_method(
    py_fn: Py<PyAny>,
    sig_ret: JType,
) -> Arc<dyn Fn(&JniArgs) -> Result<JValue, JniError> + Send + Sync> {
    let callable = PythonCallable { inner: py_fn };
    Arc::new(move |args: &JniArgs| -> Result<JValue, JniError> {
        Python::with_gil(|py| {
            let fn_ref = callable.inner.bind(py);

            // JniArgs → Python tuple（只取 values 做简单映射）
            let py_args = jni_args_to_py_tuple(py, args)?;

            // 调用 Python 函数
            let result = fn_ref
                .call1((py_args,))
                .map_err(|e| JniError::Internal(format!("Python method 调用失败: {e}")))?;

            // Python 返回值 → JValue
            let jval = py_object_to_jvalue(&result)
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
) -> Arc<dyn Fn(&JniArgs) -> Result<JValue, JniError> + Send + Sync> {
    let callable = PythonCallable { inner: py_fn };
    Arc::new(move |_args: &JniArgs| -> Result<JValue, JniError> {
        Python::with_gil(|py| {
            let fn_ref = callable.inner.bind(py);
            let result = fn_ref
                .call0()
                .map_err(|e| JniError::Internal(format!("Python method 调用失败: {e}")))?;
            let jval = py_object_to_jvalue(&result)
                .map_err(|e| JniError::Internal(format!("返回值转换失败: {e}")))?;
            validate_return_value(&jval, &sig_ret)?;
            Ok(jval)
        })
    })
}

/// 将 Python 对象转换为 Rust [`JValue`]。
///
/// 类型映射：
/// - None → JValue::Null
/// - bool → JValue::Boolean
/// - int（i32 范围）→ JValue::Int
/// - int（超出 i32）→ JValue::Long
/// - float → JValue::Double
/// - bytes → JValue::Null（暂不支持 Object 引用，未来用 ObjectId）
/// - str → JValue::Null（同上）
pub fn py_object_to_jvalue(obj: &Bound<'_, PyAny>) -> Result<JValue, String> {
    if obj.is_none() {
        return Ok(JValue::Null);
    }
    if let Ok(b) = obj.extract::<bool>() {
        return Ok(JValue::Boolean(b));
    }
    // 先尝试 i32，再尝试 i64
    if let Ok(i) = obj.extract::<i32>() {
        return Ok(JValue::Int(i));
    }
    if let Ok(l) = obj.extract::<i64>() {
        return Ok(JValue::Long(l));
    }
    if let Ok(d) = obj.extract::<f64>() {
        return Ok(JValue::Double(d));
    }
    // bytes / str 暂不支持 ObjectId 引用，返回 Null 占位
    if obj.extract::<Vec<u8>>().is_ok() || obj.extract::<String>().is_ok() {
        return Ok(JValue::Null);
    }
    let type_name = obj.get_type()
        .name()
        .map(|s| s.to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());
    Err(format!(
        "不支持的 Python 返回类型: {type_name}"
    ))
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
        (JValue::Long(_), JType::Long) => Ok(()),
        (JValue::Float(_), JType::Float) => Ok(()),
        (JValue::Double(_), JType::Double) => Ok(()),
        (JValue::Object(_), JType::Object(_)) => Ok(()),
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

/// 将 [`JniArgs`] 转换为 Python tuple。
fn jni_args_to_py_tuple<'py>(
    py: Python<'py>,
    args: &JniArgs,
) -> Result<Bound<'py, PyTuple>, JniError> {
    let values = args.values();
    let items: Vec<PyObject> = values.iter().map(|v| jvalue_to_py_object(py, v)).collect();
    PyTuple::new(py, items)
        .map_err(|e| JniError::Internal(format!("PyTuple 构造失败: {e}")))
}

/// 将单个 [`JValue`] 转换为 Python 对象。
fn jvalue_to_py_object(py: Python<'_>, val: &JValue) -> PyObject {
    match val {
        JValue::Void => py.None(),
        JValue::Boolean(b) => b.into_py_any(py).unwrap(),
        JValue::Byte(b) => b.into_py_any(py).unwrap(),
        JValue::Char(c) => c.into_py_any(py).unwrap(),
        JValue::Short(s) => s.into_py_any(py).unwrap(),
        JValue::Int(i) => i.into_py_any(py).unwrap(),
        JValue::Long(l) => l.into_py_any(py).unwrap(),
        JValue::Float(f) => f.into_py_any(py).unwrap(),
        JValue::Double(d) => d.into_py_any(py).unwrap(),
        JValue::Object(_) | JValue::Null => py.None(),
    }
}
