//! 默认 linker 实现。
//!
//! 链接主流程（对应 design.md trait）：
//! 1. 调用 [`init::schedule`] 得到 init_order（拓扑序）
//! 2. 遍历 root 模块的全部 PendingRelocation
//! 3. 对每条 relocation：查符号 → 计算写回值 → 通过 LinkContext 写回
//! 4. 汇总 LinkReport（linked / unresolved / init_order）
//!
//! linker 不重新解析 ELF 原始字节，只消费 loader 产出的 PendingRelocation。

use crate::error::ElfLinkError;
use crate::init;
use crate::model::{LinkReport, ModuleGraph, ResolvedSymbol, SymbolQuery};
use crate::reloc_aarch64;
use rundroid_core::ModuleId;
use rundroid_elf_loader::LoadedModule;
use rundroid_memory::MemoryError;
use rundroid_telemetry::{TelemetryEvent, TelemetryEventKind};

/// LinkContext trait。与 LoadContext 类似的"副作用边界"，
/// 让 linker 不直接持有 backend / RegionTracker。
#[allow(unused_variables)]
pub trait LinkContext {
    /// 解析符号。
    fn resolve(&self, query: SymbolQuery<'_>) -> Result<Option<ResolvedSymbol>, ElfLinkError>;

    /// 写回一个 8 字节 relocation 值。
    /// 实现内部会把 value 拆成 8 字节小端序，调用 backend `mem_write`。
    fn write_relocation(&mut self, patch: reloc_aarch64::RelocationPatch) -> Result<(), MemoryError>;

    /// RELRO 区域权限收紧（写完 relocation 后转只读）。
    fn protect_relro(&mut self, module: ModuleId) -> Result<(), MemoryError>;

    fn emit(&mut self, event: TelemetryEvent<'_>);
}

/// 默认 linker。
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultLinker;

impl DefaultLinker {
    pub const fn new() -> Self {
        Self
    }
}

impl DefaultLinker {
    /// 链接一个模块图，从 `root` 开始。
    ///
    /// 注意：bootstrap 阶段 `link_root` 只在"已经装载好的图"上工作，
    /// 它不会触发对 DT_NEEDED 的额外装载——装载由 runtime 在调用 linker 前完成。
    pub fn link_root(
        &self,
        ctx: &mut dyn LinkContext,
        graph: &mut ModuleGraph,
        root: ModuleId,
    ) -> Result<LinkReport, ElfLinkError> {
        let init_order = init::schedule(graph, root)?;
        let mut report = LinkReport {
            linked: Vec::new(),
            unresolved: Vec::new(),
            init_order,
        };

        // 遍历 involved 集合，逐模块写回 relocation。
        // 顺序按 init_order：被依赖模块先写回，便于调试时观察因果。
        let order_for_iter = report.init_order.clone();
        for module_id in &order_for_iter {
            let module = match graph.modules.get(module_id) {
                Some(m) => m.clone(),
                None => continue,
            };
            self.link_one_module(ctx, &module, &mut report)?;
            report.linked.push(*module_id);
            // bootstrap 阶段保护 RELRO；写回失败不阻塞 report 返回。
            let _ = ctx.protect_relro(*module_id);
        }

        ctx.emit(TelemetryEvent::new(
            "module.linked",
            TelemetryEventKind::Elf,
        ));
        Ok(report)
    }

    fn link_one_module(
        &self,
        ctx: &mut dyn LinkContext,
        module: &LoadedModule,
        report: &mut LinkReport,
    ) -> Result<(), ElfLinkError> {
        let owner_load_bias = module.load_bias;
        let owner_id = module.module_id;
        // 拿到 pending 的 snapshot，避免后面借用冲突。
        let pending: Vec<_> = module.unresolved.clone();

        for p in &pending {
            // RELATIVE：不查符号；其余类型按 symbol_name 查。
            let resolved_addr = if let Some(name) = p.symbol_name.as_deref() {
                match ctx.resolve(SymbolQuery { name, requester: owner_id })? {
                    Some(sym) => Some(sym.guest_addr),
                    None => {
                        // 未命中：记录到 unresolved，跳过这条写回。
                        report.unresolved.push(crate::model::UnresolvedSymbol {
                            name: name.to_string(),
                            requester: owner_id,
                        });
                        continue;
                    }
                }
            } else {
                None
            };

            let patch = reloc_aarch64::compute_patch(p, owner_load_bias, resolved_addr)?;
            ctx.write_relocation(patch)
                .map_err(|source| ElfLinkError::RelocationWrite {
                    addr: p.guest_addr,
                    source,
                })?;
        }

        Ok(())
    }
}
