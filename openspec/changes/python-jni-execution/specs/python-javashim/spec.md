## MODIFIED Requirements

### Requirement: Python is only a registration surface

Python shim SHALL 通过显式注册进入 runtime；最终 authority 是 Rust VM。绑定层 SHALL
以 `Arc<Mutex<AndroidVM>>` 持有 VM（不经 `AndroidRuntime` 包装）。

#### Scenario: Shim becomes active only after explicit register

- **WHEN** 用户定义了一个 shim class
- **THEN** 该 shim SHALL 仅在显式 `register(...)` 后进入 runtime registry

#### Scenario: Registration synchronizes a class-centric definition into Rust

- **WHEN** Python shim 调用 `register(...)`
- **THEN** runtime SHALL 将该 Python class 的 metadata 收敛成单个 class definition
- **AND** Rust 侧 SHALL 以 class-centric authority 接收它
- **AND** 不 SHALL 要求 Python 侧分别向全局 method registry / field registry 做零散注册

#### Scenario: Python is only a registration surface

- **WHEN** Python shim 完成注册
- **THEN** Rust VM SHALL 成为最终同步点与最终 authority
- **AND** Python 不 SHALL 持有独立于 Rust VM 的最终 class/member 状态
- **AND** 该注册结果 SHALL 进入 `Emulator` 直接持有的 `AndroidVM`（经 `Arc<Mutex<AndroidVM>>`）

#### Scenario: Python binding adapter state is non-authoritative

- **WHEN** Python binding 为调用/实例化维护内部缓存或 backing object 映射
- **THEN** 这些状态 SHALL NOT 被视为最终 class/member/object authority
- **AND** 运行时语义 SHALL 以 `AndroidVM` 中的 canonical state 为准
- **AND** 若保留 `class_types`、`method_names`、`java_instances` 一类结构，SHALL 仅作为 adapter-private implementation detail

### Requirement: Android VM surface is namespaced under emu.avm

Android VM / JNI 操作 SHALL 收拢到 `emu.avm` 门面下；机器层操作 SHALL 留在 `emu`。
`emu.avm` SHALL 镜像既有 `emu.fs` 子对象模式。

#### Scenario: emu.avm groups JNI/VM operations

- **WHEN** 访问 emulator 的 Android VM 表面
- **THEN** `emu.avm` SHALL 暴露 `register_java_class` / `register_java_object` / `new_object`
- **AND** SHALL 暴露过渡/调试 API `call_java_method` / `read_java_field`
- **AND** 机器层操作（`load` / `call` / `write_guest` / `fs` / `seed` / `close`）SHALL 留在 `emu`，不在 `avm` 下

#### Scenario: ObjectId is allocated by the AVM layer

- **WHEN** `register_java_object` 注册对象
- **THEN** `ObjectId` SHALL 来自 `AndroidVM` 的 `object_id_alloc`（AVM 层的 `IdAllocator`）
- **AND** SHALL NOT 使用 binding 层自有计数器（如 `next_object_id`）

## ADDED Requirements

### Requirement: Emulator exposes a JNI guest-execution surface

Python `Emulator` SHALL 暴露驱动"使用 JNI 函数表的 guest native 代码"的方法，让 guest
.so 经真实 JNI ABI 回调进注册的 class。

#### Scenario: init_jni maps the JNI ABI and installs the dispatch hook

- **WHEN** 调 `init_jni()`
- **THEN** JNIEnv / JavaVM ABI 表 SHALL 映射进 guest 内存
- **AND** 一个 trampoline code hook SHALL 被安装
- **AND** `jni_env_pointer()` / `java_vm_pointer()` SHALL 返回有效 guest 指针

#### Scenario: jni_onload invokes the module lifecycle entry

- **WHEN** `load` + 注册完 class 后调 `jni_onload()`
- **THEN** 已装载模块的 `JNI_OnLoad` SHALL 经 JavaVM 指针被调用
- **AND** 返回非法 JNI version SHALL 显式失败

#### Scenario: read_guest reads guest memory

- **WHEN** 调 `read_guest(addr, len)`
- **THEN** SHALL 返回该 guest 地址的字节（供测试校验缓冲）

#### Scenario: set_jni_verbose toggles JNI call printing

- **WHEN** 执行前 `set_jni_verbose(True)`
- **THEN** 每次 guest JNI 调用 SHALL 打印一条人类可读 trace（slot 名 + 关键参数 + 返回值）
