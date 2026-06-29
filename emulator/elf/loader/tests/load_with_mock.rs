//! loader 布局逻辑测试。
//!
//! 用一个 in-memory mock LoadContext 验证 loader 的"算地址 + 段映射 + 写入 + 导出表 + init_plan"
//! 是否正确，完全不依赖 Unicorn。这是 loader / linker 三层解耦带来的可测试性收益。

use rundroid_core::{Arch, BackendKind, IdAllocator, RuntimeConfig};
use rundroid_elf_loader::{
    DefaultLoader, ElfLoader, LoadContext, LoadRequest, MappedSegment, SegmentMapSpec,
};
use rundroid_elf_parser::{
    DynamicInfo, ElfIdentity, ElfParseError, ElfParser, InitMetadata, LoadSegment, ParseInput,
    SegmentPerms,
};
use rundroid_memory::{MemoryError, MemoryPerms, MemoryUsage};
use rundroid_telemetry::TelemetryEvent;

/// 最小 mock：把 loader 对 backend 的所有调用记录成"事件流"，
/// 测试据此断言装载顺序与地址计算。
#[derive(Default)]
struct MockCtx {
    reserves: Vec<(u64, u64)>,
    maps: Vec<SegmentMapSpec<'static>>,
    protects: Vec<(u64, u64, MemoryPerms, MemoryUsage)>,
    writes: Vec<(u64, Vec<u8>)>,
    zero_fills: Vec<(u64, u64)>,
    emits: usize,
    next_base: u64,
}

impl MockCtx {
    fn new(start_base: u64) -> Self {
        Self {
            next_base: start_base,
            ..Default::default()
        }
    }
}

impl LoadContext for MockCtx {
    fn reserve_image_space(&mut self, size: u64, align: u64) -> Result<u64, MemoryError> {
        self.reserves.push((size, align));
        // 模拟"按 align 对齐返回一个基址"。
        let base = align_up(self.next_base, align);
        self.next_base = base + size;
        Ok(base)
    }
    fn map_segment(&mut self, spec: SegmentMapSpec<'_>) -> Result<MappedSegment, MemoryError> {
        // 把借用 spec.label 提升成 'static 以便存进 Vec；测试专用，生产代码不会这样。
        let static_spec = SegmentMapSpec {
            guest_addr: spec.guest_addr,
            size: spec.size,
            perms: spec.perms,
            label: unsafe { std::mem::transmute::<&str, &'static str>(spec.label) },
        };
        self.maps.push(static_spec);
        Ok(MappedSegment {
            guest_addr: spec.guest_addr,
            size: spec.size,
        })
    }
    fn protect_segment(
        &mut self,
        guest_addr: u64,
        size: u64,
        perms: MemoryPerms,
        usage: MemoryUsage,
    ) -> Result<(), MemoryError> {
        self.protects.push((guest_addr, size, perms, usage));
        Ok(())
    }
    fn write_bytes(&mut self, guest_addr: u64, bytes: &[u8]) -> Result<(), MemoryError> {
        self.writes.push((guest_addr, bytes.to_vec()));
        Ok(())
    }
    fn zero_fill(&mut self, guest_addr: u64, len: u64) -> Result<(), MemoryError> {
        self.zero_fills.push((guest_addr, len));
        Ok(())
    }
    fn emit(&mut self, _event: TelemetryEvent<'_>) {
        self.emits += 1;
    }
}

fn align_up(v: u64, a: u64) -> u64 {
    if a == 0 {
        v
    } else {
        (v + a - 1) & !(a - 1)
    }
}

/// 构造一个最小合成 ParsedElf：两段 PT_LOAD，少量符号 + relocation。
/// 避免依赖外部 .so 文件，让 loader 布局测试完全自包含。
fn synthetic_image() -> rundroid_elf_parser::ParsedElf {
    use rundroid_elf_parser::{DynSymbol, RelocationKind, RelocationRecord, SymbolBinding, SymbolVisibility};

    let segments = vec![
        LoadSegment {
            file_offset: 0x1000,
            vaddr: 0x0,
            filesz: 0x100,
            memsz: 0x100,
            perms: SegmentPerms { read: true, write: false, execute: true },
            align: 0x1000,
        },
        LoadSegment {
            file_offset: 0x2000,
            vaddr: 0x2000,
            filesz: 0x80,
            memsz: 0x200, // memsz > filesz → .bss
            perms: SegmentPerms { read: true, write: true, execute: false },
            align: 0x1000,
        },
    ];

    let symbols = vec![
        DynSymbol {
            name: "".to_string(),
            value: 0,
            size: 0,
            visibility: SymbolVisibility::Default,
            binding: SymbolBinding::Local,
            shndx: 0,
        },
        DynSymbol {
            name: "Java_pkg_foo".to_string(),
            value: 0x40,
            size: 0x20,
            visibility: SymbolVisibility::Default,
            binding: SymbolBinding::Global,
            shndx: 1, // 已定义
        },
    ];

    let relocations = vec![RelocationRecord {
        offset: 0x2050,
        symbol_index: None,
        addend: 0x1234,
        kind: RelocationKind::Relative,
    }];

    let dynamic = DynamicInfo {
        init: Some(0x30),
        init_array: Some(0x2060),
        init_array_size: 8, // 1 个条目
        ..Default::default()
    };

    rundroid_elf_parser::ParsedElf {
        module_name: "synthetic.so".to_string(),
        file: ElfIdentity {
            is_64bit: true,
            little_endian: true,
            machine: 183,
            e_type: 3,
            entry: 0x40,
        },
        segments,
        dynamic,
        symbols,
        relocations,
        init: InitMetadata {
            has_init: true,
            has_init_array: true,
            init_array_count: 1,
            ..Default::default()
        },
        notes: Vec::new(),
    }
}

#[test]
fn loader_maps_segments_and_builds_exports() {
    let mut ctx = MockCtx::new(0x1000_0000);
    let image = synthetic_image();
    // 提供足够长的"backing bytes"，loader 会按 file_offset 取片段。
    let mut bytes = vec![0u8; 0x4000];
    // 在 0x1000..0x1100 写入特征字节，便于断言" loader 写入了正确片段"。
    for (i, b) in bytes.iter_mut().enumerate() {
        if (0x1000..0x1100).contains(&i) {
            *b = (i & 0xff) as u8;
        }
    }

    let allocator = IdAllocator::new();
    let module_id = allocator.module();
    let req = LoadRequest {
        image_align: 0x1000,
        bytes: &bytes,
        module_id,
    };

    let module = DefaultLoader::new().load(&mut ctx, &image, req).unwrap();

    // base 应当对齐到 0x1000，从 0x1000_0000 起。
    assert_eq!(module.base, 0x1000_0000);
    assert_eq!(module.load_bias, 0x1000_0000);

    // 两段都被映射。
    assert_eq!(ctx.maps.len(), 2);
    assert_eq!(ctx.maps[0].guest_addr, 0x1000_0000); // base + vaddr(0)
    assert_eq!(ctx.maps[1].guest_addr, 0x1000_2000); // base + vaddr(0x2000)
    assert_eq!(ctx.protects.len(), 2);
    assert_eq!(ctx.protects[0].0, 0x1000_0000);
    assert_eq!(ctx.protects[0].2, MemoryPerms::READ_EXEC);
    assert_eq!(ctx.protects[0].3, MemoryUsage::ELFImage);
    assert_eq!(ctx.protects[1].0, 0x1000_2000);
    assert_eq!(ctx.protects[1].2, MemoryPerms::READ_WRITE);

    // 第二段有 .bss：filesz=0x80 写入、memsz-filesz=0x180 零填充。
    assert_eq!(ctx.writes.len(), 2);
    assert_eq!(ctx.writes[1].0, 0x1000_2000);
    assert_eq!(ctx.writes[1].1.len(), 0x80);
    assert_eq!(ctx.zero_fills.len(), 1);
    assert_eq!(ctx.zero_fills[0], (0x1000_2080, 0x180));

    // 导出表：Java_pkg_foo 在 base+0x40。
    let exp = module.exports.find("Java_pkg_foo").expect("export present");
    assert_eq!(exp.guest_addr, 0x1000_0040);

    // pending relocation：guest_addr = base + 0x2050。
    assert_eq!(module.unresolved.len(), 1);
    assert_eq!(module.unresolved[0].guest_addr, 0x1000_2050);

    // init_plan：legacy DT_INIT = base+0x30，init_array 一个槽位 base+0x2060。
    assert_eq!(module.init_plan.legacy_init, Some(0x1000_0030));
    assert_eq!(module.init_plan.init_array, vec![0x1000_2060]);

    // runtime config / allocator 不实际参与装载，这里只验证类型可用。
    let _ = RuntimeConfig {
        arch: Arch::Arm64,
        backend: BackendKind::Unicorn,
        ..RuntimeConfig::bootstrap()
    };
}

#[test]
fn loader_rejects_empty_image() {
    let mut ctx = MockCtx::new(0x1000_0000);
    let image = rundroid_elf_parser::ParsedElf {
        module_name: "empty.so".to_string(),
        file: ElfIdentity {
            is_64bit: true,
            little_endian: true,
            machine: 183,
            e_type: 3,
            entry: 0,
        },
        segments: Vec::new(),
        dynamic: DynamicInfo::default(),
        symbols: Vec::new(),
        relocations: Vec::new(),
        init: InitMetadata::default(),
        notes: Vec::new(),
    };
    let bytes = vec![0u8; 8];
    let allocator = IdAllocator::new();
    let req = LoadRequest {
        image_align: 0x1000,
        bytes: &bytes,
        module_id: allocator.module(),
    };
    let err = DefaultLoader::new().load(&mut ctx, &image, req).unwrap_err();
    assert!(matches!(err, rundroid_elf_loader::ElfLoadError::NoLoadableSegments));
}

#[test]
fn load_real_so_via_mock() {
    // 端到端：parser 解析真实 .so，loader 装载到 mock ctx。
    // 确保 parser → loader 接口对齐。
    const FIXTURE: &str =
        "F:/reverse-workspace/unidbg/unidbg-android/src/test/resources/example_binaries/arm64-v8a/libjnidispatch.so";
    let bytes = match std::fs::read(FIXTURE) {
        Ok(b) => b,
        Err(_) => {
            eprintln!("skip: fixture missing");
            return;
        }
    };
    let parsed = rundroid_elf_parser::ElfCrateParser::new()
        .parse(ParseInput::new("libjnidispatch.so", &bytes))
        .unwrap();

    let mut ctx = MockCtx::new(0x2000_0000);
    let allocator = IdAllocator::new();
    let req = LoadRequest {
        image_align: 0x1000,
        bytes: &bytes,
        module_id: allocator.module(),
    };
    let module = DefaultLoader::new().load(&mut ctx, &parsed, req).unwrap();
    assert!(module.size > 0);
    assert!(!ctx.maps.is_empty());
}

// 让 ParseInput / ElfParser / ElfParseError 这几个名字被引用，避免 unused import 警告。
fn _force_imports(_: ParseInput<'_>) -> Result<(), ElfParseError> {
    Ok(())
}
