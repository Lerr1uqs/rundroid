## Purpose

定义 `rundroid` 在 ELF 依赖装载、soname 解析和符号查找顺序上的长期要求，确保链接行为基于依赖图而不是基于偶然的模块扫描顺序。
## Requirements
### Requirement: DT_NEEDED-driven loading

runtime SHALL 以 `DT_NEEDED` 作为最小依赖装载输入。

#### Scenario: Root module loads direct dependencies

- **WHEN** runtime 装载一个 root ELF module
- **THEN** 它 SHALL 读取该模块的 `DT_NEEDED`
- **AND** 装载至少 direct dependency 集合
- **AND** 在模块图中建立显式依赖关系

### Requirement: Stable symbol resolution order

linker SHALL 按稳定顺序解析符号。

#### Scenario: Resolve by graph order instead of global scan

- **WHEN** linker 为某个 requester 解析符号
- **THEN** 它 SHALL 先检查 requester 自身导出
- **AND** 再按 direct dependencies 的稳定顺序检查
- **AND** 再按依赖闭包的稳定顺序继续检查
- **AND** 不 SHALL 通过扫描整个模块表决定最终命中结果

### Requirement: SONAME identity is explicit

模块身份 SHALL 使用解析得到的 soname 或明确的回退规则。

#### Scenario: Module graph uses parsed soname

- **WHEN** parser 提供 `DT_SONAME`
- **THEN** runtime SHALL 使用该值作为模块身份的一部分
- **AND** 不 SHALL 简单地把输入文件名长期当作 soname 替代品

### Requirement: Bootstrap direct dependency loading

bootstrap runtime SHALL 装载 root module 的 direct dependencies。

#### Scenario: Load DT_NEEDED modules before final link

- **WHEN** root module 的 `DT_NEEDED` 可被 resolver 找到
- **THEN** runtime SHALL 在最终 link 前装载这些 direct dependencies
- **AND** 把它们写入 `ModuleGraph`

### Requirement: Bootstrap graph-based resolution

bootstrap runtime SHALL 基于依赖图而不是模块全表扫描解析符号。

#### Scenario: Resolve symbol through requester graph

- **WHEN** linker 为某个 requester 解析导入符号
- **THEN** 它 SHALL 基于 requester 的依赖图顺序查找
- **AND** 不 SHALL 把全局模块扫描作为最终语义

### Requirement: Bootstrap soname handling

bootstrap runtime SHALL 显式处理 soname 身份。

#### Scenario: Parsed soname participates in module identity

- **WHEN** parser 能提供 `DT_SONAME`
- **THEN** runtime SHALL 使用该值参与模块身份与依赖匹配

