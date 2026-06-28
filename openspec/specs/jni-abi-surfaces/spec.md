# jni-abi-surfaces Specification

## Purpose
定义 guest 真实可见的 JNI ABI 表面：`_JavaVM` / `_JNIEnv` 指针模型、`JNIInvokeInterface` 与 JNIEnv function table 的槽位语义、以及 ABI slot → Rust handler 的映射模型，让 native so 能按真实 ABI 调进 Rust（而非纯 host-side mock）并对齐 unidbg 行为。约束 `GetEnv` / `AttachCurrentThread` / `FindClass` / `GetMethodID` / `Call*Method` 等最小主线的 ABI 一致性与 slot 元数据驱动 dispatch。
## Requirements
### Requirement: JavaVM and JNIEnv are guest-visible ABI objects

runtime SHALL 在 guest address space 中构造真实可见的 `JavaVM*` 与 `JNIEnv*`。

#### Scenario: JNI tables exist in guest memory

- **WHEN** Android VM 初始化完成
- **THEN** guest SHALL 能拿到稳定的 `_JavaVM` 与 `_JNIEnv` 指针
- **AND** 这些指针 SHALL 指向真实 guest memory 中的 invoke/function table

### Requirement: ABI slots map to Rust-owned handlers

runtime SHALL 为每个已支持的 JNI ABI slot 绑定统一 handler。

#### Scenario: Slot dispatch goes through central ABI mapping

- **WHEN** guest 通过 function table 调用一个已支持 JNI entry
- **THEN** runtime SHALL 通过 slot metadata 找到对应 Rust handler
- **AND** handler SHALL 复用 VM registry / dispatch 主线

### Requirement: Minimal attach and lookup path

runtime SHALL 先支持最小 thread attach 与 class/member lookup 主线。

#### Scenario: GetEnv and AttachCurrentThread return active env

- **WHEN** guest 调用 `GetEnv` 或 `AttachCurrentThread`
- **THEN** runtime SHALL 返回当前 active `JNIEnv`

#### Scenario: Class and member lookup works through ABI

- **WHEN** guest 调用 `FindClass`、`GetMethodID`、`GetFieldID` 或等价 entry
- **THEN** runtime SHALL 能通过 ABI 入口查询 VM registry

### Requirement: ABI handlers remain bridge-only

JNI ABI handlers SHALL 保持桥接职责，不直接承载 framework 业务逻辑。

#### Scenario: Framework behavior is delegated after lookup

- **WHEN** ABI handler 处理 method 或 field 调用
- **THEN** 它 SHALL 先完成参数解码和 lookup
- **AND** 后续业务行为 SHALL 委派给 framework stub / shim / native dispatch 层

