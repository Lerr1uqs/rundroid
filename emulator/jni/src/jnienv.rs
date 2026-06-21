//! 最小 `JNIEnv` surface。
//!
//! foundation 阶段只提供最小可运行的 `JNIEnv` 接口：
//! call method、call static method、get/set field、local ref 管理。
//!
//! 不包含：FindClass、NewStringUTF、GetByteArrayElements 等完整 JNI 函数表。
//! 这些在后续 change 中按需扩展。

use crate::args::JniArgs;
use crate::error::JniError;
use crate::refs::RefTable;
use crate::registry::JniRegistry;
use crate::types::{FieldSig, JValue, MethodSig, ObjectId};

/// 最小 JNIEnv surface。
///
/// 持有对 registry 和 ref table 的引用。
/// 所有 JNI 方法调用和 field 访问都通过此结构完成。
///
/// # 生命周期
///
/// `JniEnvSurface` 的生命周期绑定到当前线程的 attach 状态。
/// foundation 阶段不区分不同线程的 env，简化实现。
pub struct JniEnvSurface<'r> {
    registry: &'r JniRegistry,
    refs: &'r mut RefTable,
}

impl<'r> JniEnvSurface<'r> {
    /// 从 registry 和 ref table 构造 JNIEnv。
    pub fn new(registry: &'r JniRegistry, refs: &'r mut RefTable) -> Self {
        Self { registry, refs }
    }

    // —— Method 调用 ——

    /// 调用 instance method。
    ///
    /// 从 registry 查找目标 method，按类型分发到 handler 执行。
    ///
    /// # 参数
    /// - `obj`: 目标对象 ID（guest 侧 `jobject` 指向的对象）
    /// - `sig`: 解析后的 method signature
    /// - `args`: 参数列表
    pub fn call_method(
        &mut self,
        _obj: ObjectId,
        sig: &MethodSig,
        args: JniArgs,
    ) -> Result<JValue, JniError> {
        self.registry.dispatch_call(sig, &args, self.refs)
    }

    /// 调用 static method。
    pub fn call_static_method(
        &mut self,
        _class_name: &str,
        sig: &MethodSig,
        args: JniArgs,
    ) -> Result<JValue, JniError> {
        self.registry.dispatch_static(sig, &args, self.refs)
    }

    // —— Field 访问 ——

    /// 获取 instance field 值。
    pub fn get_field(
        &self,
        _obj: ObjectId,
        sig: &FieldSig,
    ) -> Result<JValue, JniError> {
        self.registry.dispatch_field_get(sig)
    }

    /// 设置 instance field 值。
    pub fn set_field(
        &self,
        _obj: ObjectId,
        sig: &FieldSig,
        val: JValue,
    ) -> Result<(), JniError> {
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

    // —— 引用管理 ——

    /// 创建一个新的 local reference。
    ///
    /// 返回 guest 可见的 handle（u32 整数）。
    /// local ref 在 `clear_frame()` 时被自动清除。
    pub fn new_local_ref(&mut self, obj_id: ObjectId) -> u32 {
        self.refs.new_local(obj_id)
    }

    /// 删除一个 local reference。
    pub fn delete_local_ref(&mut self, handle: u32) -> Result<(), JniError> {
        self.refs.delete_local(handle)
    }

    /// 通过 handle 获取 ObjectId。
    pub fn resolve_ref(&self, handle: u32) -> Option<ObjectId> {
        self.refs.resolve(handle)
    }

    /// 清理当前 frame 的 local refs。
    pub fn clear_frame(&mut self) {
        self.refs.clear_frame();
    }
}
