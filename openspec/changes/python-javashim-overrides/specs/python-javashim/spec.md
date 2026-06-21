## ADDED Requirements

### Requirement: Python javashim decorators are metadata-only

Python javashim SHALL 采用 metadata-only decorator 模型。

#### Scenario: Import does not mutate runtime state

- **WHEN** Python 模块定义 javashim class/method/field decorators
- **THEN** decorator SHALL 只附加 metadata
- **AND** import 模块不 SHALL 自动修改 emulator runtime state

### Requirement: Python registration is explicit

Python shim SHALL 通过显式注册进入 runtime。

#### Scenario: Shim becomes active only after explicit register

- **WHEN** 用户定义了一个 shim class
- **THEN** 该 shim SHALL 仅在显式 `register(...)` 后进入 runtime registry

### Requirement: Python override priority is stable

runtime SHALL 固定 Python override 与 framework stub 的优先级。

#### Scenario: Python override wins over framework stub

- **WHEN** 某个 class/member 同时存在 Rust framework stub 与 Python explicit override
- **THEN** runtime SHALL 优先选择 Python override
- **AND** 未被 override 的成员 SHALL 回落到 framework stub

### Requirement: Python ABI typing stays strict

Python shim SHALL 在注册和调用阶段都保持严格类型校验。

#### Scenario: Registration verifies descriptor and annotations

- **WHEN** Python shim 注册到 runtime
- **THEN** runtime SHALL 校验 descriptor 与注解的 exact match

#### Scenario: Invocation verifies returned value

- **WHEN** Python shim 返回结果给 runtime
- **THEN** runtime SHALL 校验返回值是否满足声明的 Java type
