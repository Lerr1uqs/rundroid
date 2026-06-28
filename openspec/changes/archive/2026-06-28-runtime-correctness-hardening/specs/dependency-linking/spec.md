## ADDED Requirements

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
