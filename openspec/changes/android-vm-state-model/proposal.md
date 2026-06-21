## Why

`jni-shim-foundation` 只定义了 JNI shim 的最小边界，但 Android native 目标真正跑起来时，首先需要一个稳定的 VM 状态模型。

这里的 VM 不是 Dalvik 字节码解释器，而是：

- class registry
- object store
- local/global/weak refs
- exception state
- APK-backed context
- framework / JNI dispatch 所依赖的统一状态容器

如果这层不先收敛，后续 `JNIEnv`、framework stub、Python shim、`RegisterNatives` 都会把状态散落到各模块里，最后重新变成 unidbg 里那种巨型耦合层。

## What Changes

这个 change 定义 Android VM 的状态模型与对象数据结构。

本次变更引入：

- 新 capability：`android-vm-model`
- `Emulator` 持有 `AndroidVM` 的稳定所有权模型
- class-centric `JClassDef` / `JObject` typed data model
- `RefTable`、`ExceptionState`、`ApkContext` 的稳定职责
- primitive array / object array / wrapper / string 的一等建模

这里特别约束：

- `AndroidVM` 以 class 为聚合根
- method / field 不作为与 class 并列的顶层权威 registry
- Python `@java_class` / `register(...)` 生成的完整 class definition 必须能直接进入 Rust VM

本次变更不要求：

- 完整 `JNIEnv` ABI table
- framework method stub 细节
- Python shim bridge
- `RegisterNatives`
- `JNI_OnLoad`

## Capabilities

这个 change 会新增或定义：

- android-vm-model
- testing-harness

## Impact

完成本 change 后，后续 JNI、framework、Python shim 都必须复用同一套 VM 状态模型，而不是各自维护对象和引用表。

同时：

- Python decorator 注册链路和 Rust VM authority 必须一致
- 不允许 Python 侧走 class-centric，而 Rust VM 内部又退回 method/field 分裂式 registry
