//! Framework builtin handler 捕获的共享 VM 状态。
//!
//! Rust builtin method handler 是 `Fn(&JniArgs) -> Result<JValue, JniError>` 闭包，
//! 签名上拿不到 `ObjectStore` / `ApkContext` / `ServiceRegistry`。为了让 framework
//! stub 能读 String/byte[] 入参、返回 Java 对象、查 package/signature/service 数据，
//! handler 闭包捕获一份 [`FrameworkCtx`]——里面是各 VM 状态的 `Arc<Mutex/RwLock>` 句柄。
//!
//! 这与 Python marshalling 路径（`AndroidRuntime::objects()` / `object_id_allocator()`）
//! 用的是同一模式：handler 经共享句柄访问对象池，不另起一套对象存储。

use crate::apk_context::ApkContext;
use crate::args::JniArgs;
use crate::error::JniError;
use crate::framework::service::ServiceRegistry;
use crate::object_store::{ObjectStorage, ObjectStore};
use crate::types::{IdAllocator, ObjectId};
use std::sync::{Arc, Mutex, RwLock};

// ============================================================================
// FrameworkCtx
// ============================================================================

/// builtin handler 共享的 VM 状态句柄集合。
///
/// 每个字段都是 `Arc` 包的共享可变状态，闭包按值 clone 一份 `FrameworkCtx`
/// 即可捕获全部句柄（Arc clone 廉价）。
#[derive(Clone)]
pub struct FrameworkCtx {
    /// 对象池（读入参对象 / 写返回对象）。
    pub objects: Arc<Mutex<ObjectStore>>,
    /// ObjectId 分配器（为返回的 Java 对象分配 id）。
    pub id_alloc: Arc<Mutex<IdAllocator>>,
    /// APK context（package / version / signatures / assets）。
    /// `RwLock` 因为 framework 读多写少，且支持 install 后 live 更新（mock 数据路径）。
    pub apk: Arc<RwLock<Option<ApkContext>>>,
    /// service registry（`getSystemService` 查询）。
    pub services: Arc<Mutex<ServiceRegistry>>,
    /// framework 单例 stub（PackageManager / ApplicationInfo / AssetManager），
    /// 在 `install` 时分配，供 Context 的 getter 返回稳定实例。
    pub singletons: Arc<Mutex<FrameworkSingletons>>,
}

/// framework 单例 stub 句柄集合。
///
/// `Context.getPackageManager()` / `getApplicationInfo()` / `getAssets()` 在真实
/// Android 上返回稳定的 manager 实例；这里在 `install` 时分配一次，后续 getter
/// 返回同一个 ObjectId。
#[derive(Debug, Default)]
pub struct FrameworkSingletons {
    /// `PackageManager` 单例。
    pub package_manager: Option<ObjectId>,
    /// `ApplicationInfo` 单例。
    pub application_info: Option<ObjectId>,
    /// `AssetManager` 单例。
    pub asset_manager: Option<ObjectId>,
}

impl std::fmt::Debug for FrameworkCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameworkCtx")
            .field("objects", &"<ObjectStore>")
            .field("id_alloc", &"<IdAllocator>")
            .field("apk", &self.apk.read().map(|a| a.is_some()))
            .field("services", &"<ServiceRegistry>")
            .finish()
    }
}

impl FrameworkCtx {
    /// 从 AndroidRuntime 的共享句柄 + framework 自有的 apk/service 句柄组装 ctx。
    pub(crate) fn new(
        objects: Arc<Mutex<ObjectStore>>,
        id_alloc: Arc<Mutex<IdAllocator>>,
        apk: Arc<RwLock<Option<ApkContext>>>,
        services: Arc<Mutex<ServiceRegistry>>,
    ) -> Self {
        Self {
            objects,
            id_alloc,
            apk,
            services,
            singletons: Arc::new(Mutex::new(FrameworkSingletons::default())),
        }
    }

    // —— 对象编组 helper（handler 锁内只取 owned 数据，绝不嵌套持锁）——

    /// 把一个 Rust 字符串落成 Java String 对象，返回其 ObjectId。
    ///
    /// 分配 oid（锁 id_alloc）→ 写 ObjectStore（锁 objects），两步不嵌套持锁。
    pub fn intern_string(&self, s: &str) -> Result<ObjectId, JniError> {
        let oid = self.id_alloc.lock().unwrap().object();
        self.objects
            .lock()
            .unwrap()
            .insert(oid, "java/lang/String".to_string(), ObjectStorage::String(s.to_string()))
            .map_err(|_| JniError::Internal(format!("intern_string 时 ObjectId {oid} 已存在")))?;
        Ok(oid)
    }

    /// 把字节切片落成 Java `byte[]` 对象，返回其 ObjectId。
    pub fn intern_byte_array(&self, bytes: &[u8]) -> Result<ObjectId, JniError> {
        let elements: Vec<crate::types::JValue> =
            bytes.iter().map(|b| crate::types::JValue::Byte(*b as i8)).collect();
        let oid = self.id_alloc.lock().unwrap().object();
        self.objects
            .lock()
            .unwrap()
            .insert(
                oid,
                "[B".to_string(),
                ObjectStorage::PrimitiveArray {
                    jtype: crate::types::JType::Byte,
                    elements,
                },
            )
            .map_err(|_| JniError::Internal(format!("intern_byte_array 时 ObjectId {oid} 已存在")))?;
        Ok(oid)
    }

    /// 分配一个 stub 实例对象并写入对象池，返回其 ObjectId。
    ///
    /// 用于构造 PackageInfo / Signature / service manager 等 framework stub 实例。
    pub fn intern_stub(&self, class_name: &str, storage: ObjectStorage) -> Result<ObjectId, JniError> {
        let oid = self.id_alloc.lock().unwrap().object();
        self.objects
            .lock()
            .unwrap()
            .insert(oid, class_name.to_string(), storage)
            .map_err(|_| JniError::Internal(format!("intern_stub 时 ObjectId {oid} 已存在")))?;
        Ok(oid)
    }

    /// 读取第 i 个参数为 Java String，返回其 UTF-8 内容。
    ///
    /// `Null` 入参返回 `Ok(None)`；非 String 对象类型不匹配 fail-fast。
    /// 锁内只 clone 出 String 数据即释放锁。
    pub fn read_string_arg(&self, args: &JniArgs, i: usize) -> Result<Option<String>, JniError> {
        let oid = match args.object_at(i)? {
            Some(oid) => oid,
            None => return Ok(None),
        };
        let store = self.objects.lock().unwrap();
        match store.storage(oid) {
            Some(ObjectStorage::String(s)) => Ok(Some(s.clone())),
            Some(other) => Err(JniError::TypeMismatch {
                expected: crate::types::JType::Object("java/lang/String".into()),
                actual: crate::types::JType::Object(other.kind_label().into()),
            }),
            None => Err(JniError::Internal(format!("String 参数 ObjectId {oid} 不在对象池"))),
        }
    }

    /// 读取第 i 个参数为 `byte[]`，返回其字节内容。
    ///
    /// `Null` 入参返回 `Ok(None)`。仅接受 `PrimitiveArray(Byte)`，其它 fail-fast。
    pub fn read_byte_array_arg(&self, args: &JniArgs, i: usize) -> Result<Option<Vec<u8>>, JniError> {
        let oid = match args.object_at(i)? {
            Some(oid) => oid,
            None => return Ok(None),
        };
        let store = self.objects.lock().unwrap();
        match store.storage(oid) {
            Some(ObjectStorage::PrimitiveArray { jtype, elements }) => {
                if !matches!(jtype, crate::types::JType::Byte) {
                    return Err(JniError::TypeMismatch {
                        expected: crate::types::JType::Array(Box::new(crate::types::JType::Byte)),
                        actual: jtype.clone(),
                    });
                }
                let bytes: Result<Vec<u8>, JniError> = elements
                    .iter()
                    .map(|v| match v {
                        crate::types::JValue::Byte(b) => Ok(*b as u8),
                        other => Err(JniError::TypeMismatch {
                            expected: crate::types::JType::Byte,
                            actual: other.jtype(),
                        }),
                    })
                    .collect();
                Ok(Some(bytes?))
            }
            Some(other) => Err(JniError::TypeMismatch {
                expected: crate::types::JType::Array(Box::new(crate::types::JType::Byte)),
                actual: crate::types::JType::Object(other.kind_label().into()),
            }),
            None => Err(JniError::Internal(format!("byte[] 参数 ObjectId {oid} 不在对象池"))),
        }
    }

    /// 按 service name 查询 stub oid（`getSystemService` handler 用）。
    pub fn lookup_service(&self, name: &str) -> Option<ObjectId> {
        self.services.lock().unwrap().lookup(name)
    }

    /// 读取一个 StubInstance 对象的内部数据（owned clone）。
    ///
    /// 锁内 downcast + clone 后立即释放锁，handler 在锁外使用 owned 数据。
    /// 非 StubInstance 或类型不匹配 fail-fast。
    pub fn read_stub<T>(&self, oid: ObjectId) -> Result<T, JniError>
    where
        T: Clone + Send + Sync + 'static,
    {
        let store = self.objects.lock().unwrap();
        match store.storage(oid) {
            Some(ObjectStorage::StubInstance { data }) => data
                .downcast_ref::<T>()
                .map(|v| v.clone())
                .ok_or_else(|| JniError::Internal("StubInstance 数据类型不匹配".into())),
            Some(other) => Err(JniError::Internal(format!(
                "ObjectId {oid} 不是 StubInstance（实际 {}）",
                other.kind_label()
            ))),
            None => Err(JniError::Internal(format!("ObjectId {oid} 不在对象池"))),
        }
    }

    /// 读取一个 primitive wrapper（Integer/Long/Boolean…）对象的当前值。
    pub fn read_wrapper_value(&self, oid: ObjectId) -> Result<crate::types::JValue, JniError> {
        let store = self.objects.lock().unwrap();
        match store.storage(oid) {
            Some(ObjectStorage::Wrapper { value, .. }) => Ok(value.clone()),
            Some(other) => Err(JniError::Internal(format!(
                "ObjectId {oid} 不是 Wrapper（实际 {}）",
                other.kind_label()
            ))),
            None => Err(JniError::Internal(format!("ObjectId {oid} 不在对象池"))),
        }
    }

    /// 读取一个 String 对象的内容。
    pub fn read_string_value(&self, oid: ObjectId) -> Result<String, JniError> {
        let store = self.objects.lock().unwrap();
        match store.storage(oid) {
            Some(ObjectStorage::String(s)) => Ok(s.clone()),
            Some(other) => Err(JniError::Internal(format!(
                "ObjectId {oid} 不是 String（实际 {}）",
                other.kind_label()
            ))),
            None => Err(JniError::Internal(format!("ObjectId {oid} 不在对象池"))),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::JniArgs;
    use crate::object_store::ObjectStorage;
    use crate::types::{JValue, ObjectId};
    use std::sync::{Arc, Mutex, RwLock};

    fn empty_ctx() -> FrameworkCtx {
        FrameworkCtx::new(
            Arc::new(Mutex::new(ObjectStore::new())),
            Arc::new(Mutex::new(IdAllocator::new())),
            Arc::new(RwLock::new(None)),
            Arc::new(Mutex::new(ServiceRegistry::new())),
        )
    }

    #[test]
    fn intern_string_roundtrips() {
        let ctx = empty_ctx();
        let oid = ctx.intern_string("hello").unwrap();
        let store = ctx.objects.lock().unwrap();
        match store.storage(oid) {
            Some(ObjectStorage::String(s)) => assert_eq!(s, "hello"),
            other => panic!("expected String, got {other:?}"),
        }
        assert_eq!(store.class_name(oid), Some("java/lang/String"));
    }

    #[test]
    fn read_string_arg_extracts_value() {
        let ctx = empty_ctx();
        let oid = ctx.intern_string("pkg.name").unwrap();
        let args = JniArgs::from_vec(vec![JValue::Object(oid)]);
        assert_eq!(ctx.read_string_arg(&args, 0).unwrap(), Some("pkg.name".into()));
    }

    #[test]
    fn read_string_arg_null_returns_none() {
        let ctx = empty_ctx();
        let args = JniArgs::from_vec(vec![JValue::Null]);
        assert_eq!(ctx.read_string_arg(&args, 0).unwrap(), None);
    }

    #[test]
    fn read_byte_array_arg_extracts_bytes() {
        let ctx = empty_ctx();
        // make_primitive_array 写入的是一个新的 ObjectStore；这里直接手工构造并插入。
        let elements = vec![JValue::Byte(0x01), JValue::Byte(0x02), JValue::Byte(0x03)];
        let oid = ctx.id_alloc.lock().unwrap().object();
        ctx.objects.lock().unwrap().insert(
            oid,
            "[B".into(),
            ObjectStorage::PrimitiveArray { jtype: crate::types::JType::Byte, elements },
        ).unwrap();
        let args = JniArgs::from_vec(vec![JValue::Object(oid)]);
        assert_eq!(ctx.read_byte_array_arg(&args, 0).unwrap(), Some(vec![1, 2, 3]));
    }

    #[test]
    fn lookup_service_finds_registered() {
        let ctx = empty_ctx();
        ctx.services.lock().unwrap().register("phone", ObjectId(99));
        assert_eq!(ctx.lookup_service("phone"), Some(ObjectId(99)));
        assert_eq!(ctx.lookup_service("nope"), None);
    }
}
