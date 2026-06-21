//! `rundroid-elf-parser`
//!
//! ELF 解析层（与 loader / linker 严格分层）。
//! 只负责把字节流转成稳定、不可变的 [`model::ParsedElf`] 快照：
//! ELF 头、program headers、dynamic table、dynsym / dynstr、relocation 归一化等。
//!
//! 不依赖 backend / memory mapper / syscall 层，
//! 因此可以在没有任何 emulator 的纯单元测试中验证。

#![forbid(unsafe_code)]

pub mod api;
pub mod error;
pub mod model;
pub mod packed;
pub mod parser_elf;

pub use api::{ElfParser, ParseInput, ParsePolicy};
pub use error::ElfParseError;
pub use model::{
    DynamicInfo, ElfIdentity, DynSymbol, InitMetadata, LoadSegment, ParseNote, ParsedElf,
    RelocationRecord, RelocationKind, SegmentPerms, SymbolBinding, SymbolVisibility,
};
pub use parser_elf::ElfCrateParser;
