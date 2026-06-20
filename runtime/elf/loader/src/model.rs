//! loader 产出的数据模型。
//!
//! [`LoadedModule`] 是 loader → linker 的唯一交接物：
//! - 已映射的镜像信息（base / load_bias / size）
//! - 导出表（供 linker 跨模块查找）
//! - 待解析的 relocation（交给 linker 写回）
//! - init 调度计划（交给 linker 排序后执行）
//! - TLS 模板（供 runtime 建立主线程 TLS）

use rundroid_core::ModuleId;
use rundroid_elf_parser::model::RelocationKind;

/// 一个已装载的模块。
#[derive(Debug, Clone)]
pub struct LoadedModule {
    pub module_id: ModuleId,
    pub name: String,
    /// load bias：模块内所有 vaddr → guest 地址的偏移量。
    /// 对 PIE/DYN（`e_type == 3`），通常等于 base；对 EXEC 可能为 0。
    pub load_bias: u64,
    /// 镜像在 guest 地址空间的起始地址。
    pub base: u64,
    /// 镜像占用的总大小（最后一个 PT_LOAD 的 end - base）。
    pub size: u64,
    pub entry: Option<u64>,
    pub tls: Option<TlsTemplate>,
    pub exports: ExportTable,
    /// 等待 linker 写回的 relocation。
    pub unresolved: Vec<PendingRelocation>,
    pub init_plan: InitPlan,
    /// 已映射段的精确权限信息（review finding 3）：
    /// runtime 装载完成后据此调用 backend `mem_protect` 收紧权限，
    /// 不再全程 RWX。
    pub segments: Vec<MappedSegmentInfo>,
    /// RELRO 区域（来自 PT_GNU_RELRO），guest 绝对地址。
    /// linker 完成 relocation 写回后据此调用 backend `mem_protect` 改只读。
    pub relro: Option<RelroRange>,
}

/// 一段已装载 PT_LOAD 的 guest 地址 + 权限。
#[derive(Debug, Clone, Copy)]
pub struct MappedSegmentInfo {
    pub guest_addr: u64,
    pub size: u64,
    pub perms: rundroid_elf_parser::model::SegmentPerms,
}

/// RELRO 区域的 guest 绝对地址范围（review finding 3）。
#[derive(Debug, Clone, Copy)]
pub struct RelroRange {
    pub start: u64,
    pub end: u64,
}

/// TLS 模板信息。
///
/// bootstrap 阶段只关心 `.tdata`/`.tbss` 的位置和大小，
/// 真正的 TCB / TPIDR_EL0 设置在 runtime 装配阶段处理。
#[derive(Debug, Clone, Copy)]
pub struct TlsTemplate {
    /// 模板在镜像内的起始 vaddr（相对 load_bias）。
    pub vaddr: u64,
    pub filesz: u64,
    pub memsz: u64,
}

/// 模块导出表。
///
/// bootstrap 阶段用一个扁平 `Vec` 暴露所有"已定义且对外可见"的符号，
/// loader 不做哈希索引（符号数量在 smoke 范围内线性查找够用），
/// 等 linker 性能瓶颈出现再换 hashtable。
#[derive(Debug, Default, Clone)]
pub struct ExportTable {
    pub symbols: Vec<ExportEntry>,
}

impl ExportTable {
    pub fn push(&mut self, entry: ExportEntry) {
        self.symbols.push(entry);
    }

    /// 按名字线性查找导出符号。
    pub fn find(&self, name: &str) -> Option<&ExportEntry> {
        self.symbols.iter().find(|e| e.name == name)
    }
}

#[derive(Debug, Clone)]
pub struct ExportEntry {
    pub name: String,
    /// 符号在 guest 地址空间的绝对地址（已加 load_bias）。
    pub guest_addr: u64,
    pub size: u64,
}

/// 等待 linker 处理的 relocation 条目。
///
/// loader 只把 parser 归一化好的 [`RelocationRecord`] 平移过来，
/// 并绑定模块 ID 与 load_bias，linker 据此完成写回。
#[derive(Debug, Clone)]
pub struct PendingRelocation {
    pub module_id: ModuleId,
    /// 重定位的目标 guest 地址（已加 load_bias）。
    pub guest_addr: u64,
    pub kind: RelocationKind,
    pub symbol_index: Option<u32>,
    /// linker 按 name 查符号，因此 loader 在这里直接给出 dynsym 中对应的符号名。
    /// 与 `symbol_index` 一一对应（RELATIVE 等无 symbol 时为 None）。
    pub symbol_name: Option<String>,
    pub addend: i64,
}

/// init 调度计划。
///
/// 注意：这里只是"本模块内部"的 init 顺序，
/// 真正的全局 init_order（跨模块拓扑序）由 linker 在依赖图建立后产出。
#[derive(Debug, Default, Clone)]
pub struct InitPlan {
    /// `DT_INIT` 对应的 guest 地址（旧式 init 函数）。
    pub legacy_init: Option<u64>,
    /// `DT_INIT_ARRAY` 中每个条目的 guest 地址，按 ELF 中的出现顺序。
    pub init_array: Vec<u64>,
}
