# android-vm-model Specification

## Purpose
定义 Android VM 的状态模型与 class-centric 对象数据结构：`Emulator` 持有 `AndroidVM` 的稳定所有权模型，以 **class 为聚合根**（`JClassDef` / `JObject` typed data model），`RefTable`（local / global / weak refs）、`ExceptionState`、`ApkContext` 的稳定职责，以及 primitive / object array、wrapper、string 的一等建模。约束后续 JNIEnv、framework stub、Python shim、RegisterNatives 必须复用同一套 VM 状态模型，禁止 method / field 退回与 class 并列的分裂式顶层 registry，避免状态散落成 unidbg 式巨型耦合层。
## Requirements
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

#### Scenario: Multiple registration surfaces converge through AndroidVM

- **WHEN** Python registration surface 或 Rust builtin registration surface 注册 Java class/member
- **THEN** 它们 SHALL 先被规整到统一 class definition 模型
- **AND** 再注册到 `Emulator` 直接持有的 `AndroidVM`
- **AND** `AndroidVM` / `JniRegistry` SHALL 持有最终 authority

#### Scenario: Binding and JNI trampoline hook share one AndroidVM

- **WHEN** Python 绑定层初始化 JNI 执行（`init_jni`）
- **THEN** 绑定层 SHALL 以 `Arc<Mutex<AndroidVM>>` 持有 VM
- **AND** JNI trampoline hook SHALL 拿到同一 VM 的 `Arc::clone`
- **AND** 经绑定层注册的 class SHALL 对 guest JNI dispatch 可见（同一 registry）

#### Scenario: Python binding caches are never the final VM state

- **WHEN** Python binding 为适配 shim 调用而维护 class/object/member 相关缓存
- **THEN** 这些缓存 SHALL NOT 成为最终 VM authority
- **AND** class/member/object identity SHALL 以 `AndroidVM` 状态为准
- **AND** 类似 `class_types`、`method_names`、`java_instances` 的结构若仍存在，SHALL 仅作为 binding-layer adapter cache

### Requirement: No VM re-entry during guest JNI dispatch

guest JNI dispatch（在 `emu_start` 期间）触发 Python override 时，该 override SHALL NOT 再次获取 VM 锁。这是单线程仿真的内在约束。

#### Scenario: Python JNI override does not re-enter the VM

- **WHEN** guest JNI dispatch 调到一个 Python `@java_method` override
- **THEN** 该 override SHALL NOT 调 `avm.new_object` / `emulator.call` 等再次入 VM / engine 的路径
- **AND** 绑定层文档 SHALL 明确标注该限制
- **AND** 测试 fixture SHALL 仅使用纯计算型 override（读字段、算返回值），不得依赖 guest dispatch 期间的 VM re-entry

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
