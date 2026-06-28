## ADDED Requirements

### Requirement: JNI foundation cases are executable through harness

testing harness SHALL 为 JNI foundation 提供可执行的回归 case。

#### Scenario: Run a successful Python shim registration case

- **WHEN** harness 运行一个通过 Python shim 显式注册 class / method 的 case
- **THEN** runtime SHALL 能完成注册并成功 dispatch 对应 method / field
- **AND** case SHALL 不需要修改 harness 核心代码

#### Scenario: Registration mismatch fails before runtime execution

- **WHEN** harness 运行一个 Python 注解与 Java descriptor 不匹配的 case
- **THEN** registration SHALL 在执行前或启动时显式失败
- **AND** case artifact SHALL 能观察到 descriptor mismatch 的归因信息

#### Scenario: JNI_OnLoad path is covered by harness

- **WHEN** harness 运行一个带最小 `JNI_OnLoad` 入口的 JNI case
- **THEN** runtime SHALL 走通 `JavaVM` / `JNIEnv` foundation 调用链
- **AND** case SHALL 能断言该入口成功或以结构化方式失败
