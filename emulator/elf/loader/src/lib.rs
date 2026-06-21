//! `rundroid-elf-loader`
//!
//! ELF loader（单模块装载，不做跨模块符号解析）。
//! 负责 PT_LOAD 映射、段权限、load bias、TLS 基础布局、导出表建立，
//! 把待解析 relocation 以 [`PendingRelocation`] 形式交给 linker。
//!
//! 与 backend / memory 的交互全部通过 [`LoadContext`] trait 中转，
//! loader 本身不持有 Unicorn 句柄，方便单元测试用 mock context 验证布局逻辑。

#![forbid(unsafe_code)]

pub mod api;
pub mod error;
pub mod loader;
pub mod model;
pub mod relro;
pub mod tls;

pub use api::{
    ElfLoader, LoadContext, LoadRequest, MappedSegment, SegmentMapSpec,
};
pub use error::ElfLoadError;
pub use loader::DefaultLoader;
pub use model::{ExportEntry, ExportTable, InitPlan, LoadedModule, MappedSegmentInfo, PendingRelocation, RelroRange, TlsTemplate};
