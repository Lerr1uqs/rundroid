//! JNI 对象存储 — 分层对象模型。
//!
//! [`ObjectStorage`] 区分对象的具体种类（string / wrapper / array / stub 等），
//! 避免全部塞进 `Box<dyn Any>` 然后运行时猜测类型。
//!
//! [`ObjectStore`] 是 `ObjectId → (class_name, storage)` 的权威映射表，
//! 所有 guest 可见的 Java 对象都必须在此取得正式 `ObjectId`。

use crate::types::{JType, JValue, ObjectId};
use std::any::Any;
use std::collections::HashMap;

// ============================================================================
// ObjectStorage — 对象数据的分层存储
// ============================================================================

/// Java 对象数据的类型化存储。
///
/// 不鼓励使用 `HostValue`（`Box<dyn Any>`）作为通用兜底；
/// 优先使用具体变体让 dispatch 层可以不经类型猜测直接访问。
#[derive(Debug)]
pub enum ObjectStorage {
    /// Java String（UTF-8 编码）。
    String(String),

    /// Primitive wrapper（如 `Integer`、`Boolean`）。
    /// `jtype` 标记 wrapper 对应的 primitive 类型，
    /// `value` 是当前值。
    Wrapper {
        /// wrapper 对应的 primitive 类型（如 `JType::Int` 对应 `Integer`）。
        jtype: JType,
        /// 当前值。
        value: JValue,
    },

    /// Primitive 数组（如 `byte[]`、`int[]`）。
    /// 每个元素存储为 `JValue`。
    PrimitiveArray {
        /// 元素类型。
        jtype: JType,
        /// 元素列表。
        elements: Vec<JValue>,
    },

    /// 对象数组（如 `String[]`、`Object[]`）。
    /// 每个元素是一个 `ObjectId`（也可以是 Null 以 `ObjectId(0)` 表示）。
    ObjectArray {
        /// 数组元素类型（class name）。
        class_name: String,
        /// 元素 ObjectId 列表。
        elements: Vec<ObjectId>,
    },

    /// Framework stub 实例——由 Rust builtin 提供 backing 的对象。
    /// 例如 `Signature`、`PackageInfo` 等 framework class 的实例。
    StubInstance {
        /// Rust 侧持有的任意数据。
        data: Box<dyn Any + Send>,
    },

    /// 通用 host 侧值——主要用于 Python shim 提供的对象。
    /// 仅在 Rust 侧不需要理解内部结构时使用。
    HostValue {
        /// host 侧持有的任意数据。
        data: Box<dyn Any + Send>,
    },
}

impl ObjectStorage {
    /// 返回此 storage 变体的描述标签（用于调试/telemetry）。
    pub fn kind_label(&self) -> &'static str {
        match self {
            ObjectStorage::String(_) => "string",
            ObjectStorage::Wrapper { .. } => "wrapper",
            ObjectStorage::PrimitiveArray { .. } => "primitive_array",
            ObjectStorage::ObjectArray { .. } => "object_array",
            ObjectStorage::StubInstance { .. } => "stub_instance",
            ObjectStorage::HostValue { .. } => "host_value",
        }
    }
}

// ============================================================================
// ObjectStore — ObjectId → 对象记录的权威映射
// ============================================================================

/// 对象存储中的一条记录。
#[derive(Debug)]
struct ObjectRecord {
    /// 对象所属的 slash-separated class name。
    class_name: String,
    /// 对象数据。
    storage: ObjectStorage,
}

/// JNI 对象存储表。
///
/// 所有 guest 可见的 Java 对象都必须在此存储中注册。
/// Python binding 层通过 `ObjectId` 关联到 Python backing object，
/// 不应由 `PyEmulator.java_instances` 单独充当最终对象 authority。
#[derive(Debug, Default)]
pub struct ObjectStore {
    objects: HashMap<ObjectId, ObjectRecord>,
}

impl ObjectStore {
    /// 创建空的对象存储。
    pub fn new() -> Self {
        Self {
            objects: HashMap::new(),
        }
    }

    /// 存入一个对象记录。
    ///
    /// 如果 `ObjectId` 已存在则返回错误。
    pub fn insert(
        &mut self,
        id: ObjectId,
        class_name: String,
        storage: ObjectStorage,
    ) -> Result<(), ObjectStoreError> {
        if self.objects.contains_key(&id) {
            return Err(ObjectStoreError::DuplicateId(id));
        }
        self.objects.insert(id, ObjectRecord { class_name, storage });
        Ok(())
    }

    /// 移除一个对象记录，返回其 class name 和 storage。
    pub fn remove(&mut self, id: ObjectId) -> Option<(String, ObjectStorage)> {
        self.objects.remove(&id).map(|r| (r.class_name, r.storage))
    }

    /// 查询对象的 class name。
    pub fn class_name(&self, id: ObjectId) -> Option<&str> {
        self.objects.get(&id).map(|r| r.class_name.as_str())
    }

    /// 查询对象存储的不可变引用。
    pub fn storage(&self, id: ObjectId) -> Option<&ObjectStorage> {
        self.objects.get(&id).map(|r| &r.storage)
    }

    /// 查询对象存储的可变引用。
    pub fn storage_mut(&mut self, id: ObjectId) -> Option<&mut ObjectStorage> {
        self.objects.get_mut(&id).map(|r| &mut r.storage)
    }

    /// 对象是否存在。
    pub fn contains(&self, id: ObjectId) -> bool {
        self.objects.contains_key(&id)
    }

    /// 当前存储中的对象数量。
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// 存储是否为空。
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// 遍历所有对象（用于调试/telemetry）。
    pub fn iter(&self) -> impl Iterator<Item = (ObjectId, &str, &ObjectStorage)> {
        self.objects.iter().map(|(id, r)| (*id, r.class_name.as_str(), &r.storage))
    }
}

// ============================================================================
// ObjectStoreError
// ============================================================================

/// 对象存储操作错误。
#[derive(Debug, thiserror::Error)]
pub enum ObjectStoreError {
    /// 重复的 ObjectId。
    #[error("重复的 ObjectId: {0}")]
    DuplicateId(ObjectId),
    /// 对象未找到。
    #[error("对象未找到: {0}")]
    NotFound(ObjectId),
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::JType;

    // —— ObjectStorage 测试 ——

    #[test]
    fn storage_kind_labels() {
        assert_eq!(
            ObjectStorage::String("hello".into()).kind_label(),
            "string"
        );
        assert_eq!(
            ObjectStorage::Wrapper { jtype: JType::Int, value: JValue::Int(42) }.kind_label(),
            "wrapper"
        );
        assert_eq!(
            ObjectStorage::PrimitiveArray { jtype: JType::Byte, elements: vec![] }.kind_label(),
            "primitive_array"
        );
        assert_eq!(
            ObjectStorage::ObjectArray { class_name: "java/lang/String".into(), elements: vec![] }.kind_label(),
            "object_array"
        );
        assert_eq!(
            ObjectStorage::StubInstance { data: Box::new(42i32) }.kind_label(),
            "stub_instance"
        );
        assert_eq!(
            ObjectStorage::HostValue { data: Box::new("data") }.kind_label(),
            "host_value"
        );
    }

    // —— ObjectStore 测试 ——

    #[test]
    fn store_insert_and_retrieve() {
        let mut store = ObjectStore::new();
        let id = ObjectId(1);

        store.insert(id, "java/lang/String".into(), ObjectStorage::String("hello".into())).unwrap();

        assert!(store.contains(id));
        assert_eq!(store.class_name(id), Some("java/lang/String"));
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
    }

    #[test]
    fn store_duplicate_id_fails() {
        let mut store = ObjectStore::new();
        let id = ObjectId(1);

        store.insert(id, "java/lang/String".into(), ObjectStorage::String("a".into())).unwrap();
        let result = store.insert(id, "java/lang/String".into(), ObjectStorage::String("b".into()));
        assert!(result.is_err());
    }

    #[test]
    fn store_remove_returns_data() {
        let mut store = ObjectStore::new();
        let id = ObjectId(1);

        store.insert(id, "java/lang/Integer".into(), ObjectStorage::Wrapper {
            jtype: JType::Int,
            value: JValue::Int(42),
        }).unwrap();

        let (class_name, storage) = store.remove(id).unwrap();
        assert_eq!(class_name, "java/lang/Integer");
        assert!(matches!(storage, ObjectStorage::Wrapper { .. }));
        assert!(!store.contains(id));
    }

    #[test]
    fn store_mutable_access() {
        let mut store = ObjectStore::new();
        let id = ObjectId(1);

        store.insert(id, "java/lang/Integer".into(), ObjectStorage::Wrapper {
            jtype: JType::Int,
            value: JValue::Int(10),
        }).unwrap();

        // 可变修改
        if let Some(ObjectStorage::Wrapper { value, .. }) = store.storage_mut(id) {
            *value = JValue::Int(20);
        }

        if let Some(ObjectStorage::Wrapper { value, .. }) = store.storage(id) {
            assert_eq!(*value, JValue::Int(20));
        }
    }

    #[test]
    fn store_iter_yields_all_entries() {
        let mut store = ObjectStore::new();
        store.insert(ObjectId(1), "java/lang/String".into(), ObjectStorage::String("a".into())).unwrap();
        store.insert(ObjectId(2), "java/lang/Integer".into(), ObjectStorage::Wrapper {
            jtype: JType::Int,
            value: JValue::Int(1),
        }).unwrap();

        let items: Vec<_> = store.iter().collect();
        assert_eq!(items.len(), 2);
    }

    // —— Array storage 测试 ——

    #[test]
    fn primitive_array_storage() {
        let elements = vec![
            JValue::Int(1),
            JValue::Int(2),
            JValue::Int(3),
        ];

        let storage = ObjectStorage::PrimitiveArray {
            jtype: JType::Int,
            elements,
        };

        match storage {
            ObjectStorage::PrimitiveArray { jtype, elements } => {
                assert_eq!(jtype, JType::Int);
                assert_eq!(elements.len(), 3);
                assert_eq!(elements[0], JValue::Int(1));
                assert_eq!(elements[2], JValue::Int(3));
            }
            _ => panic!("expected PrimitiveArray"),
        }
    }

    #[test]
    fn object_array_storage() {
        let obj1 = ObjectId(10);
        let obj2 = ObjectId(20);

        let storage = ObjectStorage::ObjectArray {
            class_name: "java/lang/String".into(),
            elements: vec![obj1, obj2],
        };

        match storage {
            ObjectStorage::ObjectArray { class_name, elements } => {
                assert_eq!(class_name, "java/lang/String");
                assert_eq!(elements.len(), 2);
                assert_eq!(elements[0], ObjectId(10));
            }
            _ => panic!("expected ObjectArray"),
        }
    }

    // —— String storage 测试 ——

    #[test]
    fn string_storage_utf8() {
        let storage = ObjectStorage::String("你好，世界".to_string());
        match storage {
            ObjectStorage::String(s) => assert_eq!(s, "你好，世界"),
            _ => panic!("expected String"),
        }
    }

    // —— Stub instance 测试 ——

    #[test]
    fn stub_instance_stores_arbitrary_data() {
        let storage = ObjectStorage::StubInstance { data: Box::new(42u64) };
        match storage {
            ObjectStorage::StubInstance { data } => {
                let val: &u64 = data.downcast_ref::<u64>().unwrap();
                assert_eq!(*val, 42);
            }
            _ => panic!("expected StubInstance"),
        }
    }

    /// 验证 primitive 数组 class name 使用 canonical JNI descriptor。
    /// 如 `int[]` 应为 `[I`，而非 `[int`。
    #[test]
    fn primitive_array_canonical_class_name() {
        // 通过 make_primitive_array 验证 class name
        let (_, class_name, storage) = crate::object::make_primitive_array(
            ObjectId(1),
            JType::Int,
            vec![JValue::Int(1), JValue::Int(2)],
        );
        assert_eq!(class_name, "[I", "int[] 的 canonical class name 是 [I");

        let (_, class_name_b, _) = crate::object::make_primitive_array(
            ObjectId(2),
            JType::Byte,
            vec![JValue::Byte(1)],
        );
        assert_eq!(class_name_b, "[B", "byte[] 的 canonical class name 是 [B");

        let (_, class_name_z, _) = crate::object::make_primitive_array(
            ObjectId(3),
            JType::Boolean,
            vec![JValue::Boolean(true)],
        );
        assert_eq!(class_name_z, "[Z", "boolean[] 的 canonical class name 是 [Z");

        // 验证 storage variant
        match storage {
            ObjectStorage::PrimitiveArray { jtype, elements } => {
                assert_eq!(jtype, JType::Int);
                assert_eq!(elements.len(), 2);
            }
            _ => panic!("expected PrimitiveArray"),
        }
    }
}
