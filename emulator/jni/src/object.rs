//! JNI 对象模型。
//!
//! foundation 阶段只定义最小模型：对象 ID + class name + 可选的自定义数据。
//! 完整对象模型（字段存储、继承链、虚拟方法表）留到后续 change。

use crate::types::ObjectId;
use std::any::Any;

/// JNI 对象。
///
/// foundation 阶段仅保留身份信息和可选的 Rust 侧附加数据。
/// guest 看到的 handle 由 `RefTable` 管理，`ObjectId` 是 Rust 内部标识。
#[derive(Debug)]
pub struct JavaObject {
    /// Rust 内部对象 ID。
    pub id: ObjectId,
    /// 对象的 slash-separated class name。
    pub class_name: String,
    /// 可选的 Rust 侧附加数据（例如自定义状态）。
    pub data: Option<Box<dyn Any + Send>>,
}

impl JavaObject {
    /// 创建新对象。
    pub fn new(id: ObjectId, class_name: String) -> Self {
        Self { id, class_name, data: None }
    }

    /// 创建带附加数据的对象。
    pub fn with_data(id: ObjectId, class_name: String, data: Box<dyn Any + Send>) -> Self {
        Self { id, class_name, data: Some(data) }
    }
}
