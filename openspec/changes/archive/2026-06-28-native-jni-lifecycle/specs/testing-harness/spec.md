## ADDED Requirements

### Requirement: Native JNI lifecycle is covered by harness

testing harness SHALL 覆盖 native JNI lifecycle 的关键主线。

#### Scenario: RegisterNatives path is executable

- **WHEN** harness 运行一个通过 `RegisterNatives` 绑定 native method 的 case
- **THEN** case SHALL 能断言绑定生效并可调用

#### Scenario: JNI_OnLoad path is executable

- **WHEN** harness 运行一个导出 `JNI_OnLoad` 的模块 case
- **THEN** case SHALL 能断言 `JNI_OnLoad` 成功或以结构化方式失败
