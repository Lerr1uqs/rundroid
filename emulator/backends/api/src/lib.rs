//! `rundroid-backend`
//!
//! backend 抽象层。定义与具体 emulator（Unicorn / QEMU / ...）无关的 trait，
//! 让上层 runtime 只依赖抽象接口，而不直接绑定 Unicorn-specific API。
//!
//! bootstrap 阶段的 API 范围刻意只覆盖"跑通一个 ARM64 stub"所需的最小操作：
//! 内存映射、内存读写、寄存器读写、单次/有界 emu_start。
//! 后续 hook、syscall、内存权限切换等接口在对应 task 中再扩展。

#![forbid(unsafe_code)]

pub mod engine;
pub mod error;
pub mod mem;
pub mod reg;

pub use engine::{Backend, Engine, SyscallCpu, SyscallHook};
pub use error::BackendError;
pub use mem::MemPerms;
pub use reg::Arm64Reg;
