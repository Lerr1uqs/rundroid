## MODIFIED Requirements

### Requirement: Class-centric VM authority with typed ids

Android VM SHALL 以 class-centric authority 持有 Java world 状态，并对 class/object/member 使用 typed id。

#### Scenario: Internal authority uses typed ids

- **WHEN** runtime 创建或查询 class、object、method、field
- **THEN** 内部 authority SHALL 使用 typed id
- **AND** 不 SHALL 仅以 hash 或原始签名字串作为最终权威模型

#### Scenario: Class is the aggregate root for methods and fields

- **WHEN** runtime 注册或查询 Java method / field
- **THEN** 它 SHALL 以 class definition 作为聚合根
- **AND** method / field SHALL 作为 class member 被持有
- **AND** 不 SHALL 把 method registry / field registry 作为与 class 并列的最终权威状态

#### Scenario: Guest method ids are not the final member authority

- **WHEN** guest 通过 `GetMethodID`、`Call*Method` 或 `RegisterNatives` 使用 `jmethodID`
- **THEN** 该 `jmethodID` SHALL 被视为 guest ABI token
- **AND** 它 SHALL NOT 被视为 Rust 内部 method authority 本身
- **AND** 最终 method authority SHALL 仍以 class-centric canonical member model 为准
