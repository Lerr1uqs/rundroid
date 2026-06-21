//! linker 错误模型。
//!
//! 只描述"链接阶段"问题：依赖缺失、符号未解析、relocation 写回失败、init 调度非法。
//! 不描述 ELF 格式问题（那是 parser）或映射问题（那是 loader）。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ElfLinkError {
    /// `DT_NEEDED` 声明的依赖在图中找不到对应模块。
    #[error("missing dependency `{0}` required by module #{1}")]
    MissingDependency(String, u64),

    /// 符号查找彻底失败（所有 source 都没命中）。
    #[error("unresolved symbol `{name}` referenced by module #{module:?}")]
    UnresolvedSymbol {
        name: String,
        module: rundroid_core::ModuleId,
    },

    /// relocation 写回失败（目标地址未映射 / 权限拒绝）。
    #[error("relocation write failed at {addr:#x}")]
    RelocationWrite {
        addr: u64,
        #[source]
        source: rundroid_memory::MemoryError,
    },

    /// 遇到 bootstrap 不支持的重定位类型。
    /// 不当作 fatal，由调用方决定是 fail-hard 还是当作 unresolved。
    #[error("unsupported relocation type {0:?}")]
    UnsupportedRelocation(rundroid_elf_parser::model::RelocationKind),

    /// 依赖图存在循环，无法产生稳定的 init_order。
    #[error("dependency cycle detected at module #{0:?}")]
    DependencyCycle(rundroid_core::ModuleId),
}
