## ADDED Requirements

### Requirement: Android VM is a state model, not a bytecode engine

runtime SHALL 把 Android VM 实现为 JNI-facing Java world state model。

#### Scenario: VM scope excludes Dalvik bytecode execution

- **WHEN** 实现方构建 Android VM
- **THEN** 该 VM SHALL 负责 class/object/ref/exception/framework 所需状态
- **AND** 当前阶段不 SHALL 要求 Dalvik/ART bytecode interpretation

### Requirement: Typed class/object/method/field registries

Android VM SHALL 以 typed registry 持有 class/object/method/field 权威状态。

#### Scenario: Internal authority uses typed ids

- **WHEN** runtime 创建或查询 class、object、method、field
- **THEN** 内部 authority SHALL 使用 typed id
- **AND** 不 SHALL 仅以 hash 或原始签名字串作为最终权威模型

### Requirement: Reference semantics are explicit

Android VM SHALL 显式区分 local/global/weak references。

#### Scenario: Local refs are frame-scoped

- **WHEN** runtime 创建 local reference
- **THEN** 它 SHALL 绑定到明确的 local frame 语义
- **AND** frame 结束时 SHALL 可被清理

#### Scenario: Global and weak-global refs remain distinguishable

- **WHEN** runtime 创建 global 或 weak-global reference
- **THEN** 它 SHALL 显式记录 ref kind
- **AND** `GetObjectRefType` 等价语义 SHALL 能区分它们

### Requirement: APK-backed framework context is first-class

Android VM SHALL 为 framework 行为提供统一的 APK context。

#### Scenario: Framework reads package/signature/asset data from VM context

- **WHEN** framework stub 需要 package name、version、manifest、signature 或 asset 数据
- **THEN** 它 SHALL 从统一 `ApkContext` 或等价模型读取
- **AND** 不 SHALL 依赖散落在多个 helper 中的隐式状态

### Requirement: Arrays and wrappers are first-class object kinds

Android VM SHALL 为 primitive arrays、object arrays 和常见 primitive wrappers 提供一等建模。

#### Scenario: Arrays are not collapsed into opaque generic objects

- **WHEN** runtime 创建 `byte[]`、`int[]`、`Object[]` 或等价数组对象
- **THEN** 它 SHALL 以显式 array kind 存储
- **AND** 后续 region read/write 与 element access SHALL 复用该一等模型
