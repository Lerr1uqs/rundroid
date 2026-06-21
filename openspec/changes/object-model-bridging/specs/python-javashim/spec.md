## MODIFIED Requirements

### Requirement: Python registration is explicit

Python shim SHALL 通过显式注册进入 runtime。

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
- **AND** 该注册结果 SHALL 进入 `Emulator` 持有的 `AndroidRuntime`

#### Scenario: Python binding adapter state is non-authoritative

- **WHEN** Python binding 为调用/实例化维护内部缓存或 backing object 映射
- **THEN** 这些状态 SHALL NOT 被视为最终 class/member/object authority
- **AND** 运行时语义 SHALL 以 `AndroidRuntime` / `AndroidVM` 中的 canonical state 为准
- **AND** `class_types`、`method_names` 等 adapter cache SHALL 下沉到专用 `PythonShimAdapter` 结构
- **AND** 对象实例 SHALL 通过 `ObjectStore`（`HostValue`）存储，不 SHALL 再维护独立的 `java_instances` HashMap

### Requirement: Python override priority is stable

runtime SHALL 固定 Python override 与 framework stub 的优先级。

#### Scenario: Python override wins over framework stub

- **WHEN** 某个 class/member 同时存在 Rust framework stub 与 Python explicit override
- **THEN** runtime SHALL 优先选择 Python override
- **AND** 未被 override 的成员 SHALL 回落到 framework stub
- **AND** 两者 SHALL 仍共享同一套 Rust class/member 数据结构
- **AND** 优先级 SHALL 通过 `register_or_merge_class` 的 merge 语义实现，dispatch 不再做两路分支判断

#### Scenario: Dispatch flows through unified Rust registry path

- **WHEN** Python shim 调用 `call_java_method(handle, sig, args)`
- **THEN** dispatch SHALL 统一走 `RefTable::resolve(handle)` → `ObjectStore::get(ObjectId)` → `JniRegistry::dispatch_call(sig, jni_args, refs)` 路径
- **AND** Python override 和 framework stub 的优先级由 `JniRegistry` 中已注册的 `MethodImpl` 决定
- **AND** 不 SHALL 再通过 `method_names` adapter cache 做"先查 Python 再 fallback Rust"的分支判断
