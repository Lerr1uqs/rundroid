## ADDED Requirements

### Requirement: RegisterNatives binds guest functions to typed methods

runtime SHALL 支持通过 `RegisterNatives` 把 guest 函数绑定到 typed method registry。

#### Scenario: RegisterNatives records explicit binding

- **WHEN** guest 调用 `RegisterNatives`
- **THEN** runtime SHALL 读取 `JNINativeMethod[]`
- **AND** 将 name/descriptor/fn ptr 绑定到对应 `MethodId`

### Requirement: Dynamic Java_* lookup remains as fallback

runtime SHALL 在未显式注册时支持 `Java_*` 符号名 fallback 查找。

#### Scenario: Unregistered native method resolves through mangled name

- **WHEN** 某个 native method 未通过 `RegisterNatives` 绑定
- **THEN** runtime MAY 尝试按 `Java_*` mangled name 查找 loaded module symbol
- **AND** 命中后 SHALL 把结果纳入统一 native dispatch 主线

### Requirement: JNI_OnLoad is a first-class module lifecycle step

runtime SHALL 在模块装载后处理 `JNI_OnLoad`。

#### Scenario: Module with JNI_OnLoad is invoked through JavaVM

- **WHEN** 已装载模块导出 `JNI_OnLoad`
- **THEN** runtime SHALL 通过 active `JavaVM*` 调用它
- **AND** SHALL 校验其返回的 JNI version

#### Scenario: Illegal JNI version fails explicitly

- **WHEN** `JNI_OnLoad` 返回不受支持的 JNI version
- **THEN** runtime SHALL 显式失败

### Requirement: Native lifecycle emits structured telemetry

runtime SHALL 为 native JNI lifecycle 输出结构化事件。

#### Scenario: Register and onload events are observable

- **WHEN** runtime 处理 `RegisterNatives`、dynamic native lookup 或 `JNI_OnLoad`
- **THEN** 它 SHALL 输出结构化 telemetry
