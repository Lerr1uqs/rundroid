## ADDED Requirements

### Requirement: Method-id strategy is regression-tested through harness

testing harness SHALL 覆盖 guest method-id 生成策略的稳定行为。

#### Scenario: Default method-id strategy matches unidbg-compatible hash

- **WHEN** harness 运行一个最小 JNI case 并请求某个已知 method 的 `GetMethodID`
- **THEN** 返回值 SHALL 等于该 canonical signature 的 Java `String.hashCode()` 语义结果

#### Scenario: Method call path accepts the generated guest id

- **WHEN** harness 使用 `GetMethodID` 返回的 guest id 继续执行 `NewObject` 或 `Call*Method`
- **THEN** runtime SHALL 成功解析回唯一 canonical method 并完成调用

#### Scenario: Method-id collision fails explicitly

- **WHEN** harness 注入一个会制造 method-id 冲突的自定义 generator
- **THEN** runtime SHALL 在注册或初始化阶段显式失败
- **AND** 错误 SHALL 可被测试断言
