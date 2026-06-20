## ADDED Requirements

### Requirement: Unified Telemetry Subsystem

runtime SHALL 提供一个统一的 telemetry 子系统，承载日志、结构化事件、trace 输出和 debugger transcript。

#### Scenario: Telemetry owned by one subsystem

- **WHEN** runtime 某个子系统发出诊断信息或 trace 信息
- **THEN** 它 SHALL 通过统一 telemetry 子系统输出
- **AND** 它 SHALL 不依赖拆开的 observability/debugging 双体系

### Requirement: Telemetry Mode Control

telemetry 行为 SHALL 由配置或 flag 控制。

#### Scenario: Telemetry disabled

- **WHEN** telemetry mode 被设置为 `disabled`
- **THEN** 执行过程 SHALL 不输出除强制执行结果之外的 telemetry artifacts

#### Scenario: Events only

- **WHEN** telemetry mode 被设置为 `events_only`
- **THEN** runtime SHALL 输出结构化事件
- **AND** 它 SHALL 不要求人类可读日志输出

### Requirement: Bootstrap Telemetry Artifacts

bootstrap runtime 执行 SHALL 输出适合调试和回放的 machine-readable artifacts。

#### Scenario: Case run emits artifacts

- **WHEN** telemetry 打开时执行一个 smoke case
- **THEN** 这次运行 SHALL 输出 `result.json`
- **AND** 输出 `backend.json`
- **AND** 输出 `events.jsonl`
