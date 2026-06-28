## Why

`python-javashim-overrides` 让 Python shim 能以 class-centric 方式注册到 Rust VM，
但 Python 侧的 shim 对象本身仍是"裸 Python 实例"——没有稳定的实例类型、没有 VM 身份
（ObjectId/handle），也无法在 Python 脚本里按 Java 方法名直接调用 / 链式调用 / 重载
（GitHub issue #1）。issue #1 的伪代码
`Signature().builder().signature(b"abc").build().sign("hello")` 目前跑不通，
因为 `JavaObject` 基类是空的 `pass`，没有任何分派逻辑。

## What Changes

把 Python shim 的"类"与"实例"拆成两个**真实类型**，并让实例构造落到 VM
（ObjectId/handle），与 `android-vm-state-model`「VM 为 object identity 权威」方向对齐：

- **`JavaClass`**：蓝图基类（`class Signature(JavaClass)`）。类创建时（`__init_subclass__`）
  扫描 `@java_method` 元数据，产出分派表 `__java_dispatch__` + 注册表 `__java_methods__`。
  `__new__` **显式接收 avm 作为首参**（`Signature(avm, ...)`），委托给 `avm.new_object`。
- **`JavaObject`**：真实实例类型（**非** `JavaClass` 别名）。携带
  `_java_class` / `_avm` / `_handle` + 用户字段；`__getattr__` 按 Java 方法名
  分派到蓝图方法体，重载按 argc 解析。
- **`avm.new_object(java_class, *args) -> JavaObject`**：构造原语——建 `JavaObject` →
  跑蓝图 `__init__` 填字段 → 注册到 VM（ObjectId + handle）→ 回填 handle。
  AVM（Android VM）门面经 `emu.avm` 暴露，照搬 `emu.fs` 子对象模式，收拢整个 JNI/VM 表面。
- **构造强依赖 avm**：`JavaClass` 构造**必须显式传 avm**（`Signature(avm, ...)`），不传则
  `TypeError`（fail-fast）；实例身份始终在 VM，Python 侧只缓存 handle。
- Rust 绑定新增 `register_java_object(class_name, py_obj) -> handle`：接收**已创建**的
  Python 对象（JavaObject），存 `ObjectStore` + 分配 handle（ObjectId 由 AVM 的
  `IdAllocator` 分配）。**移除** `new_java_instance`（构造改由 avm 驱动，不再按 class_name
  内部实例化）。
- Python 构造入口 `__init__` 不做注册（`__init__` 返回值被 Python 丢弃，无法改变对象类型）；
  注册必须经 `__new__` 或 `avm.new_object`。

> **命名约定**（AGENTS.md）：缩写**类名**全大写 `AVM`（不写 mixed-case `Avm`）；
> 但**属性 / 字段 / 参数 / 模块**按 Python 惯例小写——property `emu.avm`、字段 `_avm`、
> 参数 `avm`、模块 `avm.py`。类既有 `emu.fs` → `FsProxy` 同款。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `python-javashim`：增加 `JavaClass` 蓝图 / `JavaObject` 实例类型拆分、
  `avm.new_object` 的 VM-backed 构造、`__getattr__` 调用分派 + argc 重载、
  handle 注入与 fail-fast 构造、JNI/VM 表面收拢到 `emu.avm` 等要求。

## Impact

- **代码**：
  - `python/rundroid/javashim/base.py` —— `JavaClass`（`__init_subclass__` + `__new__` 委托）
    + `JavaObject`（`__getattr__` 分派）+ 辅助（核心）
  - `python/rundroid/avm.py`（新）—— `AVM` 门面代理类：`new_object` 编排 + 透传
    `register_java_class` / `register_java_object` / `call_java_method` 等（照搬 `emu.fs` 子对象模式）
  - `python/rundroid/emulator.py`（新）—— 轻量 `Emulator` wrapper 包 `_rundroid.Emulator` engine；
    `__getattr__` 透传机器层方法 + `avm` property
  - `python/rundroid/__init__.py` —— 导出 wrapper `Emulator`（替代直接 re-export `_rundroid.Emulator`）
  - `python/rundroid/javashim/registry.py` —— `register()` 经 `emulator.avm.register_java_class`
  - `python/rundroid/javashim/__init__.py` —— 导出 `JavaClass` / `JavaObject`（真实类型）
  - `emulator/bindings/python/src/lib.rs` —— 新增 flat `register_java_object`
    （用 `AndroidRuntime::allocate_object_id`，ObjectId 归 AVM 层）；**移除** `new_java_instance`；
    既有 JNI 方法保持 flat，由 Python `avm` 代理命名空间化
- **API**：新增 `emu.avm` 门面（property）收拢整个 JNI/VM 表面
  （`register_java_class` / `new_object` / `call_java_method` / `read_java_field`…）；
  `JavaObject` 实例类型；`JavaClass.__new__(cls, avm, *args)` 委托。**移除** `new_java_instance`
  （构造统一走 `Cls(emu.avm)` 或 `emu.avm.new_object(Cls)`）。需重编译 `_rundroid`。
  **BREAKING**（既有 flat JNI 调用方 / 测试改为 `emu.avm.*`；实例化改为 `Cls(avm)`）。
