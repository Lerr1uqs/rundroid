# JavaClass + JavaObject + 调用逻辑 — 实现方案（对应 issue #1）

> 关联：`project_design/model/java-class.md`、`openspec/changes/python-javashim-overrides/`、
> `openspec/changes/android-vm-state-model/`、GitHub issue #1。
>
> issue 里的代码是**伪代码，只表示含义**；本方案是落地设计。
>
> **v2 变更（相对初稿）**：放弃 `JavaObject = JavaClass` 别名。
> 改为**两个真实类型** + `avm.new_object` 工厂构造，实例身份落到 VM（ObjectId/handle）。
>
> **构造语义已定（用户拍板）**：**强依赖 avm** —— `JavaObject` 始终带 ObjectId/handle，
> `Signature(args)` 无 avm 时直接报错；issue #1 的 chaining 在 emulator 上下文里跑。

---

## 0. 可行性结论

**可行。** issue #1 要的是 Python 侧的 `JavaClass`/`JavaObject` + 方法调用/链式/重载。
它是和现有「guest JNI → Python」注册链路**正交的另一个调用方向**：

| 方向 | 触发 | 路径 | 现状 |
|------|------|------|------|
| A. guest → Python | .so 调 JNI | `register_java_class` → `MethodImpl::RustNative` 回调 | **已实现** |
| B. Python → Python（本 issue） | 用户脚本 | `obj.builder().signature(b"abc").build().sign("hello")` | **未实现** |

核心机制（`__init_subclass__` 收集 + `__getattr__` 按 Java 名分派 + argc 重载）在 Python 语义上成立。

---

## 1. 命名与类型（用户确认：不要别名）

两个**真实类型**，语义对齐 Java：

- **`JavaClass`** —— 蓝图基类。用户写 `class Signature(JavaClass)`。
  对应 Java"被加载的 Class"。承载类级元数据收集（`__init_subclass__`）。
- **`JavaObject`** —— 实例类型。`avm.new_object(Signature, args)` 产出的就是它。
  对应 Java Object。携带 VM 身份（`_object_id` / `_handle`）+ 字段存储 + 分派逻辑。

> 不要 `JavaObject = JavaClass` 别名。两者是「类」与「实例」的关系。

---

## 2. 关键 Python 陷阱：`__new__` vs `__init__`

用户设想「`JavaClass` 构造函数返回 `JavaObject`」「`__init__` 里封装 `avm.new_object`」。
**结论**：

- ❌ **不能在 `__init__` 里做并返回**：`__init__` 返回值被 Python 丢弃，且它改不了
  `__new__` 已创建的对象类型。
- ✅ 要让 `Signature(args)` 产出 `JavaObject`，必须在 **`__new__`** 里委托给 `avm.new_object`
  （`__new__` 决定创建什么类型的对象）。
- ✅ 或更直接：**不重载构造**，统一用 `avm.new_object(Signature, args)`，`Signature(args)` 不直接用。

落地取**两者都支持**：`avm.new_object` 是真正原语；`JavaClass.__new__` 在 `_avm` 已绑定
时委托给它，使 `Signature(args)` 也能用（语法糖）。

---

## 3. 构造流程：`avm.new_object`

```
avm.new_object(blueprint: type[JavaClass], *args) -> JavaObject
   1. obj = JavaObject.__new__(JavaObject)            # （可选）动态建 per-blueprint 子类，
   2. obj._blueprint = blueprint                       #     让 type(obj) 更可读
   3. obj._avm = avm
   4. blueprint.__init__(obj, *args)                   # 跑用户 __init__，字段写进 obj
   5. handle = rust.register_java_object(class_name, obj)   # 存 ObjectStore + 分配 handle
   6. obj._object_id / obj._handle 回填
   7. return obj
```

- 用户方法体（如 `py_sign(self, text)`）是 blueprint 上的普通函数；
  分派时 `getattr(self._blueprint, "py_sign")(self_obj, text)`，把 JavaObject 当 `self`。
- 字段（`self._msig = ...`）直接写到 JavaObject 实例 `__dict__`，天然每实例独立。

---

## 4. 调用分派：`JavaObject.__getattr__` + 重载

- **单一匹配**：`obj.builder()` → `__getattr__("builder")` →
  `getattr(self._blueprint, entries[0].py_name)(self)`。
- **重载**：`__java_dispatch__[name]` 多条时返回 `dispatcher(*args)`，按 argc
  （`fn.__code__.co_argcount - 0`，因为分派时显式传 self，不减 1）选备选。
- **链式**：方法 `return self` / 返回新 JavaObject 即可，与分派机制无关。

蓝图 `__java_dispatch__` 由 `JavaClass.__init_subclass__` 在**类创建时**扫描
`@java_method` 元数据构建（同初稿 §2，一次扫描产出分派表 + 注册表两份）。

---

## 5. 改动清单（class + method + 文字描述）

### 5.1 `python/rundroid/javashim/base.py`（核心）

新增两个类 + 构造原语：

```python
class JavaClass:
    """蓝图基类。class Signature(JavaClass)。类级收集元数据；_avm 由 register 注入。"""
    _avm = None  # ClassVar，register(emulator, [Cls]) 时盖写

    def __init_subclass__(cls, **kw):
        super().__init_subclass__(**kw)
        # 扫 MRO，把 @java_method 元数据整理成：
        #   cls.__java_dispatch__ : dict[java_name -> list[_Entry(py_name, desc, argc)]]
        #   cls.__java_methods__  : list[(py_name, desc, fn, is_static)]   （喂 Rust）
        ...

    def __new__(cls, *args, **kw):
        if cls is JavaClass or getattr(cls, "_avm", None) is None:
            return super().__new__(cls)        # 无 avm：退化为普通实例（或按 §7 决策报错）
        return cls._avm.new_object(cls, *args, **kw)   # 语法糖：委托工厂


class JavaObject:
    """实例类型。携带 _blueprint / _avm / _object_id / _handle + 用户字段。"""
    _blueprint: type[JavaClass]
    _avm: object
    _object_id: int
    _handle: int

    def __getattr__(self, name):
        dispatch = getattr(self._blueprint, "__java_dispatch__", {})
        entries = dispatch.get(name)
        if not entries:
            raise AttributeError(f"{self._blueprint.__name__} instance has no java method {name!r}")
        if len(entries) == 1:
            return _bind(self._blueprint, entries[0].py_name, self)
        def _disp(*a, **k):
            for e in entries:
                if e.argc == len(a):
                    return _bind(self._blueprint, e.py_name, self)(*a, **k)
            raise TypeError(f"no overload of {name!r} matches argc={len(a)}")
        return _disp
```

辅助：`_bind(bp, py_name, self_obj)` = `lambda *a, **k: getattr(bp, py_name)(self_obj, *a, **k)`；
`_Entry = dataclass(frozen=True)`；descriptor name / argc / static 判定同初稿。

### 5.2 `python/rundroid/javashim/registry.py` + 新增 avm 入口

- `register(emulator, [Cls])`：除了调 `emulator.register_java_class(cls)`，
  还**给 `cls._avm = emulator`**（注入，使 `Cls(args)` / 方法体内 `self._avm` 可用）。
- 在 `Emulator`（PyO3）侧暴露 `new_object(class_name, args)`，或在 Python 侧包一层
  `avm = AvmProxy(emulator)`，`avm.new_object(blueprint, *args)`：
  建 JavaObject → 跑 blueprint `__init__` → 调 Rust `register_java_object` → 回填 handle。

### 5.3 Rust：`emulator/bindings/python/src/lib.rs`（小改）

现有 `new_java_instance(class_name)` 内部自己 `py_cls()` 无参实例化。
**拆出 / 新增**：

```rust
/// 接收已创建的 Python 对象（JavaObject），注册到 VM：存 ObjectStore + 分配 handle。
fn register_java_object(&mut self, class_name: &str, py_obj: Py<PyAny>) -> PyResult<u32> {
    // 复用 new_java_instance 的 ObjectId 分配 + ObjectStore::HostValue + RefTable::new_global 逻辑，
    // 只是把"内部 py_cls()"换成"接收外部 py_obj"。
}
```

（`new_java_instance` 可保留为 `register_java_object` + 内部实例化的薄封装，向后兼容现有测试。）

### 5.4 `decorators.py` / `__init__.py`

- `@java_class` / `@java_method` / `@java_field`：**不变**（继续只挂 metadata，仍是唯一真相源）。
- `__init__.py`：导出 `JavaClass`、`JavaObject`（真实类型，非别名）。

### 5.5 测试

新增 `python/tests/test_javaclass_call.py`：
- `test_new_object_returns_java_object`：`avm.new_object(Signature, ...)` 返回 `JavaObject`，
  且 `type(obj) is not Signature`、`obj._handle > 0`。
- `test_chained_call`：`avm.new_object(Signature).builder().signature(b"abc").build().sign("hello")`。
- `test_overload_by_argc`、`test_dispatch_by_java_name`、`test_unknown_attr_raises`。
- `test_construct_via_syntax_sugar`：`Signature(args)`（`_avm` 已注入）等价于 `avm.new_object`。

调整 `test_javashim.py`：现有用例 `class X(JavaObject)` → 改 `class X(JavaClass)`，
实例化 `emu.new_java_instance(...)` → `avm.new_object(X, ...)`（或保留 `new_java_instance` 兼容路径）。

---

## 6. 与现有两条链路的关系

- **方向 A（guest → Python）**：`register_java_class` 把 blueprint 方法包成
  `MethodImpl::RustNative`。Rust 回调时通过 `ObjectId → ObjectStore::HostValue` 拿到的
  现在是 **JavaObject**（而非裸 Python 实例）。回调里把 JavaObject 当 self 传给 blueprint 方法体。
  共享同一个 `__java_dispatch__`，零分歧。
- **方向 B（Python → Python）**：`JavaObject.__getattr__` 直接分派，不经过 Rust dispatch。
- **object identity**：JavaObject 持 `_handle`，符合 `android-vm-state-model`
  「identity 以 AndroidRuntime/ObjectStore 为权威」的方向（Python 侧只是缓存 handle）。

---

## 7. 构造语义（已决策：强依赖 avm）

`avm.new_object` 要求 avm 在场。**已选 (A) 强依赖 avm**：

- `JavaObject` 始终带 ObjectId/handle；`Signature(args)` 在 `_avm` 未注入时直接报错
  （fail-fast，符合 AGENTS.md「let-it-failed」）。
- identity 故事单一：VM（ObjectStore）为权威，Python 侧只缓存 handle。
- issue #1 的 chaining 必须在 emulator 上下文里跑（测试里先 `register(emu, [Cls])` 注入 avm）。

> 未采用 (B) 惰性 handle（纯 Python 对象 + 按需分配 handle）。它会让 identity 双轨、
> 实现更复杂，且与 `android-vm-state-model`「VM 为权威」方向相悖。

---

## 8. 风险与边界

| 风险 | 处理 |
|------|------|
| `__new__` 返回非 `cls` 实例 → Python 不自动调 `__init__` | `avm.new_object` 内部手动跑 `blueprint.__init__`；不在 `__init__` 里做注册 |
| `__getattr__` 无限递归 | 只拦截 `__java_dispatch__` 内的名；其余 `AttributeError` |
| 重载 argc 歧义（同 argc 不同类型） | 首版按 argc，文档标注；后续按 descriptor 类型 best-effort |
| py 名 == java 名 → 不走 `__getattr__` | 正确行为；重载须用不同 py 名（Python 不允许同名两方法） |
| `isinstance(obj, Signature)` 为 False | `type(obj) is JavaObject`；如需可读类型，`avm.new_object` 动态建 per-blueprint 子类 |
| 静态方法分派 | 静态方法无实例，走方向 A；`__getattr__`（实例上）只处理实例方法 |

---

## 9. 验收标准

1. `avm.new_object(Signature, ...)` 返回 `JavaObject`，带 `_handle`。
2. issue #1 链式调用跑通：`...build().sign("hello")` 返回正确字符串。
3. 重载按 argc 正确分派；未知方法名抛 `AttributeError`。
4. 方向 A E2E 不回归（`call_java_method` / `register_framework_stub` 用例）。
5. `pytest python/tests` 全绿；`uv run pytest` 跑通。
6. 无 avm 构造明确报错而非静默（强依赖 avm）。

---

## 10. 工作量与顺序

1. `base.py`：`JavaClass`（`__init_subclass__` + `__new__` 委托）+ `JavaObject`（`__getattr__`）+ 辅助（核心）
2. Rust `register_java_object`（拆 `new_java_instance`）+ Python `avm.new_object`
3. `registry.py` 注入 `_avm`；`__init__.py` 导出
4. 新测试 + 改既有用例 + `uv run pytest`

Rust 改动小（拆一个方法），需重编译 `_rundroid`。
