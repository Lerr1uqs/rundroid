## ADDED Requirements

### Requirement: Android VM is a state model, not a bytecode engine

runtime SHALL 把 Android VM 实现为 JNI-facing Java world state model。

#### Scenario: VM scope excludes Dalvik bytecode execution

- **WHEN** 实现方构建 Android VM
- **THEN** 该 VM SHALL 负责 class/object/ref/exception/framework 所需状态
- **AND** 当前阶段不 SHALL 要求 Dalvik/ART bytecode interpretation

### Requirement: Class-centric VM authority with typed ids

Android VM SHALL 以 class-centric authority 持有 Java world 状态，并对 class/object/member 使用 typed id。

#### Scenario: Internal authority uses typed ids

- **WHEN** runtime 创建或查询 class、object、method、field
- **THEN** 内部 authority SHALL 使用 typed id
- **AND** 不 SHALL 仅以 hash 或原始签名字串作为最终权威模型

#### Scenario: Class is the aggregate root for methods and fields

- **WHEN** runtime 注册或查询 Java method / field
- **THEN** 它 SHALL 以 class definition 作为聚合根
- **AND** method / field SHALL 作为 class member 被持有
- **AND** 不 SHALL 把 method registry / field registry 作为与 class 并列的最终权威状态

### Requirement: Rust VM is the final synchronization point

Android VM SHALL 以 Rust 侧 VM / registry 作为最终同步点。

#### Scenario: Builtin classes and Python-registered classes converge to one authority

- **WHEN** runtime 注册 Rust builtin framework class 或 Python javashim class
- **THEN** 两者 SHALL 收敛到同一套 Rust class/member registry
- **AND** 它们 SHALL 共享相同的 class/member 数据结构
- **AND** 不 SHALL 分别维护两套彼此独立的 method/field authority

#### Scenario: Multiple registration surfaces converge through AndroidRuntime

- **WHEN** Python registration surface 或 Rust builtin registration surface 注册 Java class/member
- **THEN** 它们 SHALL 先被规整到统一 class definition 模型
- **AND** 再注册到 `Emulator` 持有的 `AndroidRuntime`
- **AND** `AndroidRuntime` 内部的 `AndroidVM` / `JniRegistry` SHALL 持有最终 authority

#### Scenario: Python binding caches are never the final VM state

- **WHEN** Python binding 为适配 shim 调用而维护 class/object/member 相关缓存
- **THEN** 这些缓存 SHALL NOT 成为最终 VM authority
- **AND** class/member/object identity SHALL 以 `AndroidRuntime` / `AndroidVM` 状态为准
- **AND** 类似 `class_types`、`method_names`、`java_instances` 的结构若仍存在，SHALL 仅作为 binding-layer adapter cache

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
