## 1. Rust 绑定：对象注册入口（ObjectId 归 AVM 层）

- [x] 1.1 在 `emulator/bindings/python/src/lib.rs` 新增 `register_java_object(&mut self, class_name: &str, py_obj: Py<PyAny>) -> PyResult<u32>`：`object_id = self.runtime.allocate_object_id()`（**用 AVM 的 IdAllocator，不用 binding 计数器**）→ `ObjectStore::insert(.., ObjectStorage::HostValue{..})` → `refs_mut().new_global` → 返回 handle（加中文注释）
- [x] 1.2 **移除** `new_java_instance` 及 `PyEmulatorBridge::next_object_id` 字段；确认 `call_java_method` / `release_java_instance` / `java_instance` 等 handle 类 API 不受影响（保持 flat，由 Python avm 代理命名空间化）
- [x] 1.3 `maturin develop` 重编译 `_rundroid`，确认 `import _rundroid` 不报错

## 2. Python avm 门面 + Emulator wrapper

- [x] 2.1 新建 `python/rundroid/avm.py`：`class AVM` 持有 engine（`_rundroid.Emulator`），透传 flat Rust 方法（`register_java_class` / `register_java_object` / `call_java_method` / `read_java_field` / `register_framework_stub` / `release` / `java_instance`）
- [x] 2.2 在 `AVM` 实现 `new_object(java_class, *args)`：`JavaObject.__new__(JavaObject)` → 挂 `_java_class`/`_avm` → `java_class.__init__(obj, *args)`（avm 不传入）→ `engine.register_java_object(class_name, obj)` → 回填 `_handle` → 返回
- [x] 2.3 新建 `python/rundroid/emulator.py`：轻量 `class Emulator` 包 `_rundroid.Emulator` engine；`__getattr__` 透传机器层方法（`load`/`call`/`write_guest`/`fs`/`seed`/`close`）；`@property def avm(self)` 返回 `AVM(self._engine)`
- [x] 2.4 `python/rundroid/__init__.py` 改为 `from .emulator import Emulator`（替代 `from ._rundroid import Emulator`）

## 3. Python 核心类型：JavaClass / JavaObject

- [x] 3.1 在 `python/rundroid/javashim/base.py` 实现 `JavaClass`：`__init_subclass__` 扫描 MRO 的 `@java_method` 元数据 → 产出 `__java_dispatch__`（dict[java_name → list[_Entry]]）+ `__java_methods__`（list[(py_name, desc, fn, is_static)]）
- [x] 3.2 实现 `JavaClass.__new__(cls, avm, *args, **kwargs)`：`return avm.new_object(cls, *args, **kwargs)`（avm 必填首参，不传则 Python TypeError）
- [x] 3.3 实现 `JavaObject`：字段 `_java_class` / `_avm` / `_handle`；`__getattr__` 只拦截 `__java_dispatch__` 内的名，单条直返、多条按 argc dispatcher，未知名抛 `AttributeError`（防递归）
- [x] 3.4 辅助：`_Entry`（frozen dataclass：py_name/desc/argc）、`_bind(java_class, py_name, self_obj)`、descriptor name / argc / static 判定函数（均带中文注释）

## 4. 注册 / decorator 导出

- [x] 4.1 简化 `python/rundroid/javashim/registry.py` 的 `register()`：优先复用 `__init_subclass__` 已建的 `__java_methods__`，调 `emulator.avm.register_java_class(cls)`；**不再注入** 类级 `_avm`
- [x] 4.2 `python/rundroid/javashim/__init__.py` 导出 `JavaClass`、`JavaObject`（真实类型，非别名）；`decorators.py` 保持 metadata-only 不变

## 5. 测试

- [x] 5.1 改写 `python/tests/test_javashim.py`：`Emulator` 从 `rundroid` 导入（wrapper）；基类 `JavaObject` → `JavaClass`；实例化 `emu.new_java_instance(name)` → `Cls(emu.avm)` / `emu.avm.new_object(Cls)`；flat JNI 调用改 `emu.avm.*`；移除 `new_java_instance` 用例
- [x] 5.2 新增 `python/tests/test_javaclass_call.py`：复刻 issue #1 链式 `Signature(emu.avm).builder().signature(b"abc").build().sign("hello")` 断言返回值
- [x] 5.3 新增重载测试（同名不同 argc 按 argc 选中）、未知方法名抛 `AttributeError`、`Signature()` 不传 avm 抛 `TypeError`
- [x] 5.4 新增「import 不修改 runtime」「`emu.avm.new_object(Cls)` 产出对象 `type is JavaObject` 且 `_handle > 0`」「方法体内 `self._avm.new_object(Other)` 派生独立对象」用例

## 6. 验证

- [x] 6.1 `uv run pytest python/tests` 全绿（含既有方向 A E2E 不回归）
- [x] 6.2 `openspec validate --type change python-javaclass-call --strict` 通过
- [x] 6.3 手测 issue #1 端到端链式调用，确认返回正确字符串
