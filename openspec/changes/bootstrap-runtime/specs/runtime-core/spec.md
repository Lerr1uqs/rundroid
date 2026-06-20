## ADDED Requirements

### Requirement: Bootstrap Runtime Workspace

项目 SHALL 为 bootstrap runtime 提供 Rust-first 的 workspace skeleton。

#### Scenario: Bootstrap workspace exists

- **WHEN** 贡献者打开仓库
- **THEN** 他们 SHALL 看到包含 bootstrap crates 的 workspace 布局
- **AND** workspace SHALL 包含 `runtime/core`、`runtime/backends/api`、`runtime/backends/unicorn`、`runtime/memory`、`runtime/elf/parse`、`runtime/elf/loader`、`runtime/elf/linker`、`runtime/os/linux`、`runtime/telemetry`、`runtime/cli`

### Requirement: ELF Layer Separation

runtime SHALL 把 ELF parser 与 loader/linker 明确分层。

#### Scenario: Parse and load responsibilities are separated

- **WHEN** 实现方开发 ELF 支持
- **THEN** `runtime/elf/parse` SHALL 只负责 ELF 结构读取与统一解析接口
- **AND** `runtime/elf/loader` SHALL 负责段映射、load bias、TLS 基础布局和模块对象构建
- **AND** `runtime/elf/linker` SHALL 负责依赖图、符号解析、重定位写回和 init/fini 调度

#### Scenario: Parser uses existing library

- **WHEN** bootstrap runtime 选择 ELF parser 实现
- **THEN** 它 SHALL 优先复用现成 Rust parser crate
- **AND** 不 SHALL 在 bootstrap 阶段手写完整 ELF parser

#### Scenario: Runtime loader remains guest-oriented

- **WHEN** bootstrap runtime 选择 ELF loader/linker 实现
- **THEN** 它 SHALL 以 Android guest 在 Unicorn 中运行的语义为准
- **AND** 不 SHALL 直接把 host-oriented 通用 loader 当成 required runtime core

### Requirement: Bootstrap Runtime Execution Path

runtime SHALL 支持以 Unicorn 作为首个 backend 的最小 ARM64 执行路径。

#### Scenario: Execute an ARM64 stub

- **WHEN** runtime 被配置为 ARM64 且 backend 为 Unicorn
- **THEN** 它 SHALL 能够 map guest memory、写寄存器并执行最小 ARM64 stub

#### Scenario: Execute an exported ELF symbol

- **WHEN** 加载一个简单 ARM64 ELF shared object
- **THEN** runtime SHALL 能按名字解析导出符号
- **AND** 用确定性参数调用它
- **AND** 返回正确结果

### Requirement: Bootstrap Scope Guard

bootstrap runtime 在判定完成时 SHALL 不要求完整 JNI、ARM32/Thumb 或完整 driver simulation。

#### Scenario: Bootstrap review scope

- **WHEN** 某个 change 按 bootstrap runtime milestone 进行 review
- **THEN** 完整 JNI、ARM32/Thumb、完整 hook support、完整 driver simulation 仍未实现也 SHALL 被视为可接受
- **AND** 如果 bootstrap 主线尚未稳定，review SHALL 拒绝在这些方向上的不必要扩张
