## ADDED Requirements

### Requirement: Framework behavior is class-spec driven

runtime SHALL 通过 class-oriented spec registry 实现 Android framework stubs。

#### Scenario: Framework class behavior is registered by class spec

- **WHEN** 实现方新增一个 framework class stub
- **THEN** 它 SHALL 通过 class-spec 注册到 framework registry
- **AND** 不 SHALL 依赖新的 giant signature switch 作为稳定主线

#### Scenario: Framework builtins enter the same VM authority as Python shims

- **WHEN** runtime 注册 Rust builtin framework class
- **THEN** 它 SHALL 进入 Rust VM 持有的统一 class/member registry
- **AND** 它 SHALL 通过 `Emulator` 持有的 `AndroidRuntime` 完成注册
- **AND** 后续 Python shim override 或补环境 SHALL 复用同一套底层数据结构

### Requirement: APK-backed package and signature stubs

runtime SHALL 为 package/signature 相关 Android 行为提供 APK-backed stub。

#### Scenario: PackageManager and PackageInfo read from ApkContext

- **WHEN** guest 查询 package info、version、signatures 或相关元数据
- **THEN** framework stub SHALL 从 `ApkContext` 或等价模型读取

### Requirement: Service lookup uses service registry

runtime SHALL 把 `getSystemService` 风格行为收敛到 service registry。

#### Scenario: Context resolves service by registry

- **WHEN** guest 通过 `Context` / `Application` 查询 system service
- **THEN** runtime SHALL 通过统一 `ServiceRegistry` 返回 service stub

### Requirement: Core Java utility classes are first-class stubs

runtime SHALL 为常见 Java utility classes 提供最小但正式的 stub。

#### Scenario: String and wrappers do not rely on ad-hoc signature cases

- **WHEN** guest 通过 JNI 调用 `String`、`Class`、primitive wrapper 或常见 collection 类
- **THEN** runtime SHALL 通过正式 stub handler 提供最小行为
