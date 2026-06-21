//! 符号解析器。
//!
//! 解析顺序（design.md "Key Decisions 3" 要求按 source 区分）：
//! 1. 本模块自身导出表（self）
//! 2. 图中其他模块的导出表（dep）
//! 3. （未来）host bridge / driver bridge
//!
//! bootstrap 暂时只覆盖 1 + 2；host/driver bridge 留作未命中返回 None，
//! 让 linker 决定是否当作 unresolved。

use crate::model::{ModuleGraph, ResolvedSymbol, SymbolQuery};
use rundroid_core::ModuleId;

/// 符号命中来源。telemetry 据此区分"resolved by dep / by host bridge / ..."。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolSource {
    /// 本模块导出。
    SelfModule,
    /// 依赖图中的其它模块导出。
    Dependency,
    /// runtime 注入的 host bridge 符号（bootstrap 阶段未实现）。
    HostBridge,
    /// runtime 注入的 driver bridge 符号（bootstrap 阶段未实现）。
    DriverBridge,
}

/// 在图中查找符号。
///
/// 查找顺序（与 Android linker 的 ELF symbol resolution 一致）：
/// 1. 本模块导出表（self）
/// 2. 按 `deps` 顺序遍历依赖闭包（breadth-first），首个命中胜出
///
/// 这与"扫描整个 modules HashMap"的关键差别（review finding 2）：
/// 扫描整张表会让"模块 A 依赖 B、模块 C 也导出同名符号"时解析到 C，
/// 而 Android 语义下 A 不依赖 C 就不该看到 C 的符号。
/// 这里改成 BFS 依赖闭包，能正确隔离非依赖模块。
pub fn resolve(graph: &ModuleGraph, query: SymbolQuery<'_>) -> Option<ResolvedSymbol> {
    // 1. self
    if let Some(m) = graph.get(query.requester) {
        if let Some(e) = m.exports.find(query.name) {
            return Some(ResolvedSymbol {
                guest_addr: e.guest_addr,
                size: e.size,
                source: SymbolSource::SelfModule,
            });
        }
    }

    // 2. 沿依赖图 BFS：requester 的直接依赖优先，再它们的依赖，依次类推。
    //    这样符号可见性严格按"装载语义"决定，与 Android linker 一致。
    let mut visited = std::collections::HashSet::new();
    visited.insert(query.requester);
    let mut queue: std::collections::VecDeque<ModuleId> = std::collections::VecDeque::new();
    if let Some(direct) = graph.deps.get(&query.requester) {
        for d in direct {
            queue.push_back(*d);
        }
    }
    while let Some(id) = queue.pop_front() {
        if !visited.insert(id) {
            continue;
        }
        if let Some(m) = graph.get(id) {
            if let Some(e) = m.exports.find(query.name) {
                return Some(ResolvedSymbol {
                    guest_addr: e.guest_addr,
                    size: e.size,
                    source: SymbolSource::Dependency,
                });
            }
        }
        if let Some(transitive) = graph.deps.get(&id) {
            for d in transitive {
                queue.push_back(*d);
            }
        }
    }

    None
}

/// 仅用于让 `ModuleId` 在 docstring 里被引用；不需要实际函数。
#[allow(dead_code)]
fn _doc_anchor(_id: ModuleId) {}
