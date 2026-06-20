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
