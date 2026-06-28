## Purpose

定义 `rundroid` 基于 case 的测试系统和 differential execution 的稳定要求，保证 case 格式、资源解析方式、以及可选 oracle 的工作模式长期一致。
## Requirements
### Requirement: Declarative runtime cases

项目 SHALL 以声明式方式定义 runtime case。

#### Scenario: Add a new case

- **WHEN** 贡献者新增一个 runtime case
- **THEN** 他们 SHALL 通过 case 文件完成，而不是修改 harness 核心

### Requirement: Resource-aware case execution

case SHALL 通过资源系统解析外部资产。

#### Scenario: Resource-backed case

- **WHEN** case 引用了 resource URI
- **THEN** harness SHALL 通过声明的 resource packs 解析它

### Requirement: Optional differential oracle

harness SHALL 支持把非 Rust oracle 作为可选项。

#### Scenario: Rust-only execution path

- **WHEN** 可选 oracle 不可用
- **THEN** Rust execution path SHALL 仍然可用

### Requirement: Scratch memory is harness-scoped

scratch memory SHALL 仅作为 testing-harness / stub / 调试辅助能力存在。

#### Scenario: Scratch allocation is used only by harness-facing APIs

- **WHEN** case runner、Python stub 或调试辅助逻辑需要准备目标输入输出缓冲区
- **THEN** harness MAY 通过 scratch API 申请和管理临时目标内存
- **AND** 该能力 SHALL 被标记为 testing/harness/debug 用途，而不是稳定目标 userspace ABI

#### Scenario: Scratch memory does not replace target heap semantics

- **WHEN** 目标程序的正常执行路径需要动态内存
- **THEN** runtime SHALL 继续依赖目标程序自身的 `malloc`、`mmap`、`brk` 或等价机制
- **AND** scratch API 不 SHALL 被实现为正式 heap allocator 的替代品

### Requirement: JNI ABI is observable through harness

testing harness SHALL 能验证 guest 通过真实 JNI ABI table 调用 runtime。

#### Scenario: Guest invokes JNI call through JNIEnv table

- **WHEN** harness 运行一个最小 JNI case
- **THEN** case SHALL 能通过 `_JNIEnv` function table 调用至少一个 class lookup 和一个 method call

#### Scenario: Thread attach path is covered

- **WHEN** harness 运行一个依赖 `GetEnv` 或 `AttachCurrentThread` 的 case
- **THEN** case SHALL 能断言返回的 env 对当前 VM 有效

