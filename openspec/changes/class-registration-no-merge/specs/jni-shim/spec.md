## MODIFIED Requirements

### Requirement: Registry-backed class and member definitions

runtime SHALL 通过显式 registry 管理 JNI class / method / field 定义。重复注册同一个 class 名 SHALL 立即失败，registry SHALL NOT 对重复定义做任何合并/覆盖（无 merge 语义）。

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

#### Scenario: Cross-source class-name collision fails fast without merge

- **WHEN** 一个 class 名已被一个注册来源（Rust framework stub 或 Python shim）注册，另一个来源再以同名注册（即使只覆盖部分 method/field）
- **THEN** registry SHALL 立即返回显式错误（`DuplicateRegistration`）
- **AND** SHALL NOT 执行任何"同名替换、其余保留"的合并
- **AND** Python bridge SHALL 把该错误映射为带 class 名的明确异常（如 `ValueError`），信息 SHALL 指出"重复定义暂不支持"
- **AND** runtime SHALL NOT 提供 `register_or_merge_class` 这类以合并为语义的注册入口

#### Scenario: Repeated framework install fails fast

- **WHEN** `FrameworkRegistry::install` 对同一批 builtin class 被调用第二次
- **THEN** registry SHALL 在首个重复 class 上立即返回显式错误
- **AND** SHALL NOT 靠合并语义把二次 install 变成静默 noop
