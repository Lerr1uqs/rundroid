## Context

`python-javashim-overrides` 已建立 Python shim 的注册模型（metadata-only decorator、显式
`register`、class-centric 同步到 Rust `JClassDef`、override 优先级、strict typing）。
但 Python 侧的 shim **对象**仍是裸 Python 实例：

- `python/rundroid/javashim/base.py` 的 `JavaObject` 是空 `pass`，无分派逻辑。
- Python 脚本无法按 Java 方法名调用 / 链式 / 重载（issue #1 跑不通）。
- 实例没有 VM 身份；`new_java_instance` 内部按 class_name 自己 `py_cls()` 无参实例化，
  既绕过 AVM，也无法参数化构造。

现有 Rust 侧已具备所需身份原语（`emulator/bindings/python/src/lib.rs`）：
`ObjectStore::insert(oid, name, ObjectStorage::HostValue{..})` + `RefTable::new_global(oid) -> u32`。
本 change 复用它们，不在 VM 层新增身份模型（与 `android-vm-state-model` 一致）。

约束（来自 AGENTS.md / 既有 change）：
- 中文注释；let-it-failed，无兜底；`get_xxx` 风格禁止；缩写类名全大写（`AVM` 不写 `Avm`），但属性/字段/参数按 Python 惯例小写（`avm`）。
- import 不修改 runtime；注册显式；Rust VM 为最终权威。

## Goals / Non-Goals

**Goals:**
- Python 侧 `JavaClass`（蓝图）/ `JavaObject`（实例）两真实类型，非别名。
- `avm.new_object(java_class, *args)` 构造原语，实例落 VM（ObjectId + handle）。
- `JavaClass.__new__(cls, avm, *args)` 委托，使 `Signature(avm, ...)` 等价于 `avm.new_object`。
- **移除** `new_java_instance`：构造统一由 AVM 驱动。
- `JavaObject.__getattr__` 按 Java 名分派 + argc 重载。
- issue #1 链式调用跑通。

**Non-Goals:**
- 不改 VM 身份模型（复用 `ObjectStore` / `RefTable`）。
- 不实现「方法体内回 call native」（`self._avm.call(...)`）—— 仅存 `_avm` 引用留口。
- 不做 descriptor 类型级重载解析（首版 argc；文档标注限制）。
- 不做静态方法的 Python 侧 `__getattr__` 分派（静态方法走 guest→Python 方向 A）。

## Decisions

### 决策 1：两个真实类型，`JavaObject` 不是 `JavaClass` 别名

`JavaClass` = 蓝图（用户 `class Signature(JavaClass)`），类级收集元数据；
`JavaObject` = 实例，携带 VM 身份与字段。方法体定义在蓝图上，分派时把 `JavaObject` 当 `self`。

```python
# python/rundroid/javashim/base.py
class JavaClass:
    """蓝图基类。子类用 @java_method 声明方法体。构造时显式传 avm。"""
    # 注意：不再有类级 _avm。avm 在构造时显式传入（见决策 2/5）。

    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        # 扫 MRO，把 @java_method 元数据整理成两份：
        #   cls.__java_dispatch__ : dict[java_name -> list[_Entry(py_name, desc, argc)]]
        #   cls.__java_methods__  : list[(py_name, desc, fn, is_static)]   （喂 Rust register）
        ...


class JavaObject:
    """实例类型。携带 _java_class/_avm/_handle + 用户字段。"""
    _java_class: "type[JavaClass]"
    _avm: object           # 由构造时传入的 avm 复制而来，供方法体内 spawn 相关对象
    _handle: int
```

> 命名约定（AGENTS.md）：缩写**类名**全大写 `AVM`（不写 mixed-case `Avm`）；但
> **属性 / 字段 / 参数** 按 Python 惯例小写：property `emu.avm`、字段 `_avm`、参数 `avm`。
> 类比既有 `emu.fs` → `FsProxy`（小写属性 + 大写类）。

> 不用 `JavaObject = JavaClass` 别名：用户明确要求「类」与「实例」分开。
> 不用 metaclass 方案：会要求用户写 `class Signature(JavaObject)`，与设计文档
> `class Signature(JavaClass)` 及既有约定冲突。

### 决策 2：`__new__` 改写——显式接收 avm 作首参（不在 `__init__` 做）

**关键 Python 语义**：`__init__` 返回值被丢弃、改不了 `__new__` 已建的对象类型。
所以「构造产出 `JavaObject`」必须在 `__new__`，或直接用 `avm.new_object`。
用户决策：avm **显式作为构造首参**传入，`Signature(avm, ...)`。

```python
class JavaClass:
    def __new__(cls, avm, *args, **kwargs):
        # avm 是必填首参；不传 → Python 直接 TypeError（签名不匹配），天然 fail-fast
        return avm.new_object(cls, *args, **kwargs)
```

要点：
- `Signature(avm)` → `__new__(Signature, avm)` → `avm.new_object(Signature)`。
- `Signature(avm, sig)` → `__new__(Signature, avm, sig)` → `avm.new_object(Signature, sig)`，
  `sig` 作为真实参数流到蓝图 `__init__(self, sig)`；avm 被 `__new__` 剥离，不进 `__init__`。
- `avm.new_object` 返回 `JavaObject`（非 `cls` 实例）→ Python 检测到返回类型不是 `cls`
  就**跳过** `cls.__init__`——故字段初始化在 `avm.new_object` 内手动跑（决策 3）。
- `avm.new_object` 内部用 `JavaObject.__new__(JavaObject)` 建对象，**不**调 `cls()`，
  避免回到 `JavaClass.__new__` 形成递归。

> 对比「register 注入类级 `_avm`」方案：显式传 avm 更直白、无隐式状态耦合，
> 且 `register()` 只管注册元数据，职责更纯。

### 决策 3：`emu.avm` 门面 + `avm.new_object` + `register_java_object`——handle 怎么来

**`AVM` 类作为 `emu.avm` property 门面**（照搬 `emu.fs` 模式），收拢整个 Android VM / JNI 表面：

```
emu.avm:  register_java_class / register_java_object / new_object
          call_java_method / read_java_field / register_framework_stub / release / java_instance
emu:      load / call / write_guest / fs / seed / close   （机器层，不动）
```

- property 而非方法（`emu.avm`，与 `emu.fs` 一致）：`avm = emu.avm; avm.register_java_class(...)`
  或内联 `emu.avm.register_java_class(...)`。
- 实现为**纯 Python `AVM` 代理类 + 轻量 `rundroid.Emulator` wrapper**（包 `_rundroid.Emulator` engine）：
  `new_object` 编排是 Python（要构造 `JavaObject`、跑蓝图 `__init__`）；Rust 不重构 `AndroidRuntime`
  为 `Arc` 共享，**只新增 flat 的 `register_java_object`**。`rundroid.Emulator` 用 `__getattr__`
  透传 engine，再加 `avm` property。
- `call_java_method` / `read_java_field` 是过渡/调试 API（见 `project_design/docs/pyemulator.md`），
  收进 AVM 命名空间便于后续被 DVM 替换。

构造分两层：Python 编排 + Rust 落身份。

**Python 侧 `AVM`（轻量代理类，经 `emu.avm` 取得）**：

```python
# python/rundroid/avm.py
class AVM:
    """emulator 的 Android VM 门面：封装对象构造。经 emu.avm 取得。"""
    def __init__(self, engine):
        self._engine = engine          # _rundroid.Emulator（flat Rust 方法在此）

    def new_object(self, java_class, *args):
        """构造 JavaObject 并注册到 VM，回填 handle。"""
        # 1. 建实例（绕过 JavaClass.__new__，避免递归）
        obj = JavaObject.__new__(JavaObject)
        obj._java_class = java_class
        obj._avm = self                      # 方法体内可 self._avm.new_object(Other) 或 Other(self._avm)
        # 2. 跑蓝图 __init__ 填字段（self = obj；avm 已剥离，不传给 __init__）
        java_class.__init__(obj, *args)
        # 3. 注册到 Rust VM：存 ObjectStore + 分配 handle（ObjectId 由 AVM 的 IdAllocator 分配）
        class_name = java_class.__java_class_name__
        handle = self._engine.register_java_object(class_name, obj)   # -> u32
        obj._handle = handle
        # 仅回填 _handle；ObjectId 是 Rust 内部 ObjectStore key，不暴露给 Python
        return obj
```

**Rust 侧 `register_java_object`（新增，唯一构造落点）**：

```rust
// emulator/bindings/python/src/lib.rs
/// 接收已创建的 Python 对象（JavaObject），注册到 VM：存 ObjectStore + 分配 global handle。
fn register_java_object(&mut self, class_name: &str, py_obj: Py<PyAny>) -> PyResult<u32> {
    // ObjectId 由 AVM 层 IdAllocator 分配（AndroidRuntime::allocate_object_id），
    // 不再用 binding 自有的 next_object_id 计数器——ObjectId 归 AVM 层。
    let object_id = self.runtime.allocate_object_id();
    self.runtime.vm.objects.lock().unwrap().insert(
        object_id,
        class_name.to_string(),
        ObjectStorage::HostValue { data: Box::new(py_obj) },
    ).map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;
    let handle = self.runtime.refs_mut().new_global(object_id);   // -> u32
    Ok(handle)
}
```

> **为何 `JavaObject` 只持 `_handle`，不持 `_object_id`**：`ObjectId` 是 Rust VM
> `ObjectStore` 的 key，方向 A（guest→Python）回调用它定位对象，全程在 Rust 内完成
> （handle → `RefTable::resolve` → ObjectId → `ObjectStore`）。Python 侧不持有也不暴露
> ObjectId；`_handle`（JNI `jobject` 等价物）足以做身份引用与生命周期回收
> （`release(handle)` 在 Rust 内反查 ObjectId，清理 `ObjectStore` + 删 global ref）。

**移除 `new_java_instance`**：原「按 class_name 内部实例化」的入口删除。
调用方改写：`emu.new_java_instance("X")` → `X(emu.avm)` 或 `emu.avm.new_object(X)`。
`call_java_method(handle, ...)` / `release_java_instance(handle)` 等 handle 类 API **保留**
（方向 A 回调 / Python 侧 handle 操作仍需要）。

> `HostValue` 现在存的是 `JavaObject`（而非裸 Python 实例）。方向 A（guest→Python）回调时
> 从 `HostValue` 取出 `JavaObject`，把**它**当 `self` 传给蓝图方法体——与方向 B 共用
> 同一张 `__java_dispatch__`，零分歧。

### 决策 4：`__getattr__` 分派 + argc 重载

```python
class JavaObject:
    def __getattr__(self, name):
        # 只拦截蓝图分派表里的 Java 方法名；其余直接 AttributeError（防递归）
        dispatch = getattr(self._java_class, "__java_dispatch__", {})
        entries = dispatch.get(name)
        if not entries:
            raise AttributeError(
                f"{self._java_class.__name__} 实例无 Java 方法 {name!r}"
            )
        if len(entries) == 1:
            return _bind(self._java_class, entries[0].py_name, self)
        # 重载：按 argc 选（首版限制：同 argc 不同类型会歧义）
        def _dispatcher(*a, **k):
            for e in entries:
                if e.argc == len(a):
                    return _bind(self._java_class, e.py_name, self)(*a, **k)
            raise TypeError(f"{name!r} 无匹配 argc={len(a)} 的重载")
        return _dispatcher

def _bind(java_class, py_name, self_obj):
    """把蓝图上的函数绑定到指定 self_obj（JavaObject）。"""
    fn = getattr(java_class, py_name)          # 普通函数（Py3 未绑定）
    return lambda *a, **k: fn(self_obj, *a, **k)
```

`__java_dispatch__` 由 `JavaClass.__init_subclass__` 在类创建时构建（唯一真相源 =
`@java_method` 元数据），同时产出 `__java_methods__` 供 `register()` 喂 Rust。
argc 计算：`fn.__code__.co_argcount`（分派时显式传 self，不减 1）。

### 决策 5：构造强依赖 avm（显式传入，fail-fast）

`JavaObject` 始终带身份；构造必须 `Cls(avm, ...)` 或 `avm.new_object(Cls, ...)`。
不传 avm → `JavaClass.__new__(cls, avm, ...)` 签名不匹配 → Python 直接 `TypeError`，无需手写校验。
identity 单轨：VM（`ObjectStore`）权威，Python 仅缓存 `_handle`。
issue #1 的 chaining 在 emulator 上下文：首对象 `Signature(emu.avm)`，后续对象由方法体内
`self._avm.new_object(...)` 派生（AVM 引用随 `_avm` 字段传递）。

### 决策 6：方向 A 递归重入——锁释放 + `&self` 方法（避免自锁 / pyo3 借用冲突）

方法体内 `self._avm.new_object(Other)` 经 Rust bridge（方向 A，`call_java_method` /
`wrap_python_method`）触发时，会回调 `register_java_object` 再次访问 `runtime`。
两层潜在自锁必须同时规避：

1. **ObjectStore Mutex 自锁**：原实现持守 `objects.lock()` 进 Python 调用，方法体内
   `register_java_object` 再次 `objects.lock()` → 同线程 Mutex 死锁。
   修法：**锁内只 clone 出所需 owned 数据**（`Py<PyAny>` 引用 + `class_name`），
   **立即释放锁**，再进 Python 调用。`call_java_method` 与 `wrap_python_method` 都改如此。
2. **pyo3 `#[pyclass]` 借用冲突**：`call_java_method`（`&self`，`PyRef`）调用 Python 时，
   方法体内 `register_java_object`（原 `&mut self`，`PyRefMut`）→ pyo3 抛 `Already borrowed`。
   修法：`runtime` 包成 `RwLock<AndroidRuntime>`，**`register_java_object` 改 `&self`**
   （write guard 内部访问 runtime）——两个 `&self` 的 `PyRef` 可共存。所有访问 runtime 的
   方法经 `read()/write()` guard；凡调用 Python 的方法（`call_java_method` 等）必须在
   Python 调用前释放 guard，否则 write 会与持守的 read 自锁。

### 决策 7：override 命中按 argc（同名不同签名不误命中）

`PythonShimAdapter` 的 override 存在性缓存键含 **argc**：`(class_name, java_name, argc)`，
仅缓存 instance method（静态方法不进 `__java_dispatch__`，走 `dispatch_call`）。
`call_java_method` 用 `sig.args.len()` 查询。这样 `foo()I`（Python override，argc 0）与
`foo(I)I`（framework stub，argc 1）互不干扰：`foo()I` 命中 Python 路径，`foo(I)I` 回落
framework。与 Python 侧 argc 重载解析一致（同 argc 不同类型仍是首版限制，见 Non-Goal）。

### 端到端示例（issue #1 复刻）

```python
from rundroid.javashim import JavaClass, java_class, java_method, register

@java_class("com/example/Signature")
class Signature(JavaClass):
    def __init__(self):
        self._msig = b""

    @java_method("Signature([B)V")
    def py_init_with_sig(self, sig): self._msig = bytes(sig)

    @java_method("sign(Ljava/lang/String;)Ljava/lang/String;")
    def py_sign(self, text): return f"signed:{text}:{self._msig.hex()}"

    @java_method("builder()Lcom/example/SignatureBuilder;")
    def py_builder(self):
        # 方法体内派生新对象：复用本实例的 _avm
        return self._avm.new_object(SignatureBuilder)

@java_class("com/example/SignatureBuilder")
class SignatureBuilder(JavaClass):
    def __init__(self): self._sig = b""
    @java_method("signature([B)Lcom/example/SignatureBuilder;")
    def py_signature(self, sig): self._sig = bytes(sig); return self
    @java_method("build()Lcom/example/Signature;")
    def py_build(self):
        s = self._avm.new_object(Signature); s.py_init_with_sig(self._sig); return s

emu = Emulator("arm64", "unicorn", 42)
register(emu, [Signature, SignatureBuilder])   # 仅注册元数据到 Rust，不注入 _avm

# Signature(emu.avm) → __new__(Signature, emu.avm) → avm.new_object(Signature) → JavaObject(handle)
out = Signature(emu.avm).builder().signature(b"abc").build().sign("hello")
# 等价显式写法：emu.avm.new_object(Signature).builder().signature(b"abc").build().sign("hello")
```

## Risks / Trade-offs

- **`__new__` 返回非 `cls` → Python 不调 `__init__`** → `avm.new_object` 内手动跑
  `java_class.__init__`；不在 `__init__` 做注册。已规避。
- **`__getattr__` 无限递归** → 仅拦截 `__java_dispatch__` 内的名，其余 `AttributeError`。已规避。
- **方向 A 递归重入**（方法体内 `new_object` 经 Rust bridge 回调 engine）→ 两层自锁：
  ObjectStore Mutex（持锁进 Python）+ pyo3 `PyRef`/`PyRefMut` 借用冲突。规避见决策 6。
- **override 同名不同签名误命中** → 缓存键含 argc，见决策 7。
- **重载 argc 歧义**（同 argc 不同类型） → 首版按 argc，文档/spec 标注限制；后续可升级
  descriptor 类型匹配。已记录为 Non-Goal。
- **`isinstance(obj, Signature)` 为 False**（`type(obj) is JavaObject`）→ 如需可读类型，
  `avm.new_object` 可动态建 per-java_class 子类（`type(name, (JavaObject,), {})`）。
  首版不做，spec 不强制。
- **BREAKING：移除 `new_java_instance`** → 既有 `test_javashim.py` 调用方需改写为
  `Cls(avm)` / `avm.new_object(Cls)`。属预期 break，记入 tasks。
- **`py 名 == java 名`** → 直接命中 `__dict__`，不走 `__getattr__`（正确）；重载须用不同 py 名
  （Python 不允许同名两方法）。文档说明。
- **avm 显式传入的 ergonomics** → 每个顶层构造要带 `emu.avm`；方法体内可借 `_avm` 复用，
  不必反复传。可接受。

## Migration Plan

1. 先加 Rust `register_java_object`，**移除** `new_java_instance`，重编译 `_rundroid`。
2. 再加 Python `JavaClass`/`JavaObject`/`AVM`（`emu.avm` 代理）。
3. 改 `test_javashim.py`（基类换名、实例化改 `Cls(emu.avm)` / `emu.avm.new_object(Cls)`、
   移除 `new_java_instance` 用例）。
4. 新增 `test_javaclass_call.py`（chain / overload / 显式 avm / 不传 avm 报错）。
5. `uv run pytest` 全绿；既有方向 A E2E 不回归。

回退：删除新 change 代码、恢复 `new_java_instance` 即可（无数据迁移）。

## Open Questions

- `AVM` 暴露形式：`emu.avm`（代理，匹配用户 `avm.new_object` 习惯）vs 直接 `emu.new_object`。
  倾向 `emu.avm` 代理，留出后续 `avm.call_static` / `avm.new_string` 等扩展位。待实现时定。
