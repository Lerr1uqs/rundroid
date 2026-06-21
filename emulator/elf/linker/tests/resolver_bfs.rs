//! resolver 的依赖闭包 BFS 测试（review finding 2）。
//!
//! 关键断言：A 依赖 B，C 与 A 无依赖关系，A 解析符号时不应看到 C 的导出，
//! 即使 C 和 B 都导出同名符号。

use rundroid_core::IdAllocator;
use rundroid_elf_linker::{
    resolve, ModuleGraph, SymbolQuery, SymbolSource,
};
use rundroid_elf_loader::{ExportEntry, ExportTable, InitPlan, LoadedModule, PendingRelocation};

fn empty_module(id: rundroid_core::ModuleId, name: &str, exports: ExportTable) -> LoadedModule {
    LoadedModule {
        module_id: id,
        name: name.to_string(),
        load_bias: 0,
        base: 0,
        size: 0,
        entry: None,
        tls: None,
        exports,
        unresolved: Vec::new(),
        segments: vec![],
        relro: None,
        init_plan: InitPlan::default(),
    }
}

fn export(name: &str, addr: u64) -> ExportEntry {
    ExportEntry {
        name: name.to_string(),
        guest_addr: addr,
        size: 4,
    }
}

#[test]
fn requester_only_sees_own_dependency_closure() {
    let alloc = IdAllocator::new();
    let a = alloc.module(); // root
    let b = alloc.module(); // A 依赖 B
    let c = alloc.module(); // 与 A 无依赖关系

    let mut exports_b = ExportTable::default();
    exports_b.push(export("shared_sym", 0x1000));
    let mut exports_c = ExportTable::default();
    exports_c.push(export("shared_sym", 0x2000));

    let mut graph = ModuleGraph::new();
    graph.insert(empty_module(a, "a", ExportTable::default()), Some("a".into()));
    graph.insert(empty_module(b, "b", exports_b), Some("b".into()));
    graph.insert(empty_module(c, "c", exports_c), Some("c".into()));
    graph.add_dep(a, b); // 仅 A → B

    // A 查 shared_sym：应该命中 B（0x1000），不是 C（0x2000）。
    let r = resolve(
        &graph,
        SymbolQuery {
            name: "shared_sym",
            requester: a,
        },
    )
    .expect("should resolve via B");
    assert_eq!(r.guest_addr, 0x1000);
    assert_eq!(r.source, SymbolSource::Dependency);
}

#[test]
fn unrelated_module_symbol_is_invisible() {
    let alloc = IdAllocator::new();
    let a = alloc.module();
    let c = alloc.module();

    let mut exports_c = ExportTable::default();
    exports_c.push(export("only_in_c", 0x3000));

    let mut graph = ModuleGraph::new();
    graph.insert(empty_module(a, "a", ExportTable::default()), Some("a".into()));
    graph.insert(empty_module(c, "c", exports_c), Some("c".into()));
    // 不加 a → c 的依赖边

    let r = resolve(
        &graph,
        SymbolQuery {
            name: "only_in_c",
            requester: a,
        },
    );
    assert!(r.is_none(), "A 不依赖 C，C 的符号对 A 不可见");
}

#[test]
fn transitive_dependency_is_visible() {
    // A → B → C：A 应当能看到 C 的传递依赖符号。
    let alloc = IdAllocator::new();
    let a = alloc.module();
    let b = alloc.module();
    let c = alloc.module();

    let mut exports_c = ExportTable::default();
    exports_c.push(export("deep_sym", 0x4000));

    let mut graph = ModuleGraph::new();
    graph.insert(empty_module(a, "a", ExportTable::default()), Some("a".into()));
    graph.insert(empty_module(b, "b", ExportTable::default()), Some("b".into()));
    graph.insert(empty_module(c, "c", exports_c), Some("c".into()));
    graph.add_dep(a, b);
    graph.add_dep(b, c);

    let r = resolve(
        &graph,
        SymbolQuery {
            name: "deep_sym",
            requester: a,
        },
    )
    .expect("transitive dep should be visible");
    assert_eq!(r.guest_addr, 0x4000);
}

#[test]
fn cycle_does_not_infinite_loop() {
    // A → B → A： resolver 不能死循环。
    let alloc = IdAllocator::new();
    let a = alloc.module();
    let b = alloc.module();

    let mut graph = ModuleGraph::new();
    graph.insert(empty_module(a, "a", ExportTable::default()), Some("a".into()));
    graph.insert(empty_module(b, "b", ExportTable::default()), Some("b".into()));
    graph.add_dep(a, b);
    graph.add_dep(b, a);

    let r = resolve(
        &graph,
        SymbolQuery {
            name: "missing",
            requester: a,
        },
    );
    assert!(r.is_none());
}

// PendingRelocation / ExportEntry 的 unused 警告抑制（trait 关联但本测试不全用）。
#[allow(dead_code)]
fn _unused(_p: PendingRelocation) {}
