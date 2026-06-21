## ADDED Requirements

### Requirement: Android VM state model is testable through harness

testing harness SHALL 能验证 Android VM 的基础状态语义。

#### Scenario: APK context is observable to framework-facing logic

- **WHEN** harness 运行一个带 APK metadata 的 Android VM case
- **THEN** case SHALL 能观察 package name、version 或 signature 数据通过 VM context 被读取

#### Scenario: Reference lifetime is observable

- **WHEN** harness 创建 local/global/weak refs 并结束 local frame
- **THEN** case SHALL 能断言 local refs 被清理，而 global refs 仍然有效
