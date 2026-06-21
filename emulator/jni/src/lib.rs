//! `rundroid-jni`
//!
//! JNI shim foundation — Rust 持有 JNI 核心状态与分派权威，
//! Python 通过 decorator 声明 class / method / field shim。
//!
//! 当前阶段（foundation）提供：
//! - [`types`]：canonical `JType` / `JValue` / `MethodSig` / `FieldSig` 类型模型
//! - [`descriptor`]：method / field descriptor 解析器（`MethodSig::parse` / `FieldSig::parse`）
//! - [`args`]：类型化 JNI 参数获取器
//! - [`object`] / [`refs`]：最小对象模型和引用表
//! - [`registry`]：class / method / field registry，collision fail-fast
//! - [`dispatch`]：统一 Rust-native / Python-shim 分发
//! - [`verify`]：Python 注解与 Java descriptor 严格匹配校验
//! - [`jnienv`] / [`javavm`]：最小 `JNIEnv` / `JavaVM` surface
//!
//! # 设计原则
//!
//! - descriptor 在注册入口解析为 canonical typed signature，后续不再重新解析原始字符串
//! - 新增 class / method 不需要编辑中心化 switch-case
//! - registry collision 立即失败，不静默覆盖
//! - 类型不匹配在注册阶段尽早失败（fail-fast）

#![forbid(unsafe_code)]

pub mod args;
pub mod class;
pub mod descriptor;
pub mod dispatch;
pub mod error;
pub mod field;
pub mod javavm;
pub mod jnienv;
pub mod object;
pub mod refs;
pub mod registry;
pub mod types;
pub mod verify;

pub use args::JniArgs;
pub use class::{ClassBuilder, JClassDef};
pub use dispatch::MethodImpl;
pub use error::JniError;
pub use field::{FieldAccess, RustFieldHandler, SharedField};
pub use javavm::JavaVMSurface;
pub use jnienv::JniEnvSurface;
pub use object::JavaObject;
pub use refs::{RefKind, RefTable};
pub use registry::JniRegistry;
pub use types::{FieldSig, JType, JValue, MethodSig, ObjectId};
pub use verify::PythonCallableAnnotations;

/// Rust-native method handler 类型别名。
pub type RustMethodHandler = dyn Fn(&JniArgs) -> Result<JValue, JniError> + Send + Sync;
/// Python shim method 标识（u64 占位符）。
pub type ShimMethodId = u64;
