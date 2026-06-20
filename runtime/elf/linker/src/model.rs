//! linker 数据模型：模块图 / 符号查询 / 链接报告。

use rundroid_core::ModuleId;
use rundroid_elf_loader::LoadedModule;
use std::collections::HashMap;

/// 模块图：保存所有已装载模块及其依赖关系。
///
/// bootstrap 阶段用 `HashMap` 索引，规模不大；
/// 后续如果引入大模块图再换更紧凑的 arena 结构。
#[derive(Default)]
pub struct ModuleGraph {
    pub modules: HashMap<ModuleId, LoadedModule>,
    /// 依赖邻接表：module → 它声明的依赖对应的 ModuleId 列表。
    ///
    /// 依赖的"按 soname → ModuleId"解析在 linker 装入模块时完成，
    /// 因此这里直接存 ModuleId 而不是字符串，避免重复解析。
    pub deps: HashMap<ModuleId, Vec<ModuleId>>,
    /// soname → ModuleId 反查表，用于按名新增依赖。
    pub by_soname: HashMap<String, ModuleId>,
}

impl ModuleGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, module: LoadedModule, soname: Option<String>) {
        if let Some(name) = soname {
            self.by_soname.insert(name, module.module_id);
        }
        self.modules.insert(module.module_id, module);
    }

    pub fn get(&self, id: ModuleId) -> Option<&LoadedModule> {
        self.modules.get(&id)
    }

    /// 声明 `from` 依赖 `on`（按 ModuleId）。
    pub fn add_dep(&mut self, from: ModuleId, on: ModuleId) {
        self.deps.entry(from).or_default().push(on);
    }
}

/// 符号查询请求。
#[derive(Debug, Clone, Copy)]
pub struct SymbolQuery<'a> {
    pub name: &'a str,
    /// 触发查询的模块，用于 resolve 顺序与 telemetry 归因。
    pub requester: ModuleId,
}

/// 符号解析结果。
#[derive(Debug, Clone, Copy)]
pub struct ResolvedSymbol {
    pub guest_addr: u64,
    pub size: u64,
    pub source: super::resolver::SymbolSource,
}

/// 未解析符号记录，进入 [`super::LinkReport::unresolved`]。
#[derive(Debug, Clone)]
pub struct UnresolvedSymbol {
    pub name: String,
    pub requester: ModuleId,
}

/// 链接报告。
#[derive(Debug, Default, Clone)]
pub struct LinkReport {
    /// 成功完成 relocation 写回的模块。
    pub linked: Vec<ModuleId>,
    /// 未解析符号列表（weak 允许进这里，strong 默认 fail-hard）。
    pub unresolved: Vec<UnresolvedSymbol>,
    /// init 调用顺序（拓扑），runtime 据此依次调用 init。
    pub init_order: Vec<ModuleId>,
}
