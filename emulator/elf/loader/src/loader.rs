//! 默认 loader 实现。
//!
//! 算法（对应 design.md 中"7. 调用导出符号"之前的部分）：
//! 1. 用镜像总大小向 ctx 申请一块连续空间 → 得到 base / load_bias
//! 2. 逐个 PT_LOAD：在 base+vaddr 映射、写入 file 字节、零填充 memsz-filesz
//! 3. 从 dynsym 构造 ExportTable（仅"已定义 + 对外可见"）
//! 4. 把所有 relocation 转成 PendingRelocation 交给 linker
//! 5. 从 DT_INIT / DT_INIT_ARRAY 构造 InitPlan
//!
//! 关键约束：不在 `load()` 内部递归 `DT_NEEDED`（那是 linker 的事）。

use crate::api::{ElfLoader, LoadContext, LoadRequest, MappedSegment, SegmentMapSpec};
use crate::error::ElfLoadError;
use crate::model::{
    ExportEntry, ExportTable, InitPlan, LoadedModule, PendingRelocation,
};
use crate::tls::extract_tls_template;
use rundroid_elf_parser::model::{DynSymbol, SymbolBinding, SymbolVisibility};
use rundroid_elf_parser::ParsedElf;
use rundroid_telemetry::{TelemetryEvent, TelemetryEventKind};

/// ARM64 page size。Unicorn 与 Android bionic 都按 4KiB 算。
const PAGE_SIZE: u64 = 0x1000;

/// 默认 loader。
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultLoader;

impl DefaultLoader {
    pub const fn new() -> Self {
        Self
    }
}

impl ElfLoader for DefaultLoader {
    fn load(
        &self,
        ctx: &mut dyn LoadContext,
        image: &ParsedElf,
        request: LoadRequest<'_>,
    ) -> Result<LoadedModule, ElfLoadError> {
        if image.segments.is_empty() {
            return Err(ElfLoadError::NoLoadableSegments);
        }

        let (total_size, image_align) = compute_image_footprint(image, request.image_align);
        let base = ctx
            .reserve_image_space(total_size, image_align)
            .map_err(|_| ElfLoadError::ReserveFailed {
                size: total_size,
                align: image_align,
            })?;
        // PIE/DYN 的 vaddr 从 0 开始，load_bias 即 base；
        // EXEC 类型 vaddr 不从 0 开始时，bias=0、base=vaddr_min。
        // bootstrap 只面向 .so（DYN），简化为 bias=base。
        let load_bias = base;

        ctx.emit(TelemetryEvent::new(
            "module.reserved",
            TelemetryEventKind::Elf,
        ));

        // 逐段映射 + 写入。段间不重叠由有效 ELF 保证。
        for seg in &image.segments {
            map_one_segment(ctx, seg, load_bias, request.bytes)?;
        }

        let exports = build_exports(image, load_bias);
        let unresolved = build_pending_relocations(image, request.module_id, load_bias);
        let init_plan = build_init_plan(image, load_bias);
        let tls = extract_tls_template(&image.segments, &image.dynamic);

        let module_end = image
            .segments
            .iter()
            .map(|s| s.vaddr + s.memsz)
            .max()
            .unwrap_or(0);
        let module_size = module_end; // vaddr 从 0 开始，size 即 end

        ctx.emit(TelemetryEvent::new(
            "module.loaded",
            TelemetryEventKind::Elf,
        ));

        // 段精确权限信息 + RELRO 范围：供 runtime 装载后做 mem_protect 收紧。
        let segments: Vec<crate::model::MappedSegmentInfo> = image
            .segments
            .iter()
            .map(|s| crate::model::MappedSegmentInfo {
                guest_addr: load_bias + s.vaddr,
                size: s.memsz,
                perms: s.perms,
            })
            .collect();
        let relro = if image.dynamic.relro {
            // PT_GNU_RELRO 的范围需要从 program headers 拿；
            // 当前 parser 没有暴露 PT_GNU_RELRO 的 vaddr/memsz，只给了 bool。
            // 这里 fallback：把"包含 .got / .dynamic 的可写数据段"近似当成 RELRO 区。
            // parser 后续补全 PT_GNU_RELRO 字段后再换为精确范围。
            image
                .segments
                .iter()
                .find(|s| s.perms.read && s.perms.write && !s.perms.execute)
                .map(|s| crate::model::RelroRange {
                    start: load_bias + s.vaddr,
                    end: load_bias + s.vaddr + s.memsz,
                })
        } else {
            None
        };

        Ok(LoadedModule {
            module_id: request.module_id,
            name: image.module_name.clone(),
            load_bias,
            base,
            size: module_size,
            entry: (image.file.entry != 0).then_some(load_bias + image.file.entry),
            tls,
            exports,
            unresolved,
            init_plan,
            segments,
            relro,
        })
    }
}

/// 镜像覆盖的总大小 = max(vaddr + memsz)，向上对齐到 page。
/// 对齐取 `request_align` 与段 align 的较大值。
fn compute_image_footprint(image: &ParsedElf, request_align: u64) -> (u64, u64) {
    let end = image
        .segments
        .iter()
        .map(|s| s.vaddr + s.memsz)
        .max()
        .unwrap_or(0);
    let align = image
        .segments
        .iter()
        .map(|s| s.align)
        .filter(|a| *a != 0)
        .fold(request_align, u64::max);
    (align_up(end, PAGE_SIZE), align)
}

fn align_up(value: u64, align: u64) -> u64 {
    if align == 0 {
        return value;
    }
    let mask = align - 1;
    (value + mask) & !mask
}

/// 映射一个 PT_LOAD 段，并写入 file 内容 + 零填充。
///
/// 映射粒度：以 page 为单位从段起始 vaddr 向下取整、向 end 向上取整，
/// 这样相邻段可能落在同一个已映射 page 上，但 Unicorn 对重叠 mem_map 会报错，
/// 因此这里改为"按段精确 [vaddr, vaddr+memsz) 映射"，
/// bootstrap 的 .so 段通常已按 page 分离，不会出现部分重叠。
fn map_one_segment(
    ctx: &mut dyn LoadContext,
    seg: &rundroid_elf_parser::model::LoadSegment,
    load_bias: u64,
    bytes: &[u8],
) -> Result<MappedSegment, ElfLoadError> {
    let guest_addr = load_bias + seg.vaddr;
    let MappedSegment { .. } = ctx
        .map_segment(SegmentMapSpec {
            guest_addr,
            size: seg.memsz,
            perms: seg.perms,
            label: "PT_LOAD",
        })
        .map_err(|source| ElfLoadError::SegmentMap {
            addr: guest_addr,
            size: seg.memsz,
            source,
        })?;

    // 写入 file 内容。filesz 可能为 0（纯 .bss 段），此时跳过写入。
    if seg.filesz > 0 {
        let file_start = seg.file_offset as usize;
        let file_end = file_start.checked_add(seg.filesz as usize).ok_or_else(|| {
            ElfLoadError::SegmentDataTruncated { vaddr: seg.vaddr }
        })?;
        let slice = bytes
            .get(file_start..file_end)
            .ok_or_else(|| ElfLoadError::SegmentDataTruncated { vaddr: seg.vaddr })?;
        ctx.write_bytes(guest_addr, slice).map_err(|source| {
            ElfLoadError::SegmentWrite {
                addr: guest_addr,
                source,
            }
        })?;
    }

    // memsz > filesz 的部分是 .bss，零填充。
    if seg.memsz > seg.filesz {
        let bss_addr = guest_addr + seg.filesz;
        let bss_len = seg.memsz - seg.filesz;
        ctx.zero_fill(bss_addr, bss_len)
            .map_err(|source| ElfLoadError::SegmentWrite {
                addr: bss_addr,
                source,
            })?;
    }

    Ok(MappedSegment {
        guest_addr,
        size: seg.memsz,
    })
}

/// 从 dynsym 构造导出表。
///
/// 只纳入同时满足以下条件的符号：
/// - 已定义（shndx != SHN_UNDEF，即 != 0）
/// - 对外可见（Default / Protected；Hidden 算导出但 linker 按需过滤）
/// - binding 为 Global 或 Weak（Local 不导出）
///
/// `value` 是相对 vaddr，导出地址 = load_bias + value。
fn build_exports(image: &ParsedElf, load_bias: u64) -> ExportTable {
    let mut table = ExportTable::default();
    for sym in &image.symbols {
        if !is_exported(sym) {
            continue;
        }
        table.push(ExportEntry {
            name: sym.name.clone(),
            guest_addr: load_bias + sym.value,
            size: sym.size,
        });
    }
    table
}

fn is_exported(sym: &DynSymbol) -> bool {
    if sym.shndx == 0 {
        return false; // SHN_UNDEF：导入符号
    }
    matches!(
        sym.binding,
        SymbolBinding::Global | SymbolBinding::Weak
    ) && matches!(
        sym.visibility,
        SymbolVisibility::Default | SymbolVisibility::Protected | SymbolVisibility::Hidden
    )
}

/// 把 parser 归一化的 relocation 转成 PendingRelocation。
///
/// 这里不解析符号，只把"目标地址"从相对 vaddr 转成 guest 绝对地址，
/// 同时把 dynsym index 翻译成符号名一并带上，避免 linker 反查。
fn build_pending_relocations(
    image: &ParsedElf,
    module_id: rundroid_core::ModuleId,
    load_bias: u64,
) -> Vec<PendingRelocation> {
    image
        .relocations
        .iter()
        .map(|r| {
            let symbol_name = r
                .symbol_index
                .and_then(|idx| image.symbols.get(idx as usize))
                .map(|s| s.name.clone());
            PendingRelocation {
                module_id,
                guest_addr: load_bias + r.offset,
                kind: r.kind,
                symbol_index: r.symbol_index,
                symbol_name,
                addend: r.addend,
            }
        })
        .collect()
}

/// 从 DT_INIT / DT_INIT_ARRAY 构造 InitPlan。
///
/// 这些地址在 parser 里是相对模块基址的 vaddr，这里加 load_bias。
fn build_init_plan(image: &ParsedElf, load_bias: u64) -> InitPlan {
    let legacy_init = image.dynamic.init.map(|v| load_bias + v);
    let init_array = match (image.dynamic.init_array, image.dynamic.init_array_size) {
        (Some(base), size) if size > 0 => {
            let count = (size / 8) as usize;
            // 这里只产出"指针所在的 guest 地址"列表，不读取指针内容
            // —— 指针解引用在 linker 完成 relocation 写回之后进行。
            // 但 Android 语义要求 init_array 的 entries 是函数指针，
            // loader 阶段镜像刚写完，已经可以读取。
            // 为保持 loader 的"不读 guest"边界，这里给出指针槽位地址，
            // 由 linker / runtime 在 init 调度阶段读取。
            // 上面注释保留以解释为什么是 base + i*8 而不是解引用结果。
            (0..count)
                .map(|i| base + (i as u64) * 8 + load_bias)
                .collect()
        }
        _ => Vec::new(),
    };
    InitPlan {
        legacy_init,
        init_array,
    }
}
