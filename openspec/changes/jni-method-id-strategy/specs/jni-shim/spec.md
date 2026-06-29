## MODIFIED Requirements

### Requirement: Registry-backed class and member definitions

runtime SHALL 通过显式 registry 管理 JNI class / method / field 定义。

#### Scenario: Register class and members without central switch-case

- **WHEN** 实现方新增一个 Java shim class、method 或 field
- **THEN** 它 SHALL 通过 registry 注册，而不是编辑中心化 switch-case 分派逻辑
- **AND** method key SHALL 使用完整 `MethodSig`
- **AND** field key SHALL 使用完整 `FieldSig`

#### Scenario: Rust builtin and Python registration share one authority

- **WHEN** 实现方通过 Rust builtin 或 Python bridge 注册 class/member
- **THEN** 两者 SHALL 进入同一套 Rust registry authority
- **AND** 两者 SHALL 先通过 `Emulator` 持有的 `AndroidRuntime` 完成统一注册
- **AND** 不 SHALL 演化为 builtin 一套、Python 一套的平行状态系统

#### Scenario: Registration collisions fail fast

- **WHEN** runtime 或 Python bridge 试图重复注册同一个 class、method 或 field 签名
- **THEN** registry SHALL 立即返回显式错误
- **AND** 不 SHALL 静默覆盖旧定义

#### Scenario: Guest method-id lookup uses the configured strategy

- **WHEN** guest 调用 `GetMethodID` 或 `GetStaticMethodID`
- **THEN** runtime SHALL 先用 canonical class/member registry 命中目标 method
- **AND** 再通过已配置的 method-id generator 返回稳定 token
- **AND** 同一 canonical method 在同一 runtime 配置下 SHALL 返回稳定一致的 guest method id

### Requirement: Minimal JNIEnv and JavaVM foundation

runtime SHALL 提供 foundation 阶段最小但稳定的 `JNIEnv` / `JavaVM` surface。

#### Scenario: Call methods and access fields through JNIEnv surface

- **WHEN** guest 代码或 runtime 内部逻辑需要通过 `JNIEnv` 调用 method 或访问 field
- **THEN** runtime SHALL 提供最小 method call 和 field get/set 能力
- **AND** 这些操作 SHALL 复用同一 registry / dispatch / verify 主线

#### Scenario: Attach current thread for JNI_OnLoad path

- **WHEN** `JNI_OnLoad` 或等价 JNI 入口请求当前线程的 `JNIEnv`
- **THEN** runtime SHALL 能通过 `JavaVM` surface 完成当前线程 attach
- **AND** 提供与该线程绑定的最小 `JNIEnv` 能力

#### Scenario: Call paths resolve guest method ids through the canonical map

- **WHEN** guest 把某个 `jmethodID` 传回 `Call*Method`、`CallStatic*Method`、`NewObject` 或等价调用
- **THEN** runtime SHALL 先通过 method-id generator 对应的 canonical 映射解析到唯一 method
- **AND** 再执行后续 dispatch
- **AND** 不 SHALL 依赖“注册顺序递增值”作为 guest method id 的隐式语义
