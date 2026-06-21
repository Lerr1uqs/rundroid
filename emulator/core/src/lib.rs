//! `rundroid-core`
//!
//! runtime 的公共底座，bootstrap 阶段提供：
//! - [`config::RuntimeConfig`]：跨子系统开关的唯一入口
//! - [`ids`]：模块 / session 的稳定 ID 类型与分配器
//! - [`emulator::Emulator`]：emulator 装配点
//! - [`session::Session`]：单次执行 session 边界
//! - [`error`]：runtime 层统一 error model
//!
//! 这一层刻意不依赖 backend / memory / elf 实现，只定义契约，
//! 让后续 task 可以在不动 core 的情况下接入具体子系统。

#![forbid(unsafe_code)]

pub mod arch;
pub mod backend;
pub mod config;
pub mod emulator;
pub mod error;
pub mod ids;
pub mod session;

pub use arch::Arch;
pub use backend::BackendKind;
pub use config::{MemoryConfig, RuntimeConfig};
pub use emulator::Emulator;
pub use error::{ConfigError, RuntimeError, SessionError};
pub use ids::{IdAllocator, ModuleId, SessionId};
pub use session::Session;
