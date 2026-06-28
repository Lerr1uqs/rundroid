# python-javashim Specification

## Purpose
定义 `rundroid` Python javashim 层（Python 声明 Java class / method / field shim、交由 Rust VM 执行）的稳定契约：decorator 只产出 registration metadata（不 import 即污染全局）、注册显式；`JavaClass`（蓝图）与 `JavaObject`（实例）类型分离，对象构造经 `avm` 显式驱动并注册进 Rust VM；方法 dispatch 按 Java 名 + 参数个数重载，Python override 与 framework stub 共用同一分派主线、优先级稳定；Python ABI 注解严格校验、与 Java descriptor 不漂移。JNI / VM 表面统一收敛到 `emu.avm` 门面。
## Requirements
### Requirement: Python javashim decorators are metadata-only

Python javashim SHALL 采用 metadata-only decorator 模型。

#### Scenario: Import does not mutate runtime state

- **WHEN** Python 模块定义 javashim class/method/field decorators
- **THEN** decorator SHALL 只附加 metadata
- **AND** import 模块不 SHALL 自动修改 emulator runtime state

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
- **AND** 若保留 `class_types`、`method_names`、`java_instances` 一类结构，SHALL 仅作为 adapter-private implementation detail

### Requirement: Python override priority is stable

runtime SHALL 固定 Python override 与 framework stub 的优先级。

#### Scenario: Python override wins over framework stub

- **WHEN** 某个 class/member 同时存在 Rust framework stub 与 Python explicit override
- **THEN** runtime SHALL 优先选择 Python override
- **AND** 未被 override 的成员 SHALL 回落到 framework stub
- **AND** 两者 SHALL 仍共享同一套 Rust class/member 数据结构

### Requirement: Python ABI typing stays strict

Python shim SHALL 在注册和调用阶段都保持严格类型校验。

#### Scenario: Registration verifies descriptor and annotations

- **WHEN** Python shim 注册到 runtime
- **THEN** runtime SHALL 校验 descriptor 与注解的 exact match

#### Scenario: Invocation verifies returned value

- **WHEN** Python shim 返回结果给 runtime
- **THEN** runtime SHALL 校验返回值是否满足声明的 Java type

### Requirement: JavaClass and JavaObject are distinct types

Python shim SHALL 把「类」与「实例」建模为两个真实类型：`JavaClass` 为蓝图基类
（用户 `class Signature(JavaClass)`），`JavaObject` 为实例类型。`JavaObject` SHALL NOT
是 `JavaClass` 的别名。

#### Scenario: JavaObject is not an alias of JavaClass

- **WHEN** 检查 `JavaObject` 与 `JavaClass` 的关系
- **THEN** `JavaObject` SHALL 是一个独立类型
- **AND** `JavaObject is not JavaClass` SHALL 成立

#### Scenario: Subclassing JavaClass builds a method dispatch table at class creation

- **WHEN** 定义 `class Signature(JavaClass)` 且其方法带 `@java_method` 元数据
- **THEN** 类创建后 `Signature.__java_dispatch__` SHALL 已存在
- **AND** 该表 SHALL 以 Java 方法名（descriptor 中 `(` 之前的部分）为 key
- **AND** import 该类（未 `register`）SHALL NOT 修改任何 emulator runtime state

### Requirement: Object construction requires an explicit avm

构造 Java 对象 SHALL 显式提供 avm。`JavaClass.__new__` SHALL 以 avm 为必填首参
（`Cls(avm, *args)`）并委托给 `avm.new_object`。

#### Scenario: Constructing with avm produces a JavaObject with a VM handle

- **WHEN** 调用 `Signature(avm)`（avm 已绑定到某 emulator）
- **THEN** 返回值 SHALL 是 `JavaObject` 实例（`type(obj) is JavaObject`）
- **AND** 该对象 SHALL 携带有效 `_handle`（大于 0）
- **AND** 该对象 SHALL 携带指向蓝图的 `_java_class`

#### Scenario: Constructing without avm fails fast

- **WHEN** 调用 `Signature()`（未传 avm）
- **THEN** SHALL 抛出 `TypeError`（`__new__` 签名不匹配）
- **AND** SHALL NOT 创建任何 VM 对象或 handle

#### Scenario: avm is not forwarded to the java_class __init__

- **WHEN** 调用 `Signature(avm, sig_bytes)` 且蓝图声明 `def __init__(self, sig)`
- **THEN** `sig_bytes` SHALL 作为首个用户参数传给 `__init__`
- **AND** avm SHALL NOT 出现在传给 `__init__` 的参数中

### Requirement: avm.new_object registers the instance in the Rust VM

`avm.new_object(java_class, *args)` SHALL 创建 `JavaObject`、运行蓝图 `__init__` 填字段、
并注册到 Rust VM（`ObjectStore` + 全局 handle）。注册 SHALL 经 Rust 绑定的
`register_java_object(class_name, py_obj)`。

#### Scenario: new_object allocates ObjectId and global handle

- **WHEN** 调用 `avm.new_object(Signature)`
- **THEN** Rust `ObjectStore` SHALL 新增一条 `HostValue` 记录，data 为该 `JavaObject`
- **AND** `RefTable` SHALL 分配一个新的全局 handle
- **AND** 返回的 `JavaObject._handle` SHALL 等于该全局 handle

#### Scenario: new_object bypasses JavaClass.__new__ to avoid recursion

- **WHEN** `avm.new_object` 构造实例
- **THEN** SHALL 直接 `JavaObject.__new__(JavaObject)` 创建对象
- **AND** SHALL NOT 调用蓝图类（避免回到 `JavaClass.__new__` 形成递归）

#### Scenario: Instances are backed by JavaObject in HostValue for guest callbacks

- **WHEN** guest JNI 回调某实例方法（方向 A）
- **THEN** Rust SHALL 从 `HostValue` 取出 `JavaObject`
- **AND** SHALL 把该 `JavaObject` 作为 `self` 传给蓝图方法体
- **AND** 该分派 SHALL 复用与 Python 侧（方向 B）相同的 `__java_dispatch__`

### Requirement: Method dispatch by Java name with argument-count overload

`JavaObject.__getattr__` SHALL 按声明的 Java 方法名分派到蓝图方法体；同名重载 SHALL 按
实参个数（argc）解析。

#### Scenario: Single method dispatched by Java name

- **WHEN** 蓝图声明 `@java_method("sign(...)...") def py_sign(self, text)`，且实例调用 `obj.sign("x")`
- **THEN** SHALL 调用 `py_sign`，`self` 绑定为该 `JavaObject`
- **AND** `text` SHALL 接收 `"x"`

#### Scenario: Overloads resolved by argument count

- **WHEN** 同一 Java 方法名有多个 `@java_method` 重载且参数个数不同
- **AND** 实例以 N 个实参调用该名
- **THEN** SHALL 选中 argc == N 的那个重载

#### Scenario: Ambiguous overload arity is rejected

- **WHEN** 同一 Java 方法名存在两个 argc 相同的重载
- **AND** 实例以该 argc 调用
- **THEN** 行为 SHALL 为选中首个匹配（首版 argc 策略限制）
- **AND** 该限制 SHALL 在文档中标注

#### Scenario: Unknown method name raises AttributeError

- **WHEN** 实例访问不在 `__java_dispatch__` 中的属性名
- **THEN** SHALL 抛出 `AttributeError`
- **AND** SHALL NOT 触发 `__getattr__` 无限递归

#### Scenario: Chained calls thread JavaObjects

- **WHEN** 一个方法返回另一 `JavaObject`（经 `self._avm.new_object(...)` 或 `Other(self._avm)`）
- **THEN** 返回值 SHALL 是带有效 `_handle` 的 `JavaObject`
- **AND** 链式调用 `a().b().c()` SHALL 正常执行

### Requirement: Object construction is AVM-driven and new_java_instance is removed

对象构造 SHALL 统一由 AVM 驱动。Emulator 绑定 SHALL NOT 再暴露按 class_name 内部实例化的
`new_java_instance`。`register_java_object` SHALL 是唯一的对象→VM 注册入口。

#### Scenario: Emulator does not expose new_java_instance

- **WHEN** 检查 `Emulator` 的 Python 接口
- **THEN** `new_java_instance` SHALL NOT 存在
- **AND** 对象构造 SHALL 经 `Cls(avm)` 或 `avm.new_object(Cls)`

#### Scenario: register does not inject class-level avm

- **WHEN** 调用 `register(emulator, [Signature])`
- **THEN** SHALL 仅把 class 元数据注册到 Rust VM
- **AND** SHALL NOT 在 `Signature` 上设置类级 `_avm`（avm 在构造时显式传入）

### Requirement: avm is carried on each JavaObject for derived construction

每个 `JavaObject` SHALL 携带 `_avm`（取自构造时传入的 avm），供方法体内派生相关对象。

#### Scenario: Method body spawns a related object via stored avm

- **WHEN** 某 `JavaObject` 的方法体内调用 `self._avm.new_object(OtherClass)`
- **THEN** SHALL 产出新的 `JavaObject`，其 `_avm` SHALL 与原对象一致
- **AND** 新对象 SHALL 独立注册到 VM（独立 `_handle`）

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
- **THEN** `ObjectId` SHALL 来自 `AndroidRuntime::allocate_object_id`（AVM 的 `IdAllocator`）
- **AND** SHALL NOT 使用 binding 层自有计数器（如 `next_object_id`）

