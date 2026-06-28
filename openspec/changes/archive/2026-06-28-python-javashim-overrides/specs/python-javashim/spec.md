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

#### Scenario: Registration synchronizes a class-centric definition into Rust

- **WHEN** Python shim 调用 `register(...)`
- **THEN** runtime SHALL 将该 Python class 的 metadata 收敛成单个 class definition
- **AND** Rust 侧 SHALL 以 class-centric authority 接收它
- **AND** 不 SHALL 要求 Python 侧分别向全局 method registry / field registry 做零散注册

#### Scenario: Python is only a registration surface

- **WHEN** Python shim 完成注册
- **THEN** Rust VM SHALL 成为最终同步点与最终 authority
- **AND** Python 不 SHALL 持有独立于 Rust VM 的最终 class/member 状态
- **AND** 该注册结果 SHALL 进入 `Emulator` 持有的 `AndroidRuntime`

#### Scenario: Python binding adapter state is non-authoritative

- **WHEN** Python binding 为调用/实例化维护内部缓存或 backing object 映射
- **THEN** 这些状态 SHALL NOT 被视为最终 class/member/object authority
- **AND** 运行时语义 SHALL 以 `AndroidRuntime` / `AndroidVM` 中的 canonical state 为准
- **AND** 若保留 `class_types`、`method_names`、`java_instances` 一类结构，SHALL 仅作为 adapter-private implementation detail

### Requirement: Python override priority is stable

runtime SHALL 固定 Python override 与 framework stub 的优先级。

#### Scenario: Python override wins over framework stub

- **WHEN** 某个 class/member 同时存在 Rust framework stub 与 Python explicit override
- **THEN** runtime SHALL 优先选择 Python override
- **AND** 未被 override 的成员 SHALL 回落到 framework stub
- **AND** 两者 SHALL 仍共享同一套 Rust class/member 数据结构

### Requirement: Python ABI typing stays strict

Python shim SHALL 在注册和调用阶段都保持严格类型校验。

#### Scenario: Registration verifies descriptor and annotations

- **WHEN** Python shim 注册到 runtime
- **THEN** runtime SHALL 校验 descriptor 与注解的 exact match

#### Scenario: Invocation verifies returned value

- **WHEN** Python shim 返回结果给 runtime
- **THEN** runtime SHALL 校验返回值是否满足声明的 Java type
