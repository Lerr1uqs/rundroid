//! JNI 对象模型。
//!
//! [`JavaObject`] 是对象的轻量视图（`ObjectId` + `class_name`），
//! 实际数据权威存储在 [`ObjectStore`](crate::object_store::ObjectStore) 中。
//!
//! # 设计原则
//!
//! - `JavaObject` 不持有数据——它只是一个身份标识
//! - 对象数据通过 `ObjectStore` 访问，避免双写和副本陈旧
//! - 工厂方法返回 `(ObjectId, String, ObjectStorage)` 三元组，
//!   调用方自行 `ObjectStore::insert()`

use crate::object_store::ObjectStorage;
use crate::types::{JType, JValue, ObjectId};

/// JNI 对象视图——轻量身份标识。
///
/// 仅持有 `ObjectId` 和 `class_name`，不持有数据。
/// 实际对象数据通过 [`ObjectStore`](crate::object_store::ObjectStore) 访问。
///
/// 这是有意为之：`ObjectStore` 是对象数据的唯一权威，
/// `JavaObject` 只是该存储中一条记录的轻量引用。
#[derive(Debug, Clone)]
pub struct JavaObject {
    /// Rust 内部对象 ID。
    pub id: ObjectId,
    /// 对象的 slash-separated class name。
    pub class_name: String,
}

impl JavaObject {
    /// 从 ObjectId 和 class name 创建对象视图。
    pub fn new(id: ObjectId, class_name: String) -> Self {
        Self { id, class_name }
    }
}

// ============================================================================
// 工厂函数 —— 构造 (ObjectId, class_name, ObjectStorage) 三元组
// ============================================================================

/// 构造 Java String 对象的三元组。
///
/// `value` 为 UTF-8 字符串内容。
/// 返回 `(ObjectId, class_name, ObjectStorage)`，调用方自行 `ObjectStore::insert()`。
pub fn make_string(id: ObjectId, value: String) -> (ObjectId, String, ObjectStorage) {
    (id, "java/lang/String".to_string(), ObjectStorage::String(value))
}

/// 构造 primitive wrapper 对象的三元组。
///
/// class name 自动推断（如 `JType::Int` → `"java/lang/Integer"`）。
pub fn make_wrapper(id: ObjectId, jtype: JType, value: JValue) -> (ObjectId, String, ObjectStorage) {
    let class_name = wrapper_class_name(&jtype);
    (id, class_name, ObjectStorage::Wrapper { jtype, value })
}

/// 构造 primitive 数组对象的三元组。
///
/// class name 使用 canonical JNI descriptor 格式（如 `[I`、`[B`），
/// 而非 human-readable 名称。
pub fn make_primitive_array(
    id: ObjectId,
    jtype: JType,
    elements: Vec<JValue>,
) -> (ObjectId, String, ObjectStorage) {
    let class_name = primitive_array_class_name(&jtype);
    (id, class_name, ObjectStorage::PrimitiveArray { jtype, elements })
}

/// 构造对象数组的三元组。
pub fn make_object_array(
    id: ObjectId,
    element_class: String,
    elements: Vec<ObjectId>,
) -> (ObjectId, String, ObjectStorage) {
    let class_name = format!("[L{element_class};");
    (id, class_name, ObjectStorage::ObjectArray { class_name: element_class, elements })
}

/// 构造 framework stub 实例的三元组。
pub fn make_stub(
    id: ObjectId,
    class_name: String,
    data: Box<dyn std::any::Any + Send + Sync>,
) -> (ObjectId, String, ObjectStorage) {
    (id, class_name, ObjectStorage::StubInstance { data })
}

/// 构造 host-side 值对象的三元组。
pub fn make_host_value(
    id: ObjectId,
    class_name: String,
    data: Box<dyn std::any::Any + Send + Sync>,
) -> (ObjectId, String, ObjectStorage) {
    (id, class_name, ObjectStorage::HostValue { data })
}

// ============================================================================
// 内部辅助
// ============================================================================

/// 根据 primitive JType 推断对应的 wrapper class name。
fn wrapper_class_name(jtype: &JType) -> String {
    match jtype {
        JType::Boolean => "java/lang/Boolean".to_string(),
        JType::Byte => "java/lang/Byte".to_string(),
        JType::Char => "java/lang/Character".to_string(),
        JType::Short => "java/lang/Short".to_string(),
        JType::Int => "java/lang/Integer".to_string(),
        JType::Long => "java/lang/Long".to_string(),
        JType::Float => "java/lang/Float".to_string(),
        JType::Double => "java/lang/Double".to_string(),
        _ => "java/lang/Object".to_string(),
    }
}

/// 生成 primitive 数组的 canonical JNI class name。
///
/// 使用 JNI descriptor 单字符表示（如 `I`、`B`、`Z`），
/// 而非 human-readable 名称（如 `int`、`byte`、`boolean`）。
/// 结果如 `"[I"`（int[]）、`"[B"`（byte[]）。
fn primitive_array_class_name(jtype: &JType) -> String {
    let ch = jtype.primitive_char()
        .unwrap_or('?');
    format!("[{ch}")
}
