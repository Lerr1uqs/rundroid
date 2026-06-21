## ADDED Requirements

### Requirement: Python object instances flow through ObjectStore

Python Java 对象实例的生命周期 SHALL 与 Rust `ObjectStore` 同步。

#### Scenario: new_java_instance creates ObjectStore entry

- **WHEN** Python 侧调用 `new_java_instance(class_name)` 创建 Java shim 实例
- **THEN** runtime SHALL 同时：
  - 在 Python 侧实例化对应的 shim class
  - 在 `ObjectStore` 中插入一条 `HostValue { data: Box<Py<PyAny>> }` 记录
  - 通过 `RefTable::new_global(object_id)` 分配 handle
- **AND** 返回的 handle 是 RefTable 管理的 JNI handle，不是自增计数器

#### Scenario: release_java_instance cleans up ObjectStore and RefTable

- **WHEN** Python 侧调用 `release_java_instance(handle)`
- **THEN** runtime SHALL：
  - 从 `RefTable` 解析 handle → `ObjectId`
  - 从 `ObjectStore` 移除对应的 `ObjectId` 记录（从而 drop Python 对象引用）
  - 从 `RefTable` 删除该 handle

#### Scenario: ObjectStore is the canonical object authority

- **WHEN** runtime 需要获取 Python Java 实例
- **THEN** SHALL 通过 `RefTable::resolve(handle)` → `ObjectId` → `ObjectStore::get(ObjectId)` 查找
- **AND** 不 SHALL 再维护独立的 `java_instances: HashMap<u32, Py<PyAny>>` 作为对象 identity authority

### Requirement: Unified dispatch through Rust registry

`call_java_method` SHALL 统一走 Rust registry dispatch 路径，
不再做 Python→Python 直接调用和 Rust dispatch 的两路分支。

#### Scenario: Python override dispatch flows through JniRegistry

- **WHEN** Python shim 调用 `call_java_method(handle, sig, args)`
- **THEN** dispatch 路径 SHALL 为：
  1. `RefTable::resolve(handle)` → `ObjectId`
  2. `ObjectStore::get(ObjectId)` → `(class_name, HostValue { data: Py<PyAny> })`
  3. `JniRegistry::dispatch_call(sig, jni_args, refs)` → handler
  4. handler 从 `ObjectStore` 取出 Python 对象 → `Python::with_gil` 调用
- **AND** 不 SHALL 通过 `method_names` adapter cache 做两路分支

#### Scenario: Framework stub dispatch unchanged

- **WHEN** method 只有 Rust framework stub 实现（无 Python override）
- **THEN** `JniRegistry::dispatch_call` SHALL 直接执行 Rust-native handler
- **AND** 不 SHALL 因为统一 dispatch 而改变 framework stub 行为

### Requirement: Adapter caches are removed from PyEmulatorBridge main state

`class_types` / `method_names` / `java_instances` SHALL 从 `PyEmulatorBridge` 主状态中移除。

#### Scenario: Adapter caches moved to PythonShimAdapter

- **WHEN** Python bridge 需要维护 class 类型引用或方法名映射
- **THEN** 这些缓存 SHALL 存储在 `PythonShimAdapter` 结构内
- **AND** `PythonShimAdapter` 的字段 SHALL 明确标注为 adapter-private implementation detail
- **AND** `PyEmulatorBridge` 主字段 SHALL NOT 包含 `java_instances` / `class_types` / `method_names`

### Requirement: PyEmulator renamed to PyEmulatorBridge

Rust 类型 `PyEmulator` SHALL 重命名为 `PyEmulatorBridge`。

#### Scenario: Python API unchanged

- **WHEN** Python 代码导入 `from rundroid._rundroid import Emulator`
- **THEN** 导入的名称 SHALL 仍为 `Emulator`（`#[pyclass(name = "Emulator")]` 保持不变）

#### Scenario: Rust internals use new name

- **WHEN** Rust 代码引用该类型
- **THEN** SHALL 使用 `PyEmulatorBridge` 名称
