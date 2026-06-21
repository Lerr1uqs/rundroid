## ADDED Requirements

### Requirement: Python javashim override behavior is testable

testing harness SHALL 覆盖 Python javashim 的 override 与 strictness 行为。

#### Scenario: Python shim overrides framework behavior

- **WHEN** harness 运行一个 Python shim 覆盖 framework stub 的 case
- **THEN** case SHALL 能观察到 override 生效

#### Scenario: Bad annotations fail before runtime execution

- **WHEN** harness 运行一个注解与 descriptor 不匹配的 Python shim case
- **THEN** 注册 SHALL 在执行前失败
