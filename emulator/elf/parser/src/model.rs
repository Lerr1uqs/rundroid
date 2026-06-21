//! 解析结果数据模型。
//!
//! 所有结构都是不可变的值类型（Owned + Clone），
//! loader / linker 拿到 [`ParsedElf`] 之后不会回写本层状态，
//! 这是 parser / loader / linker 三层解耦的硬约束。

/// 顶层解析结果。
#[derive(Debug, Clone)]
pub struct ParsedElf {
    /// 输入时给出的模块名（透传，便于 loader telemetry）。
    pub module_name: String,
    /// 文件级身份信息。
    pub file: ElfIdentity,
    /// PT_LOAD 段（已按 vaddr 排序）。
    pub segments: Vec<LoadSegment>,
    /// PT_DYNAMIC 提炼出的关键字段。
    pub dynamic: DynamicInfo,
    /// `.dynsym` 导出 / 导入符号。
    pub symbols: Vec<DynSymbol>,
    /// 归一化后的重定位记录（REL / RELA / Android packed 统一到这里）。
    pub relocations: Vec<RelocationRecord>,
    /// init / fini 相关元数据。
    pub init: InitMetadata,
    /// 解析过程中产生的非致命提示。
    pub notes: Vec<ParseNote>,
}

/// ELF 身份信息（class / endianness / arch / type）。
#[derive(Debug, Clone)]
pub struct ElfIdentity {
    pub is_64bit: bool,
    pub little_endian: bool,
    pub machine: u16,
    /// `e_type`：1 = REL, 2 = EXEC, 3 = DYN。
    pub e_type: u16,
    pub entry: u64,
}

/// 一段可装载的 PT_LOAD 段（已合并 p_filesz / p_memsz 语义）。
#[derive(Debug, Clone, Copy)]
pub struct LoadSegment {
    /// 在文件中的偏移。
    pub file_offset: u64,
    /// 段在 guest 地址空间中的虚拟地址（相对模块基址的偏移 = vaddr）。
    pub vaddr: u64,
    /// 文件中实际数据长度。
    pub filesz: u64,
    /// 段在内存中的总长度（>= filesz，差额为零填充）。
    pub memsz: u64,
    /// 段权限。
    pub perms: SegmentPerms,
    /// 对齐约束。
    pub align: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SegmentPerms {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

/// dynamic table 关键字段。
#[derive(Debug, Clone, Default)]
pub struct DynamicInfo {
    /// `DT_SONAME` 解析后的字符串（review finding 2：parser 直接给字符串，
    /// 不再只给 strtab offset，让 linker / loader 无需再二次解析）。
    pub soname: Option<String>,
    /// `DT_NEEDED` 解析后的依赖 .so 名字列表（顺序保留）。
    pub needed: Vec<String>,
    /// `DT_INIT` 地址（相对模块基址）。
    pub init: Option<u64>,
    /// `DT_FINI` 地址。
    pub fini: Option<u64>,
    /// `DT_INIT_ARRAY` 地址。
    pub init_array: Option<u64>,
    /// `DT_INIT_ARRAYSZ` 字节数。
    pub init_array_size: u64,
    /// `DT_FINI_ARRAY` 地址。
    pub fini_array: Option<u64>,
    pub fini_array_size: u64,
    /// 是否启用 RELRO（PT_GNU_RELRO 存在即视为启用）。
    pub relro: bool,
}

/// 一个 `.dynsym` 条目。
#[derive(Debug, Clone)]
pub struct DynSymbol {
    pub name: String,
    /// 符号值：对函数 / 数据 = 相对模块基址的偏移；对未定义符号 = 0。
    pub value: u64,
    pub size: u64,
    pub visibility: SymbolVisibility,
    pub binding: SymbolBinding,
    /// 符号所在的 section index；`0`（SHN_UNDEF）表示"导入"。
    pub shndx: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolVisibility {
    Default,
    Hidden,
    Protected,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolBinding {
    Local,
    Global,
    Weak,
    Other,
}

/// 归一化后的重定位记录。
///
/// parser 负责把 REL / RELA / Android packed 都映射成这个统一形态，
/// linker 不需要再处理 packed encoding。
#[derive(Debug, Clone, Copy)]
pub struct RelocationRecord {
    /// 重定位写回的目标地址（相对模块基址）。
    pub offset: u64,
    /// 符号在 `.dynsym` 中的 index；`None` 表示相对重定位（R_AARCH64_RELATIVE）。
    pub symbol_index: Option<u32>,
    /// 加数。REL 类型由 parser 解析隐式加数后填到这里，统一为显式形态。
    pub addend: i64,
    pub kind: RelocationKind,
}

/// AArch64 bootstrap 范围内的重定位类型。
///
/// 仅列出 spec 要求的最小集合（design.md "Key Decisions 3"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelocationKind {
    /// `R_AARCH64_RELATIVE`：写回 base + addend。
    Relative,
    /// `R_AARCH64_GLOB_DAT`
    GlobDat,
    /// `R_AARCH64_JUMP_SLOT`
    JumpSlot,
    /// `R_AARCH64_ABS64`
    Abs64,
    /// parser 已识别但不属于 bootstrap 最小集合的类型，
    /// linker 在遇到时按"未支持"报错。
    Other(u32),
}

/// init / fini 元数据（从 [`DynamicInfo`] 抽出，便于 linker 调度）。
#[derive(Debug, Clone, Default)]
pub struct InitMetadata {
    pub has_init: bool,
    pub has_init_array: bool,
    pub init_array_count: u64,
    pub has_fini: bool,
    pub has_fini_array: bool,
    pub fini_array_count: u64,
}

/// 解析过程中的非致命提示。
#[derive(Debug, Clone)]
pub struct ParseNote {
    pub message: String,
}
