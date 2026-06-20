## MODIFIED Requirements

### Requirement: Runtime workspace structure

项目 SHALL 为 emulator 维护 Rust-first 的 workspace 结构，并对外暴露 emulator-oriented 装配入口。

#### Scenario: Workspace layout present

- **WHEN** 贡献者查看仓库
- **THEN** 项目 SHALL 暴露包含 emulator crates 和 Python/tooling 目录的 workspace

#### Scenario: Top-level runtime directory is renamed to emulator

- **WHEN** 实现方整理顶层 crate 目录
- **THEN** 顶层 `runtime/` SHALL 重构为 `emulator/`
- **AND** backend、memory、loader、OS、JNI、driver、telemetry、bindings 等 crate SHALL 位于 `emulator/` 目录树下
- **AND** 不 SHALL 继续把 `runtime/` 作为稳定的顶层 crate 目录名

#### Scenario: External API is emulator-oriented

- **WHEN** 实现方向 Rust 或 Python 暴露面向用户的主入口对象
- **THEN** 该主入口 SHALL 使用 `Emulator` 或等价 emulator-oriented 命名
- **AND** 它 SHALL 作为 backend、memory、loader、OS、JNI、driver、telemetry 的装配层
- **AND** 不 SHALL 继续把对外主入口稳定命名为 `Runtime`
