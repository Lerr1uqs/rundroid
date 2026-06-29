## MODIFIED Requirements

### Requirement: RegisterNatives binds guest functions to typed methods

runtime SHALL 支持通过 `RegisterNatives` 把 guest 函数绑定到 typed method registry。

#### Scenario: RegisterNatives records explicit binding

- **WHEN** guest 调用 `RegisterNatives`
- **THEN** runtime SHALL 读取 `JNINativeMethod[]`
- **AND** 将 name/descriptor/fn ptr 绑定到对应 canonical method
- **AND** 该绑定 SHALL 与当前 method-id strategy 返回给 guest 的 `jmethodID` 保持一致
