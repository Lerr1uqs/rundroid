## ADDED Requirements

### Requirement: JNI native callback is testable through harness

testing harness SHALL 能验证 RegisterNatives 绑定的 native 方法通过 guest 执行并返回正确值。

#### Scenario: RegisterNatives → call → return value assertion

- **WHEN** harness 注册一个 Rust 侧 `JClassDef`，其中某 method 通过 RegisterNatives 绑定到一个已知 guest 函数指针
- **AND** harness 通过 JNI ABI 调用该方法
- **THEN** runtime SHALL 通过 sentinel 机制在 guest 侧执行该函数指针
- **AND** 返回值 SHALL 与预期一致

#### Scenario: Java_* symbol fallback path is covered

- **WHEN** harness 加载一个包含 `Java_*` 导出符号的 ELF 模块
- **AND** native method 未通过 RegisterNatives 注册
- **AND** harness 通过 JNI ABI 调用该方法
- **THEN** runtime SHALL 通过符号表查找解析到函数地址
- **AND** 通过 sentinel 机制在 guest 侧执行

#### Scenario: Nested emu_start is covered

- **WHEN** harness 配置的场景触发 guest native → JNI call → guest native 的嵌套路径
- **THEN** harness SHALL 能验证嵌套调用返回正确值
