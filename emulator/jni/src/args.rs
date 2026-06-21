//! JNI 方法调用参数容器。
//!
//! 提供类型化参数获取方法，不做 silent widening / narrowing。
//! 类型不匹配时直接返回错误，符合 fail-fast 原则。

use crate::error::JniError;
use crate::types::{JType, JValue, ObjectId};

/// JNI 方法调用参数列表。
///
/// 由 dispatch 层构造，提供按索引取出不同类型值的接口。
///
/// # 实例感知
///
/// `this` 字段携带调用目标实例的 [`ObjectId`]（instance method 调用时）。
/// static method 调用时 `this` 为 `None`。
/// handler 可通过 [`Self::this()`] 获取实例 ID，
/// 配合 `ObjectStore` 查找实例数据进行实例级状态访问。
#[derive(Debug, Clone)]
pub struct JniArgs {
    /// 调用目标实例（instance method 时为 Some，static method 时为 None）。
    this: Option<ObjectId>,
    /// 方法参数的 JNI 值列表。
    values: Vec<JValue>,
}

impl JniArgs {
    /// 构造空的参数列表（static method 调用）。
    pub fn new() -> Self {
        Self {
            this: None,
            values: Vec::new(),
        }
    }

    /// 从已有值构造（static method 调用）。
    pub fn from_vec(values: Vec<JValue>) -> Self {
        Self {
            this: None,
            values,
        }
    }

    /// 设置调用目标实例。用于 instance method dispatch 前标记 this 对象。
    pub fn set_this(&mut self, obj: ObjectId) {
        self.this = Some(obj);
    }

    /// 获取调用目标实例的 [`ObjectId`]。
    /// instance method 调用时返回 `Some`，static method 调用时返回 `None`。
    pub fn this(&self) -> Option<ObjectId> {
        self.this
    }

    /// 参数个数。
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// 是否为空参数列表。
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// 获取全部参数值。
    pub fn values(&self) -> &[JValue] {
        &self.values
    }

    // —— 类型化 getter ——

    /// 按索引取出 Boolean 值。非 Boolean 类型或 Null 则失败。
    pub fn boolean_at(&self, i: usize) -> Result<bool, JniError> {
        match self.values.get(i) {
            Some(JValue::Boolean(v)) => Ok(*v),
            Some(v) => Err(JniError::TypeMismatch { expected: JType::Boolean, actual: v.jtype() }),
            None => Err(JniError::ArgCountMismatch { expected: i + 1, actual: self.values.len() }),
        }
    }

    /// 按索引取出 Byte 值。
    pub fn byte_at(&self, i: usize) -> Result<i8, JniError> {
        match self.values.get(i) {
            Some(JValue::Byte(v)) => Ok(*v),
            Some(v) => Err(JniError::TypeMismatch { expected: JType::Byte, actual: v.jtype() }),
            None => Err(JniError::ArgCountMismatch { expected: i + 1, actual: self.values.len() }),
        }
    }

    /// 按索引取出 Char 值。
    pub fn char_at(&self, i: usize) -> Result<u16, JniError> {
        match self.values.get(i) {
            Some(JValue::Char(v)) => Ok(*v),
            Some(v) => Err(JniError::TypeMismatch { expected: JType::Char, actual: v.jtype() }),
            None => Err(JniError::ArgCountMismatch { expected: i + 1, actual: self.values.len() }),
        }
    }

    /// 按索引取出 Short 值。
    pub fn short_at(&self, i: usize) -> Result<i16, JniError> {
        match self.values.get(i) {
            Some(JValue::Short(v)) => Ok(*v),
            Some(v) => Err(JniError::TypeMismatch { expected: JType::Short, actual: v.jtype() }),
            None => Err(JniError::ArgCountMismatch { expected: i + 1, actual: self.values.len() }),
        }
    }

    /// 按索引取出 Int 值。
    pub fn int_at(&self, i: usize) -> Result<i32, JniError> {
        match self.values.get(i) {
            Some(JValue::Int(v)) => Ok(*v),
            Some(v) => Err(JniError::TypeMismatch { expected: JType::Int, actual: v.jtype() }),
            None => Err(JniError::ArgCountMismatch { expected: i + 1, actual: self.values.len() }),
        }
    }

    /// 按索引取出 Long 值。
    pub fn long_at(&self, i: usize) -> Result<i64, JniError> {
        match self.values.get(i) {
            Some(JValue::Long(v)) => Ok(*v),
            Some(v) => Err(JniError::TypeMismatch { expected: JType::Long, actual: v.jtype() }),
            None => Err(JniError::ArgCountMismatch { expected: i + 1, actual: self.values.len() }),
        }
    }

    /// 按索引取出 Float 值。
    pub fn float_at(&self, i: usize) -> Result<f32, JniError> {
        match self.values.get(i) {
            Some(JValue::Float(v)) => Ok(*v),
            Some(v) => Err(JniError::TypeMismatch { expected: JType::Float, actual: v.jtype() }),
            None => Err(JniError::ArgCountMismatch { expected: i + 1, actual: self.values.len() }),
        }
    }

    /// 按索引取出 Double 值。
    pub fn double_at(&self, i: usize) -> Result<f64, JniError> {
        match self.values.get(i) {
            Some(JValue::Double(v)) => Ok(*v),
            Some(v) => Err(JniError::TypeMismatch { expected: JType::Double, actual: v.jtype() }),
            None => Err(JniError::ArgCountMismatch { expected: i + 1, actual: self.values.len() }),
        }
    }

    /// 按索引取出 Object 引用。Null 也当作 Object 返回 None。
    pub fn object_at(&self, i: usize) -> Result<Option<ObjectId>, JniError> {
        match self.values.get(i) {
            Some(JValue::Object(id)) => Ok(Some(*id)),
            Some(JValue::Null) => Ok(None),
            Some(v) => Err(JniError::TypeMismatch {
                expected: JType::Object(String::new()),
                actual: v.jtype(),
            }),
            None => Err(JniError::ArgCountMismatch { expected: i + 1, actual: self.values.len() }),
        }
    }
}

impl Default for JniArgs {
    fn default() -> Self {
        Self::new()
    }
}
