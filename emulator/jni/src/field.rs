//! JNI field 访问器类型。
//!
//! `FieldAccess` 枚举区分 Rust-native 和 Python-shim 两种 field 实现来源，
//! 与 method 的 `MethodImpl` 模式一致。

use crate::error::JniError;
use crate::types::JValue;
use std::sync::{Arc, Mutex};

/// Rust-native field handler trait。
///
/// get / set 都通过 `&self` 访问（内部可变性模式），
/// 这样 method handler 可以通过 `Arc` 共享同一个 handler，
/// 实现 method 与 field 的联动（method 读/写 field 值）。
pub trait RustFieldHandler: Send + Sync + std::fmt::Debug {
    /// 读取 field 值。
    fn get(&self) -> JValue;

    /// 写入 field 值。
    ///
    /// 类型不匹配或其它原因导致的失败通过 `JniError` 返回。
    fn set(&self, val: JValue) -> Result<(), JniError>;
}

/// Field 访问器来源。
///
/// 与 `MethodImpl` 对称：同一个 registry 同时容纳两种实现来源。
pub enum FieldAccess {
    /// Rust 侧实现的 field handler。
    RustNative(Arc<dyn RustFieldHandler>),
    /// Python shim 实现的 field（仅存 ID，由 bridge 回调）。
    PythonShim(u64),
}

impl Clone for FieldAccess {
    fn clone(&self) -> Self {
        match self {
            FieldAccess::RustNative(handler) => FieldAccess::RustNative(Arc::clone(handler)),
            FieldAccess::PythonShim(id) => FieldAccess::PythonShim(*id),
        }
    }
}

impl std::fmt::Debug for FieldAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FieldAccess::RustNative(h) => write!(f, "RustNative({:?})", h),
            FieldAccess::PythonShim(id) => write!(f, "PythonShim({id})"),
        }
    }
}

// ============================================================================
// SharedField — 共享 field handler
// ============================================================================

/// 持有单个 JValue 的 field handler（内部用 `Mutex` 实现共享访问）。
///
/// 通过 `Arc<SharedField>`，同一个 field handler 可以同时注册到
/// registry 的 field 表和 method handler 闭包中，实现 method 内读写 field 值。
///
/// # 示例
///
/// ```ignore
/// let counter = Arc::new(SharedField::new(JValue::Int(0)));
/// // field 注册
/// class_def.add_field(sig, true, FieldAccess::RustNative(counter.clone()));
/// // method handler 通过 clone 的 Arc 读写同一个 field
/// class_def.add_method(sig, true, MethodImpl::RustNative(Arc::new(move |_| {
///     let val = counter.get();
///     ...
/// })));
/// ```
#[derive(Debug)]
pub struct SharedField {
    value: Mutex<JValue>,
}

impl SharedField {
    /// 用初始值创建。
    pub fn new(value: JValue) -> Self {
        Self { value: Mutex::new(value) }
    }
}

impl RustFieldHandler for SharedField {
    fn get(&self) -> JValue {
        self.value.lock().unwrap().clone()
    }

    fn set(&self, val: JValue) -> Result<(), JniError> {
        *self.value.lock().unwrap() = val;
        Ok(())
    }
}
