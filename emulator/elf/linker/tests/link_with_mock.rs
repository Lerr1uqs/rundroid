//! linker 端到端测试。
//!
//! 不依赖真实 backend：用内存 HashMap 模拟"guest 地址 → 字节"，
//! 验证 relocation 写回的语义（RELATIVE 写 base+addend、符号引用写符号地址）。

use std::collections::HashMap;
use rundroid_core::{IdAllocator, ModuleId};
use rundroid_elf_linker::{
    DefaultLinker, ElfLinkError, LinkContext, ModuleGraph, ResolvedSymbol, SymbolQuery,
    SymbolSource,
};
use rundroid_elf_loader::{ExportEntry, ExportTable, InitPlan, LoadedModule, PendingRelocation};
use rundroid_memory::MemoryError;
use rundroid_telemetry::TelemetryEvent;

/// 内存模拟的 guest 地址空间：只覆盖"linker 写回过的 8 字节"。
#[derive(Default)]
struct MockLinkCtx {
    mem: HashMap<u64, u64>,
}

impl LinkContext for MockLinkCtx {
    fn resolve(&self, query: SymbolQuery<'_>) -> Result<Option<ResolvedSymbol>, ElfLinkError> {
        // 测试用：按名字硬编码两个符号，模拟"已被 loader 装载好的导出表"。
        match query.name {
            "target_fn" => Ok(Some(ResolvedSymbol {
                guest_addr: 0xCAFE_0000,
                size: 8,
                source: SymbolSource::Dependency,
            })),
            "weak_sym" => Ok(None), // 模拟未命中
            _ => Ok(None),
        }
    }
    fn write_relocation(
        &mut self,
        patch: rundroid_elf_linker::reloc_aarch64::RelocationPatch,
    ) -> Result<(), MemoryError> {
        self.mem.insert(patch.target_addr, patch.value);
        Ok(())
    }
    fn protect_relro(&mut self, _module: ModuleId) -> Result<(), MemoryError> {
        Ok(())
    }
    fn emit(&mut self, _event: TelemetryEvent<'_>) {}
}

fn make_module(
    id: ModuleId,
    load_bias: u64,
    pending: Vec<PendingRelocation>,
    exports: ExportTable,
) -> LoadedModule {
    LoadedModule {
        module_id: id,
        name: format!("m{}", id.raw()),
        load_bias,
        base: load_bias,
        size: 0x1000,
        entry: None,
        tls: None,
        exports,
        unresolved: pending,
        init_plan: InitPlan::default(),
        segments: vec![],
        relro: None,
    }
}

#[test]
fn writes_relative_relocation_with_load_bias() {
    let alloc = IdAllocator::new();
    let id = alloc.module();
    let pending = vec![PendingRelocation {
        module_id: id,
        guest_addr: 0x2000_1000,
        kind: rundroid_elf_parser::model::RelocationKind::Relative,
        symbol_index: None,
        symbol_name: None,
        addend: 0x40,
    }];
    let module = make_module(id, 0x2000_0000, pending, ExportTable::default());

    let mut graph = ModuleGraph::new();
    graph.insert(module, None);

    let mut ctx = MockLinkCtx::default();
    let report = DefaultLinker::new().link_root(&mut ctx, &mut graph, id).unwrap();

    assert_eq!(report.linked, vec![id]);
    assert!(report.unresolved.is_empty());
    // RELATIVE 写回 = load_bias + addend = 0x2000_0040。
    assert_eq!(ctx.mem.get(&0x2000_1000), Some(&0x2000_0040));
}

#[test]
fn writes_glob_dat_with_resolved_symbol() {
    let alloc = IdAllocator::new();
    let id = alloc.module();
    let pending = vec![PendingRelocation {
        module_id: id,
        guest_addr: 0x3000,
        kind: rundroid_elf_parser::model::RelocationKind::GlobDat,
        symbol_index: Some(5),
        symbol_name: Some("target_fn".to_string()),
        addend: 0,
    }];
    let module = make_module(id, 0x1000, pending, ExportTable::default());

    let mut graph = ModuleGraph::new();
    graph.insert(module, None);

    let mut ctx = MockLinkCtx::default();
    let _report = DefaultLinker::new().link_root(&mut ctx, &mut graph, id).unwrap();

    // GLOB_DAT 写回 = symbol_addr + addend = 0xCAFE_0000。
    assert_eq!(ctx.mem.get(&0x3000), Some(&0xCAFE_0000));
}

#[test]
fn unresolved_symbol_recorded_in_report() {
    let alloc = IdAllocator::new();
    let id = alloc.module();
    let pending = vec![PendingRelocation {
        module_id: id,
        guest_addr: 0x4000,
        kind: rundroid_elf_parser::model::RelocationKind::JumpSlot,
        symbol_index: Some(7),
        symbol_name: Some("weak_sym".to_string()),
        addend: 0,
    }];
    let module = make_module(id, 0x1000, pending, ExportTable::default());

    let mut graph = ModuleGraph::new();
    graph.insert(module, None);

    let mut ctx = MockLinkCtx::default();
    let report = DefaultLinker::new().link_root(&mut ctx, &mut graph, id).unwrap();

    assert_eq!(report.unresolved.len(), 1);
    assert_eq!(report.unresolved[0].name, "weak_sym");
    // 未命中的写回不应该发生。
    assert!(ctx.mem.is_empty());
}

#[test]
fn self_export_resolves_when_ctx_uses_graph_resolver() {
    // resolver::resolve 自身能命中 self 导出，这里只验证 linker 调用它的契约面：
    // 用一个"按 name 在硬编码表里查"的 ctx，模拟 self 命中。
    let alloc = IdAllocator::new();
    let id = alloc.module();
    let pending = vec![PendingRelocation {
        module_id: id,
        guest_addr: 0x6000,
        kind: rundroid_elf_parser::model::RelocationKind::Abs64,
        symbol_index: Some(1),
        symbol_name: Some("my_fn".to_string()),
        addend: 0x10,
    }];
    let module = make_module(id, 0x1000, pending, ExportTable::default());

    let mut graph = ModuleGraph::new();
    graph.insert(module, None);

    struct SelfCtx;
    impl LinkContext for SelfCtx {
        fn resolve(&self, _q: SymbolQuery<'_>) -> Result<Option<ResolvedSymbol>, ElfLinkError> {
            Ok(Some(ResolvedSymbol {
                guest_addr: 0x5000,
                size: 4,
                source: SymbolSource::SelfModule,
            }))
        }
        fn write_relocation(
            &mut self,
            _p: rundroid_elf_linker::RelocationPatch,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        fn protect_relro(&mut self, _: ModuleId) -> Result<(), MemoryError> {
            Ok(())
        }
        fn emit(&mut self, _: TelemetryEvent<'_>) {}
    }

    let mut ctx = SelfCtx;
    let report = DefaultLinker::new().link_root(&mut ctx, &mut graph, id).unwrap();
    assert!(report.unresolved.is_empty());
}
