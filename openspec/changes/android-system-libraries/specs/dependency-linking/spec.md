## MODIFIED Requirements

> 本次 delta 扩展 DT_NEEDED 驱动的解析流程：符号查找顺序从原来的"requester → dep 依赖图"扩展为"requester → dep 依赖图 → SystemLibraryResolver"三段回退；loader 新增 `allow_unresolved` + `skip_init` 标记用于系统库模块。核心契约（依赖图驱动、稳定符号解析顺序、SONAME 身份显式）语义不变。

### Requirement: DT_NEEDED-driven loading

runtime SHALL 以 `DT_NEEDED` 作为最小依赖装载输入。

#### Scenario: Root module loads direct dependencies (unchanged)

- **WHEN** runtime 装载一个 root ELF module
- **THEN** 它 SHALL 读取该模块的 `DT_NEEDED`
- **AND** 装载至少 direct dependency 集合
- **AND** 在模块图中建立显式依赖关系

### Requirement: Stable symbol resolution order

linker SHALL 按稳定顺序解析符号。

#### Scenario: Resolve by graph order with system library fallback

- **WHEN** linker 为某个 requester 解析符号
- **THEN** 它 SHALL 先检查 requester 自身导出
- **AND** 再按 direct dependencies 的稳定顺序检查
- **AND** 再按依赖闭包的稳定顺序继续检查
- **AND** 若前三段均未命中，SHALL 回退到 SystemLibraryResolver 查找（检查系统库目录中所有已加载模块的导出表）
- **AND** 不 SHALL 通过扫描整个模块表决定最终命中结果

### Requirement: SONAME identity is explicit (unchanged)

模块身份 SHALL 使用解析得到的 soname 或明确的回退规则。

#### Scenario: Module graph uses parsed soname (unchanged)

- **WHEN** parser 提供 `DT_SONAME`
- **THEN** runtime SHALL 使用该值作为模块身份的一部分
- **AND** 不 SHALL 简单地把输入文件名长期当作 soname 替代品

### Requirement: Bootstrap direct dependency loading (unchanged)

bootstrap runtime SHALL 装载 root module 的 direct dependencies。

#### Scenario: Load DT_NEEDED modules before final link (unchanged)

- **WHEN** root module 的 `DT_NEEDED` 可被 resolver 找到
- **THEN** runtime SHALL 在最终 link 前装载这些 direct dependencies
- **AND** 把它们写入 `ModuleGraph`

### Requirement: Bootstrap graph-based resolution with system fallback

bootstrap runtime SHALL 基于依赖图而不是模块全表扫描解析符号，并在依赖图未命中时回退到系统库。

#### Scenario: Resolve symbol through requester graph with system fallback

- **WHEN** linker 为某个 requester 解析导入符号
- **THEN** 它 SHALL 基于 requester 的依赖图顺序查找
- **AND** 依赖图未命中时 SHALL 回退到 SystemLibraryResolver
- **AND** 不 SHALL 把全局模块扫描作为最终语义

### Requirement: Bootstrap soname handling (unchanged)

bootstrap runtime SHALL 显式处理 soname 身份。

#### Scenario: Parsed soname participates in module identity (unchanged)

- **WHEN** parser 能提供 `DT_SONAME`
- **THEN** runtime SHALL 使用该值参与模块身份与依赖匹配

### Requirement: System library loading with relaxed requirements

当加载者通过 `SystemLibraryResolver` 路径装载模块时，被装载的系统库模块 SHALL 标记 `allow_unresolved=true`（自身导入的未解析符号不导致链接失败，relocation 落地为 0）和 `skip_init=true`（init_array 不执行）。这些标记 SHALL 与普通 guest 模块使用的默认标记不同。

#### Scenario: System library loaded with allow_unresolved

- **WHEN** loader 从系统库目录装载模块
- **THEN** 该模块 SHALL 自动获得 `allow_unresolved=true` 标记
- **AND** linker SHALL 对该模块内的 unresolved 符号采取宽容节（记录到 `unresolved_symbols` 列表，非致命）

#### Scenario: System library loaded with skip_init

- **WHEN** loader 从系统库目录装载模块
- **THEN** 该模块 SHALL 自动获得 `skip_init=true` 标记
- **AND** linker 的 init 顺序 SHALL 排除该模块
