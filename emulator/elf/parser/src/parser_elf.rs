//! 基于 [`elf`] crate 的 parser 实现。
//!
//! 这一层把 `elf` crate 的零拷贝、按需解析的 API 翻译成
//! 我们自己的 owned、不可变 [`ParsedElf`](crate::model::ParsedElf)。
//!
//! 为什么需要这层"再封装"而不是直接让 loader 用 `elf` crate：
//! - 让 loader / linker 不直接依赖第三方 API，未来换 `goblin` 等无成本
//! - 在这一层做归一化（REL/RELA/packed → 统一 [`RelocationRecord`]）
//! - 在这一层做 bootstrap 范围校验（拒绝 ELF32 / 非 AArch64）

use crate::api::{ElfParser, ParseInput};
use crate::error::ElfParseError;
use crate::model::{
    DynamicInfo, DynSymbol, ElfIdentity, InitMetadata, LoadSegment, ParseNote, ParsedElf,
    RelocationKind, RelocationRecord, SegmentPerms, SymbolBinding, SymbolVisibility,
};

use elf::abi::{DT_FINI, DT_FINI_ARRAY, DT_FINI_ARRAYSZ, DT_INIT, DT_INIT_ARRAY, DT_INIT_ARRAYSZ,
    DT_NEEDED, DT_SONAME, EM_AARCH64, PF_R, PF_W, PF_X, PT_GNU_RELRO, PT_LOAD};
use elf::endian::{AnyEndian, EndianParse};
use elf::file::Class;
use elf::ElfBytes;

/// 默认 parser，基于 `elf` crate。
#[derive(Debug, Default, Clone, Copy)]
pub struct ElfCrateParser;

impl ElfCrateParser {
    pub const fn new() -> Self {
        Self
    }
}

impl ElfParser for ElfCrateParser {
    fn parse(&self, input: ParseInput<'_>) -> Result<ParsedElf, ElfParseError> {
        parse_inner(input)
    }
}

fn parse_inner(input: ParseInput<'_>) -> Result<ParsedElf, ElfParseError> {
    // `AnyEndian` 让 `elf` crate 在解析 ehdr 时自行判定端序，
    // 我们随后根据 `is_little_endian` + machine 做范围校验。
    let file = ElfBytes::<AnyEndian>::minimal_parse(input.bytes)
        .map_err(map_parse_err)?;
    validate_identity(&file)?;

    let identity = build_identity(&file);
    let mut notes = Vec::new();

    let segments = collect_load_segments(&file);
    // DT_NEEDED / DT_SONAME 的字符串解析依赖 dynstr；先用临时 offset 列表收集，
    // 再用 dynamic_symbol_table 返回的 strtab 解析成实际字符串。
    let (dynamic, needed_offsets, soname_offset) = collect_dynamic(&file, &mut notes);
    let strtab_opt = match file.dynamic_symbol_table() {
        Ok(Some((_, strtab))) => Some(strtab),
        _ => None,
    };
    let mut dynamic = dynamic;
    if let Some(strtab) = strtab_opt.as_ref() {
        dynamic.needed = needed_offsets
            .iter()
            .map(|off| resolve_strtab(strtab, *off))
            .collect();
        if let Some(off) = soname_offset {
            dynamic.soname = Some(resolve_strtab(strtab, off));
        }
    }
    let symbols = match file.dynamic_symbol_table() {
        Ok(Some((symtab, strtab))) => collect_dyn_symbols(&symtab, &strtab, &mut notes),
        _ => Vec::new(),
    };
    let relocations = collect_relocations(&file, input.bytes, &segments, &mut notes);

    let init = build_init_metadata(&dynamic);

    Ok(ParsedElf {
        module_name: input.module_name.to_string(),
        file: identity,
        segments,
        dynamic,
        symbols,
        relocations,
        init,
        notes,
    })
}

/// 把 `elf::ParseError` 归一化到我们的 [`ElfParseError`]（review finding 5）。
///
/// 不直接透传 `elf::ParseError`，是为了让上游在替换 parser 实现时
/// 错误模型保持稳定。
fn map_parse_err(e: elf::parse::ParseError) -> ElfParseError {
    // `elf = "0.8"` 的 ParseError 变体是 pub 且稳定在 0.8 范围内。
    // 这里主动区分 BadMagic 与（大概率）截断，让调用方能据此报告。
    if matches!(e, elf::parse::ParseError::BadMagic(_)) {
        ElfParseError::BadMagic
    } else {
        ElfParseError::Truncated("elf crate minimal_parse failed")
    }
}

fn validate_identity(file: &ElfBytes<'_, AnyEndian>) -> Result<(), ElfParseError> {
    if file.ehdr.class != Class::ELF64 {
        return Err(ElfParseError::Unsupported("bootstrap requires ELF64"));
    }
    if !file.ehdr.endianness.is_little() {
        return Err(ElfParseError::Unsupported("bootstrap requires little-endian"));
    }
    if file.ehdr.e_machine != EM_AARCH64 {
        return Err(ElfParseError::Unsupported(
            "bootstrap only supports EM_AARCH64",
        ));
    }
    Ok(())
}

fn build_identity(file: &ElfBytes<'_, AnyEndian>) -> ElfIdentity {
    ElfIdentity {
        is_64bit: matches!(file.ehdr.class, Class::ELF64),
        little_endian: file.ehdr.endianness.is_little(),
        machine: file.ehdr.e_machine,
        e_type: file.ehdr.e_type,
        entry: file.ehdr.e_entry,
    }
}

/// 提取 PT_LOAD 段并按 vaddr 升序返回。
fn collect_load_segments(file: &ElfBytes<'_, AnyEndian>) -> Vec<LoadSegment> {
    let mut out: Vec<LoadSegment> = file
        .segments()
        .map(|t| t.iter())
        .into_iter()
        .flatten()
        .filter(|p| p.p_type == PT_LOAD)
        .map(|p| LoadSegment {
            file_offset: p.p_offset,
            vaddr: p.p_vaddr,
            filesz: p.p_filesz,
            memsz: p.p_memsz,
            perms: perms_from_flags(p.p_flags),
            align: p.p_align,
        })
        .collect();
    out.sort_by_key(|s| s.vaddr);
    out
}

fn perms_from_flags(flags: u32) -> SegmentPerms {
    SegmentPerms {
        read: flags & PF_R != 0,
        write: flags & PF_W != 0,
        execute: flags & PF_X != 0,
    }
}

/// 解析 PT_DYNAMIC 与 PT_GNU_RELRO，填充 [`DynamicInfo`]。
///
/// 返回 (DynamicInfo, needed_offsets, soname_offset)：
/// 字符串解析延后到外层（拿到 dynstr 之后）做，
/// 本函数只负责把 dynamic 表里的 offset 收集出来。
fn collect_dynamic(
    file: &ElfBytes<'_, AnyEndian>,
    notes: &mut Vec<ParseNote>,
) -> (DynamicInfo, Vec<u64>, Option<u64>) {
    let mut info = DynamicInfo::default();
    let mut needed_offsets: Vec<u64> = Vec::new();
    let mut soname_offset: Option<u64> = None;
    let dyn_table = match file.dynamic() {
        Ok(Some(t)) => t,
        _ => return (info, needed_offsets, soname_offset),
    };

    for entry in dyn_table.iter() {
        let tag = entry.d_tag;
        let val = entry.d_val();
        match tag {
            DT_NEEDED => needed_offsets.push(val),
            DT_SONAME if soname_offset.is_none() => {
                soname_offset = Some(val);
            }
            DT_INIT => info.init = Some(val),
            DT_FINI => info.fini = Some(val),
            DT_INIT_ARRAY => info.init_array = Some(val),
            DT_INIT_ARRAYSZ => info.init_array_size = val,
            DT_FINI_ARRAY => info.fini_array = Some(val),
            DT_FINI_ARRAYSZ => info.fini_array_size = val,
            _ => {}
        }
    }

    info.relro = file
        .segments()
        .map(|t| t.iter().any(|p| p.p_type == PT_GNU_RELRO))
        .unwrap_or(false);

    if needed_offsets.is_empty() && info.init.is_none() && info.init_array.is_none() {
        notes.push(ParseNote {
            message: "no DT_NEEDED / DT_INIT / DT_INIT_ARRAY found".to_string(),
        });
    }

    (info, needed_offsets, soname_offset)
}

/// 把 strtab offset 翻译成字符串。
/// 解析失败（越界 / 非 UTF-8）时退化为空串并记一个 note——
/// bootstrap 不希望因为 SONAME 字符串损坏而中断整个装载。
fn resolve_strtab(strtab: &elf::string_table::StringTable<'_>, offset: u64) -> String {
    strtab
        .get(offset as usize)
        .map(str::to_string)
        .unwrap_or_default()
}

/// 读取 .dynsym + .dynstr，构造 [`DynSymbol`] 列表。
fn collect_dyn_symbols(
    symtab: &elf::symbol::SymbolTable<'_, AnyEndian>,
    strtab: &elf::string_table::StringTable<'_>,
    notes: &mut Vec<ParseNote>,
) -> Vec<DynSymbol> {
    let total = symtab.len();
    let mut symbols = Vec::with_capacity(total);
    for idx in 0..total {
        let raw = match symtab.get(idx) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let name = strtab.get(raw.st_name as usize).map(str::to_string).unwrap_or_default();
        symbols.push(DynSymbol {
            name,
            value: raw.st_value,
            size: raw.st_size,
            visibility: vis_from_info(raw.st_vis()),
            binding: bind_from_info(raw.st_bind()),
            shndx: raw.st_shndx,
        });
    }
    if symbols.is_empty() {
        notes.push(ParseNote {
            message: "empty .dynsym".to_string(),
        });
    }
    symbols
}

fn vis_from_info(v: u8) -> SymbolVisibility {
    match v {
        0 => SymbolVisibility::Default,
        2 => SymbolVisibility::Hidden,
        3 => SymbolVisibility::Protected,
        _ => SymbolVisibility::Other,
    }
}

fn bind_from_info(b: u8) -> SymbolBinding {
    match b {
        0 => SymbolBinding::Local,
        1 => SymbolBinding::Global,
        2 => SymbolBinding::Weak,
        _ => SymbolBinding::Other,
    }
}

/// 收集 RELA / REL / Android packed 重定位并归一化。
///
/// 三个来源最终都映射到 [`RelocationRecord`]：
/// - `SHT_RELA`：标准 RELA，`elf` crate 直接给。
/// - `SHT_REL`：标准 REL，隐式 addend 从文件镜像里读（`r_offset` 处的 8 字节）。
/// - `SHT_ANDROID_RELA` / `SHT_ANDROID_REL`：Android packed（APS2），走 [`crate::packed`] 解码。
fn collect_relocations(
    file: &ElfBytes<'_, AnyEndian>,
    bytes: &[u8],
    segments: &[crate::model::LoadSegment],
    notes: &mut Vec<ParseNote>,
) -> Vec<RelocationRecord> {
    let mut out = Vec::new();
    let Some(shdrs) = file.section_headers() else {
        return out;
    };

    for shdr in shdrs.iter() {
        match shdr.sh_type {
            elf::abi::SHT_RELA => {
                let relas = match file.section_data_as_relas(&shdr) {
                    Ok(it) => it,
                    Err(_) => continue,
                };
                for r in relas {
                    out.push(RelocationRecord {
                        offset: r.r_offset,
                        symbol_index: non_zero_sym(r.r_sym),
                        addend: r.r_addend,
                        kind: classify_aarch64_reloc(r.r_type),
                    });
                }
            }
            elf::abi::SHT_REL => {
                let rels = match file.section_data_as_rels(&shdr) {
                    Ok(it) => it,
                    Err(_) => continue,
                };
                for r in rels {
                    let implicit_addend = resolve_implicit_addend(bytes, segments, r.r_offset);
                    out.push(RelocationRecord {
                        offset: r.r_offset,
                        symbol_index: non_zero_sym(r.r_sym),
                        addend: implicit_addend,
                        kind: classify_aarch64_reloc(r.r_type),
                    });
                }
            }
            // Android packed：SHT_ANDROID_RELA=0x60000002, SHT_ANDROID_REL=0x60000003。
            // `elf` crate abi 没有常量，用裸值识别，然后走 packed 解码器。
            0x6000_0002 | 0x6000_0003 => {
                let is_rela = shdr.sh_type == 0x6000_0002;
                let (buf, _) = match file.section_data(&shdr) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                match crate::packed::decode_packed(buf, is_rela) {
                    Ok(recs) => out.extend(recs),
                    Err(e) => notes.push(ParseNote {
                        message: format!(
                            "android packed relocation section decode failed: {e}"
                        ),
                    }),
                }
            }
            _ => {}
        }
    }
    out
}

/// 对于 REL 类型的 relocation，隐式 addend 存储在目标 vaddr 处的文件镜像中。
/// 这里通过 segment 表把 vaddr → file_offset → 8 字节读取并解析为 i64（小端）。
fn resolve_implicit_addend(
    bytes: &[u8],
    segments: &[crate::model::LoadSegment],
    vaddr: u64,
) -> i64 {
    for seg in segments {
        if seg.filesz == 0 || vaddr < seg.vaddr || vaddr + 8 > seg.vaddr + seg.filesz {
            continue;
        }
        let file_off = seg.file_offset + (vaddr - seg.vaddr);
        if let Some(chunk) = bytes.get(file_off as usize..file_off as usize + 8) {
            let arr: [u8; 8] = chunk.try_into().unwrap_or([0; 8]);
            return i64::from_le_bytes(arr);
        }
    }
    0
}

/// ELF symbol index 0 是 reserved UNDEF，对相对重定位 `R_AARCH64_RELATIVE`
/// 来说没有有效 symbol，统一存成 `None`，让 linker 不去查符号表。
fn non_zero_sym(sym: u32) -> Option<u32> {
    if sym == 0 {
        None
    } else {
        Some(sym)
    }
}

/// 把 AArch64 relocation type 数字映射到我们的 [`RelocationKind`]。
fn classify_aarch64_reloc(r_type: u32) -> RelocationKind {
    // 数值来自 AArch64 ELF spec；保留 `Other(raw)` 让未知类型安全穿过。
    match r_type {
        1027 => RelocationKind::Relative,   // R_AARCH64_RELATIVE
        1025 => RelocationKind::GlobDat,    // R_AARCH64_GLOB_DAT
        1026 => RelocationKind::JumpSlot,   // R_AARCH64_JUMP_SLOT
        257 => RelocationKind::Abs64,       // R_AARCH64_ABS64
        _ => RelocationKind::Other(r_type),
    }
}

fn build_init_metadata(d: &DynamicInfo) -> InitMetadata {
    let init_array_count = if d.init_array_size > 0 && d.init_array.is_some() {
        // init_array 每项是一个指针：ELF64 下 8 字节。
        d.init_array_size / 8
    } else {
        0
    };
    let fini_array_count = if d.fini_array_size > 0 && d.fini_array.is_some() {
        d.fini_array_size / 8
    } else {
        0
    };
    InitMetadata {
        has_init: d.init.is_some(),
        has_init_array: init_array_count > 0,
        init_array_count,
        has_fini: d.fini.is_some(),
        has_fini_array: fini_array_count > 0,
        fini_array_count,
    }
}
