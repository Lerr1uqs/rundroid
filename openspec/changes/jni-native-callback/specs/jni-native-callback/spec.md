## ADDED Requirements

### Requirement: host-to-guest native method invocation through registerNatives

runtime SHALL 支持通过 RegisterNatives 绑定的 guest 函数指针进行反向调用。

#### Scenario: RegisterNatives-bound method can be invoked from host

- **WHEN** 一个 Java native method 已被 RegisterNatives 绑定 guest 函数指针（MethodId → GuestPtr）
- **AND** guest/framework/Python 通过 `CallXxxMethod` 调用该方法
- **THEN** runtime SHALL 在 JNI dispatch 主线中检测到 native binding
- **AND** SHALL 通过 sentinel 机制（`call_guest_function`）在 guest 侧执行该函数指针
- **AND** SHALL 将函数返回值按 JType 解码后返回给调用方

#### Scenario: Native binding error without GuestFunctionCaller

- **WHEN** 一个 Java native method 已通过 RegisterNatives 绑定 guest 函数指针
- **AND** 当前 JNIEnvSurface 未配置 `GuestFunctionCaller`（如纯单元测试）
- **THEN** runtime SHALL 返回 `JniError::Internal` 而非静默失败或崩溃

### Requirement: Java_* mangled symbol fallback

runtime SHALL 支持通过 `Java_*` mangled 符号名在已装载 ELF 模块中查找 native 函数。

#### Scenario: Unregistered native is resolved by symbol name

- **WHEN** 一个 Java native method **未**通过 RegisterNatives 注册
- **AND** 已装载的 ELF 模块中存在 `Java_<mangled_class>_<method>` 格式的导出符号
- **THEN** runtime SHALL 能通过符号名查找解析到该函数地址
- **AND** SHALL 优先使用无重载短格式 `Java_<class>_<method>`，失败后再尝试带签名后缀的长格式

#### Scenario: Native not found returns None

- **WHEN** native method 既未通过 RegisterNatives 注册，ELF 模块中也无匹配的 `Java_*` 符号
- **THEN** `find_native_guest_fn` SHALL 返回 `None`
- **AND** JNI dispatch SHALL 回退到 Rust/Python handler 查找（现有行为不变）

### Requirement: Parameters and return value are marshalled correctly

runtime SHALL 按照 ARM64 ABI 规则编组 JNI native 函数的参数与返回值。

#### Scenario: JNIEnv pointer and class/object ref are first two parameters

- **WHEN** native 函数被调用
- **THEN** x0 SHALL 包含 guest 可见的 `JNIEnv*` 地址（当前线程的 env_ptr）
- **AND** x1 SHALL 包含 `jclass`（static 方法）或 `jobject`（instance 方法）的 object id

#### Scenario: Primitive parameters map to registers

- **WHEN** native 方法包含 `int` / `long` / `float` / `double` / `boolean` / `byte` / `char` / `short` 参数
- **THEN** 每个参数 SHALL 按 JValue → u64 规则编组进入 x2..x7（按顺序）

#### Scenario: Return value is decoded by JType

- **WHEN** native 函数执行完毕返回到 sentinel
- **THEN** runtime SHALL 读取 x0
- **AND** 按方法声明的返回类型解码：Int→`JValue::Int(x0 as i32)`、Long→`JValue::Long(x0 as i64)`、Object→`JValue::Object(ObjectId(...))`、Void→`JValue::Void` 等

### Requirement: Nested emu_start works transparently

runtime SHALL 支持嵌套的 host→guest native 调用。

#### Scenario: Guest native calls back into JNI then into another guest native

- **WHEN** guest native 函数内部通过 JNIEnv function table 调回 host
- **AND** host handler（如 framework stub）再次触发 guest native 调用
- **THEN** 嵌套的 `emu_start` SHALL 正常执行（Unicorn 原生支持）
- **AND** SHALL 正确返回到各层调用点

### Requirement: Sentinel mechanism is shared with call_export

host→guest native 调用 SHALL 复用 `call_export` 的 sentinel + stack 映射机制。

#### Scenario: Same sentinel region used for all guest native calls

- **WHEN** JNI native 回调使用 `call_guest_function`
- **THEN** sentinel + stack SHALL 使用与 `call_export` 相同的固定地址映射
- **AND** sentinel/stack SHALL 跨多次调用复用（不重复映射）
- **AND** LR SHALL 指向 sentinel，执行到该地址时 `emu_start` 自然停止
