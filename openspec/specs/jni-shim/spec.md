# jni-shim Specification

## Purpose
定义 `rundroid` 第一版 JNI / shim foundation 的稳定可扩展边界：Rust（`emulator/jni` crate）持有 JNI 核心状态与 dispatch 权威，Python 只负责声明 class / method / field shim（decorator 产出 registration metadata，不 import 即污染全局）。约束 canonical 类型模型（`JType` / `JValue` / `MethodSig` / `FieldSig`）、descriptor parser、registry、dispatch、reference table 的基础语义；注册阶段严格校验 descriptor 与 Python 注解，调用阶段统一走 registry / dispatch 而非中心化 switch-case；`JNIEnv` / `JavaVM` / `JNI_OnLoad` 的最小 surface 与当前阶段边界清晰。目标是后续新增 Java shim class / method 不再需要编辑中心化分派代码。
## Requirements
### Requirement: JNI shim workspace skeleton

项目 SHALL 为 JNI shim foundation 暴露稳定的 workspace skeleton。

#### Scenario: Emulator JNI crate skeleton exists

- **WHEN** 贡献者查看 emulator workspace
- **THEN** 项目 SHALL 包含 `emulator/jni` crate
- **AND** 该 crate SHALL 至少暴露 `lib.rs`、`args.rs`、`class.rs`、`descriptor.rs`、`dispatch.rs`、`field.rs`、`jnienv.rs`、`javavm.rs`、`object.rs`、`refs.rs`、`registry.rs`、`types.rs`、`verify.rs`

#### Scenario: Python shim package skeleton exists

- **WHEN** 贡献者查看 Python 包结构
- **THEN** 项目 SHALL 提供面向 JNI shim 的 Python 包目录
- **AND** 该目录 SHALL 至少包含 `__init__.py`、`base.py`、`decorators.py`、`registry.py`、`types.py`

#### Scenario: Python bridge remains a binding layer

- **WHEN** 实现方扩展 Python 与 Rust 的 JNI 注册桥
- **THEN** bridge 代码 SHALL 位于 `emulator/bindings/python` 或等价 binding 层
- **AND** bridge SHALL 不成为 JNI 核心状态与分派逻辑的权威归属

#### Scenario: PyEmulator does not own final Java world state

- **WHEN** Python binding 维护 class/member/object 相关辅助状态
- **THEN** `PyEmulator` 或等价 binding object SHALL NOT 成为最终 Java world state container
- **AND** 这类状态 SHALL 仅作为 binding adapter cache
- **AND** 最终 authority SHALL 仍位于 `AndroidRuntime` / `AndroidVM` / JNI runtime state

### Requirement: Emulator owns JNI integration

对外 emulator 装配层 SHALL 成为 JNI shim foundation 的主入口归属。

#### Scenario: Emulator is the public integration surface

- **WHEN** 用户从 Rust 或 Python 使用 JNI shim 能力
- **THEN** 他们 SHALL 通过 `Emulator` 或等价 emulator-oriented 主入口接入
- **AND** `emulator/jni` SHALL 作为 emulator 持有的内部子系统工作

#### Scenario: Runtime naming is removed from the stable directory and API shape

- **WHEN** 实现方演进 JNI、driver、loader 与 OS 的装配关系
- **THEN** 对外稳定 API 不 SHALL 继续以 `Runtime` 作为主入口名
- **AND** 稳定目录结构不 SHALL 继续以 `runtime/` 作为顶层 crate 根目录

### Requirement: Canonical JNI type system

runtime SHALL 为 JNI foundation 提供稳定的 canonical type system。

#### Scenario: Parse descriptor into canonical typed signature

- **WHEN** runtime 或 Python registration bridge 接收 method / field descriptor
- **THEN** 它 SHALL 把 descriptor 解析为 canonical `JType`、`MethodSig` 或 `FieldSig`
- **AND** 内部 class name SHALL 使用 slash-separated 形式
- **AND** 非法 descriptor SHALL 在注册阶段显式失败

#### Scenario: Primitive and null semantics stay strict

- **WHEN** runtime 校验 JNI 参数或返回值
- **THEN** `Null` SHALL 仅用于 object / array 兼容位置
- **AND** primitive 返回值不 SHALL 以 `Null` 代替
- **AND** runtime 不 SHALL 对 primitive 类型做 silent widening / narrowing

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

### Requirement: Unified Rust-native and Python-shim dispatch

runtime SHALL 让 Rust-native 和 Python-shim 成员共享统一 dispatch 主线。

#### Scenario: One dispatch surface routes both implementation kinds

- **WHEN** `JNIEnv` 或等价 runtime surface 发起 method / field 调用
- **THEN** runtime SHALL 先通过 typed signature 查询 registry
- **AND** 再按 member implementation 类型分发到 Rust-native 或 Python-shim handler
- **AND** 新增 Python shim 不 SHALL 需要编辑 `JNIEnv` 核心分支

### Requirement: Reference semantics are owned by Rust runtime

runtime SHALL 由 Rust 持有 JNI object identity 和 reference semantics 权威状态。

#### Scenario: Local and global refs have distinct lifecycle

- **WHEN** runtime 为某个 object 创建 local、global 或 weak global reference
- **THEN** 它 SHALL 显式记录 reference kind 与 object identity
- **AND** local refs SHALL 能在 call frame 结束时清理
- **AND** global refs 不 SHALL 因局部调用结束而失效

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

### Requirement: Python shim registration is explicit and strict

Python shim SHALL 采用 metadata-only decorator 和显式注册模型。

#### Scenario: Decorators attach metadata without global side effects

- **WHEN** Python 模块定义 `@java_class`、`@java_method`、`@java_field` 或等价 decorator
- **THEN** decorator SHALL 只附加 metadata
- **AND** import 模块不 SHALL 自动污染全局 runtime registry

#### Scenario: Python annotations must match Java descriptor

- **WHEN** Python bridge 把一个 decorated method / field 注册到 Rust runtime
- **THEN** bridge SHALL 解析 descriptor、提取注解并执行严格匹配校验
- **AND** 不匹配 SHALL 在注册阶段直接失败
- **AND** 错误信息 SHALL 包含 class name、member name、descriptor 和注解摘要

### Requirement: JNI path is observable

runtime SHALL 为 JNI foundation 输出结构化 telemetry。

#### Scenario: Registration and invocation emit structured events

- **WHEN** runtime 处理 class / method / field 注册、JNI 调用、reference 生命周期或错误
- **THEN** 它 SHALL 输出结构化 JNI telemetry 事件
- **AND** 错误事件 SHALL 至少包含 class、member、descriptor 与类型上下文

