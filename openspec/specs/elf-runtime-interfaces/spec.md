## Purpose

定义 `rundroid` 在 ELF parser、loader、linker 三层上的稳定接口边界，确保实现方在 Rust trait、数据模型和错误模型上保持一致，不把职责重新耦合回一个“大装载器”。

## Requirements

### Requirement: Parser trait boundary

`runtime/elf/parser` SHALL 暴露独立、只读的解析接口。

#### Scenario: Parse bytes into immutable ELF view

- **WHEN** 上层把一个模块字节数组交给 parser
- **THEN** parser SHALL 返回不可变的解析结果对象
- **AND** 该结果 SHALL 包含段信息、动态表、符号表、重定位、init/fini 元数据
- **AND** parser SHALL 不依赖 backend、memory mapper、syscall 或 VFS 实现

### Requirement: Normalized relocation model

parser SHALL 对外暴露统一的重定位数据模型。

#### Scenario: Normalize Android and standard relocation sources

- **WHEN** 输入 ELF 同时包含标准 `REL`/`RELA` 或 Android packed relocation
- **THEN** parser SHALL 把它们归一化为统一的 relocation 记录流
- **AND** linker SHALL 不需要再次解析 packed relocation 原始编码

### Requirement: Loader trait boundary

`runtime/elf/loader` SHALL 只负责单模块装载和 guest memory 布局。

#### Scenario: Load one parsed image into guest memory

- **WHEN** loader 收到一个 `ParsedElf`
- **THEN** 它 SHALL 完成地址空间保留、段映射、字节写入、零填充、段权限与 TLS 基础布局
- **AND** 它 SHALL 返回包含导出表、待解析 relocation、init 计划的模块对象
- **AND** 它 SHALL 不在该步骤内完成跨模块符号解析

### Requirement: Linker trait boundary

`runtime/elf/linker` SHALL 负责依赖图、符号解析和 relocation 写回。

#### Scenario: Link loaded modules through dependency graph

- **WHEN** linker 收到一个已装载模块图
- **THEN** 它 SHALL 先建立依赖顺序，再解析符号并写回 relocation
- **AND** 它 SHALL 生成稳定的 init 调用顺序
- **AND** 它 SHALL 不重新解析 ELF 原始字节

### Requirement: Typed error separation

parser、loader、linker SHALL 使用分层错误模型。

#### Scenario: Error source stays attributable

- **WHEN** 某一步失败
- **THEN** parse error SHALL 只描述格式与输入问题
- **AND** load error SHALL 只描述 guest 映射与布局问题
- **AND** link error SHALL 只描述依赖、符号解析、relocation 写回与 init 调度问题

### Requirement: Telemetry through context

loader 和 linker SHALL 通过 context 输出结构化事件。

#### Scenario: Emit structured load/link events

- **WHEN** loader 或 linker 执行关键步骤
- **THEN** 它们 SHALL 通过 context 提供的 telemetry 接口发出事件
- **AND** 不 SHALL 直接打印或私自写文件
