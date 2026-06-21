## ADDED Requirements

### Requirement: JNI ABI is observable through harness

testing harness SHALL 能验证 guest 通过真实 JNI ABI table 调用 runtime。

#### Scenario: Guest invokes JNI call through JNIEnv table

- **WHEN** harness 运行一个最小 JNI case
- **THEN** case SHALL 能通过 `_JNIEnv` function table 调用至少一个 class lookup 和一个 method call

#### Scenario: Thread attach path is covered

- **WHEN** harness 运行一个依赖 `GetEnv` 或 `AttachCurrentThread` 的 case
- **THEN** case SHALL 能断言返回的 env 对当前 VM 有效
