//! Android framework stubs —— class-spec 驱动的 framework registry。
//!
//! 本模块把 Android framework / Java utility class 的行为收敛成注册式 class module，
//! 取代 unidbg `AbstractJni` 那种 giant signature switch。
//!
//! # 结构
//!
//! - [`spec`]：`FrameworkClassSpec` 等声明式数据结构 + 物化成 `JClassDef`
//! - [`service`]：`ServiceRegistry`，`getSystemService` 的统一查询入口
//! - [`context`]：`FrameworkCtx`，builtin handler 捕获的共享 VM 状态
//! - [`catalog`]：全部 builtin class 的声明式定义（Context / PackageManager / Signature …）
//! - [`registry`]：`FrameworkRegistry`，装配入口——`install()` 收敛进 `JniRegistry`
//!
//! # 收敛主线
//!
//! Rust builtin 与 Python shim 都写入 `JniRegistry` 持有的统一 `JClassDef` authority，
//! 通过 `register_or_merge_class` 注册。`FrameworkRegistry.classes` 仅是 catalog 自省，
//! 不参与 dispatch。详见 change `android-framework-stubs` 的 design.md。

pub mod catalog;
pub mod context;
pub mod registry;
pub mod service;
pub mod spec;

pub use context::{FrameworkCtx, FrameworkSingletons};
pub use registry::FrameworkRegistry;
pub use service::{ServiceEntry, ServiceRegistry, DEFAULT_SERVICE_NAMES};
pub use spec::{
    FrameworkClassSpec, FrameworkClassSpecBuilder, FrameworkConstructorSpec, FrameworkFieldSpec,
    FrameworkMethodSpec,
};
