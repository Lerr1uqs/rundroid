//! `rundroid-jni`
//!
//! JNI shim foundation — Rust 持有 JNI 核心状态与分派权威，
//! Python 通过 decorator 声明 class / method / field shim。
//!
//! 当前阶段提供：
//! - [`types`]：canonical `JType` / `JValue` / `MethodSig` / `FieldSig` 类型模型，
//!   以及 `ClassId` / `ObjectId` / `MethodId` / `FieldId` typed ID 体系
//! - [`descriptor`]：method / field descriptor 解析器（`MethodSig::parse` / `FieldSig::parse`）
//! - [`args`]：类型化 JNI 参数获取器
//! - [`object`] / [`object_store`]：分层对象模型（string / wrapper / array / stub instance）
//! - [`refs`]：引用表，显式区分 local / global / weak 生命周期
//! - [`registry`]：class / method / field registry，collision fail-fast
//! - [`dispatch`]：统一 Rust-native / Python-shim 分发
//! - [`verify`]：Python 注解与 Java descriptor 严格匹配校验
//! - [`jnienv`] / [`javavm`]：最小 `JNIEnv` / `JavaVM` surface
//! - [`exception`]：异常状态（pending throwable）
//! - [`apk_context`]：APK context（package / version / manifest / signatures / assets）
//! - [`android_vm`]：`AndroidVM` — class / object / ref / exception / apk 聚合根
//! - [`android_runtime`]：`AndroidRuntime` — Emulator 持有的高级整合点
//! - [`framework`]：Android framework stubs — class-spec 驱动的 framework registry，
//!   builtin class 经 `install()` 收敛进 `JniRegistry` 统一 authority
//!
//! # 设计原则
//!
//! - descriptor 在注册入口解析为 canonical typed signature，后续不再重新解析原始字符串
//! - 新增 class / method 不需要编辑中心化 switch-case
//! - registry collision 立即失败，不静默覆盖
//! - 类型不匹配在注册阶段尽早失败（fail-fast）
//! - class 是 method / field 的聚合根，不建立独立 authority 的全局 method/field registry

#![forbid(unsafe_code)]

pub mod abi;
pub mod android_runtime;
pub mod android_vm;
pub mod apk_context;
pub mod args;
pub mod class;
pub mod descriptor;
pub mod dispatch;
pub mod error;
pub mod exception;
pub mod field;
pub mod framework;
pub mod function_table;
pub mod javavm;
pub mod jnienv;
pub mod native_registry;
pub mod object;
pub mod object_store;
pub mod refs;
pub mod registry;
pub mod types;
pub mod verify;

pub use android_runtime::AndroidRuntime;
pub use android_vm::AndroidVM;
pub use apk_context::{ApkContext, SignatureData};
pub use args::JniArgs;
pub use class::{ClassBuilder, ClassKind, JClassDef, JFieldDef, JMethodDef};
pub use dispatch::MethodImpl;
pub use error::JniError;
pub use exception::{ExceptionRecord, ExceptionState};
pub use framework::{
    FrameworkClassSpec, FrameworkClassSpecBuilder, FrameworkConstructorSpec, FrameworkFieldSpec,
    FrameworkMethodSpec, FrameworkRegistry, ServiceEntry, ServiceRegistry,
};
pub use native_registry::{
    GuestPtr, NativeRegistry, mangle_java_method, mangle_java_method_overloaded,
    unmangle_java_symbol, validate_jni_version,
    JNI_VERSION_1_1, JNI_VERSION_1_2, JNI_VERSION_1_4, JNI_VERSION_1_6, JNI_VERSION_1_8,
    SUPPORTED_JNI_VERSIONS,
};
pub use field::{FieldAccess, RustFieldHandler, SharedField};
pub use abi::{
    apply_attach_current_thread, apply_detach_current_thread, apply_get_env, JNIEnvABI,
    JavaVMABI, JavaVMThreadState, JniSlotHandler, JniSlotSpec, JNI_ENV_SLOTS,
    JNI_INVOKE_ATTACH_CURRENT_THREAD, JNI_INVOKE_DETACH_CURRENT_THREAD, JNI_INVOKE_GET_ENV,
    JNI_INVOKE_SLOTS, JNI_ERR, JNI_EDETACHED, JNI_EVERSION, JNI_OK,
};
pub use javavm::JavaVMSurface;
pub use jnienv::JniEnvSurface;
pub use object::{
    JavaObject, make_host_value, make_object_array, make_primitive_array,
    make_string, make_stub, make_wrapper,
};
pub use object_store::{ObjectStorage, ObjectStore, ObjectStoreError};
pub use refs::{RefKind, RefTable};
pub use registry::JniRegistry;
pub use types::{ClassId, FieldId, FieldSig, IdAllocator, JType, JValue, MethodId, MethodSig, ObjectId};
pub use verify::PythonCallableAnnotations;

/// Rust-native method handler 类型别名。
pub type RustMethodHandler = dyn Fn(&JniArgs) -> Result<JValue, JniError> + Send + Sync;
/// Python shim method 标识（u64 占位符）。
pub type ShimMethodId = u64;
