//! init 调度。
//!
//! 输入：模块依赖图。输出：稳定的 init_order。
//!
//! Android linker 的语义：被依赖的模块先 init（dlopen 的逆序 fini）。
//! 拓扑排序采用 Kahn 算法；遇到环则报 [`ElfLinkError::DependencyCycle`]。

use crate::error::ElfLinkError;
use crate::model::ModuleGraph;
use rundroid_core::ModuleId;
use std::collections::{HashSet, VecDeque};

/// 对模块图做拓扑排序，返回 init 调用顺序。
///
/// 约定：`deps[A] = [B, C]` 表示 A 依赖 B、C，
/// 那么 init 顺序里 B、C 必须在 A 之前。
pub fn schedule(graph: &ModuleGraph, root: ModuleId) -> Result<Vec<ModuleId>, ElfLinkError> {
    // 收集所有"参与本次链接"的模块（从 root 出发的传递闭包）。
    let mut involved: HashSet<ModuleId> = HashSet::new();
    collect_reachable(graph, root, &mut involved);

    // Kahn：indeg[X] = X 自己依赖了多少个 involved 模块（=X 的依赖数）。
    // 这样入度为 0 的节点是叶子（没人依赖、且自己不依赖别人），
    // 而 init 顺序要求"被依赖的在前"，所以从叶子开始消费。
    let mut indeg: HashMap<ModuleId, usize> = HashMap::new();
    for id in &involved {
        let count = graph
            .deps
            .get(id)
            .map(|deps| deps.iter().filter(|d| involved.contains(d)).count())
            .unwrap_or(0);
        indeg.insert(*id, count);
    }

    // 入度为 0 的节点（不被任何人依赖）入队作为"末端"，
    // 但 init 顺序要求"被依赖的在前"，所以这里反向输出。
    let mut queue: VecDeque<ModuleId> = involved
        .iter()
        .copied()
        .filter(|id| indeg.get(id).copied().unwrap_or(0) == 0)
        .collect();
    // 保证输出稳定：按 raw 排序。
    let mut queue: Vec<ModuleId> = queue.drain(..).collect();
    queue.sort_by_key(|id| id.raw());

    let mut order: Vec<ModuleId> = Vec::with_capacity(involved.len());
    let mut visited: HashSet<ModuleId> = HashSet::new();

    while let Some(node) = pop_min(&mut queue) {
        if !visited.insert(node) {
            continue;
        }
        order.push(node);
        // node 已消费：所有"把 node 列为依赖"的模块的 indeg 减 1。
        for id in &involved {
            if let Some(deps) = graph.deps.get(id) {
                if deps.contains(&node) {
                    if let Some(v) = indeg.get_mut(id) {
                        *v = v.saturating_sub(1);
                        if *v == 0 && !visited.contains(id) {
                            push_sorted(&mut queue, *id);
                        }
                    }
                }
            }
        }
    }

    if order.len() != involved.len() {
        // 剩下的节点必然成环。
        let stuck = involved
            .iter()
            .copied()
            .find(|id| !visited.contains(id))
            .ok_or(ElfLinkError::DependencyCycle(root))?;
        return Err(ElfLinkError::DependencyCycle(stuck));
    }

    // Kahn 从叶子出发，因此 order 已经是"被依赖的在前"，直接返回。
    Ok(order)
}

fn collect_reachable(graph: &ModuleGraph, root: ModuleId, out: &mut HashSet<ModuleId>) {
    if !out.insert(root) {
        return;
    }
    if let Some(deps) = graph.deps.get(&root) {
        for d in deps {
            collect_reachable(graph, *d, out);
        }
    }
}

// 用 std::collections::HashMap 避免再 import；本模块内简写。
use std::collections::HashMap;

fn pop_min(v: &mut Vec<ModuleId>) -> Option<ModuleId> {
    // 保持 vec 升序，每次取第一个。
    v.sort_by_key(|id| id.raw());
    if v.is_empty() {
        None
    } else {
        Some(v.remove(0))
    }
}

fn push_sorted(v: &mut Vec<ModuleId>, id: ModuleId) {
    let pos = v.partition_point(|x| x.raw() <= id.raw());
    v.insert(pos, id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rundroid_core::IdAllocator;

    fn mk_id(alloc: &IdAllocator) -> ModuleId {
        alloc.module()
    }

    #[test]
    fn linear_chain() {
        let alloc = IdAllocator::new();
        let a = mk_id(&alloc);
        let b = mk_id(&alloc);
        let c = mk_id(&alloc);
        let mut g = ModuleGraph::new();
        // 空 module 占位。
        for id in [a, b, c] {
            g.insert(
                rundroid_elf_loader::LoadedModule {
                    module_id: id,
                    name: format!("m{}", id.raw()),
                    load_bias: 0,
                    base: 0,
                    size: 0,
                    entry: None,
                    tls: None,
                    exports: Default::default(),
                    unresolved: Vec::new(),
            segments: vec![],
            relro: None,
                    init_plan: Default::default(),
                },
                Some(format!("m{}", id.raw())),
            );
        }
        // a → b → c
        g.add_dep(a, b);
        g.add_dep(b, c);

        let order = schedule(&g, a).unwrap();
        // c 必须在 b 之前，b 必须在 a 之前。
        let pos = |id: ModuleId| order.iter().position(|x| *x == id).unwrap();
        assert!(pos(c) < pos(b));
        assert!(pos(b) < pos(a));
    }
}
