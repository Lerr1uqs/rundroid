//! `rundroid-elf-linker`
//!
//! ELF linker。消费 [`LoadedModule::unresolved`]，完成：
//! - 依赖图（`DT_NEEDED`）拓扑排序
//! - 符号查找（先模块内导出表，再依赖图）
//! - relocation 写回（bootstrap 最小集：RELATIVE / GLOB_DAT / JUMP_SLOT / ABS64）
//! - init 调度（输出稳定的 `init_order`）
//!
//! 与 parser / loader 一样，linker 不直接接触 backend 句柄，
//! 全部副作用通过 [`LinkContext`] 中转。

#![forbid(unsafe_code)]

pub mod error;
pub mod init;
pub mod linker;
pub mod model;
pub mod reloc_aarch64;
pub mod resolver;

pub use error::ElfLinkError;
pub use linker::{DefaultLinker, LinkContext};
pub use model::{LinkReport, ModuleGraph, ResolvedSymbol, SymbolQuery, UnresolvedSymbol};
pub use reloc_aarch64::RelocationPatch;
pub use resolver::{resolve, SymbolSource};
