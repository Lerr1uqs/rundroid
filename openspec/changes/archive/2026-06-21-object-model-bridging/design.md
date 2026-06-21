## Context

`python-javashim-overrides` 完成后，`PyEmulator` 已经持有 `runtime: AndroidRuntime` 作为 class/method/field 的 canonical authority。但对象实例生命周期仍走三条独立的 adapter path：

```
new_java_instance → next_instance_handle++ → java_instances.insert(handle, PyObject)
call_java_method  → java_instances.get(handle) → 直接 Python→Python 调用
release_java_instance → java_instances.remove(handle)
```

Rust 侧的 `ObjectStore`（`HashMap<ObjectId, (class_name, ObjectStorage)>`）和 `RefTable`（`HashMap<u32, (ObjectId, RefKind)>`）完全没有 Python 对象的数据。

另外 `PyEmulator` 这个名字不够精确——它不是 Emulator（Emulator 是 `emulator/core` 里的 Rust struct），它是 Python 到 Rust 的桥接层。

## Goals / Non-Goals

**Goals:**
- Python Java 对象实例创建时同步写 `ObjectStore::HostValue`
- handle 分配改用 `RefTable::new_global()`，handle↔ObjectId 映射由 RefTable 持有
- `call_java_method` 统一走 `RefTable::resolve()` → `ObjectStore` → `JniRegistry::dispatch_call()`，不再分叉
- `class_types` / `method_names` / `java_instances` 从 `PyEmulatorBridge` 移除，下沉到 `PythonShimAdapter`
- `PyEmulator` 重命名为 `PyEmulatorBridge`

**Non-Goals:**
- 不实现 JNI function table（GetMethodID / CallVoidMethod / etc.）
- 不启用 `MethodImpl::PythonShim(u64)` 变体
- 不做完整 JNIEnv / JavaVM surface
- 不改变 `@java_class` / `@java_method` decorator 的 Python API
- `register_framework_stub` 保持现有签名不做 breaking change

## Decisions

### Decision 1: 用 `HostValue` 持有 Python 对象

`ObjectStorage::HostValue { data: Box<dyn Any + Send + Sync> }` 的 `data` 字段存储 `Py<PyAny>`（Python 对象引用）。这在 `android-vm-state-model` 时已经把 bound 从 `Send` 改为 `Send + Sync`，可以直接用。

**Alternative considered**: 在 `ObjectStorage` 新增 `PythonInstance` 变体。结论：`HostValue` 已经为此设计，不需要新变体。

### Decision 2: handle 分配改走 RefTable

当前自增 `next_instance_handle: u32` → 改为 `runtime.refs_mut().new_global(object_id)`。

`new_global` 返回的 handle 是 `u32`，和现有 `call_java_method(handle: u32, ...)` API 签名兼容。

**Alternative considered**: 直接暴露 `ObjectId` 给 Python 侧。结论：JNI 约定中 guest 可见的是 handle 不是 ObjectId，保持 handle 对外暴露、ObjectId 内部使用。

### Decision 3: PythonShimAdapter 结构

```rust
/// Python shim 到 Rust runtime 的 adapter。
///
/// 持有 Python 侧实例化、分派所需的缓存映射，
/// 但不是 class/method/object 的 authority。
struct PythonShimAdapter {
    /// class_name → PyType，供 new_java_instance 实例化
    class_types: HashMap<String, Py<PyType>>,
    /// (class_name, java_method_name) → python_method_name
    method_names: HashMap<(String, String), String>,
}
```

`java_instances` 完全移除——Python 对象从 `ObjectStore` 的 `HostValue` 取出。

### Decision 4: 统一 dispatch 链路

`call_java_method(handle, sig, args)` 改为：

```
1. object_id = runtime.refs().resolve(handle)?  → ObjectId
2. (class_name, storage) = runtime.vm.objects.get(object_id)?
3. 从 HostValue 取出 Py<PyAny>
4. 用 PythonShimAdapter.method_names 查到 python_method_name
5. Python::with_gil → 调用 Python 方法 → 返回值校验
```

框架 stub 回退路径：
```
3'. 如果不是 HostValue（是 Rust StubInstance）
4'. runtime.classes().dispatch_call(sig, jni_args, refs)
```

这样一来 Python override 和 framework stub 都在同一条 `dispatch_call` 路径上（因为 Python override 的 method handler 是在 register_java_class 时通过 `wrap_python_method` 注册到 registry 的），优先级由 `register_or_merge_class` 的 merge 语义自然保证。

### Decision 5: 重命名范围

- Rust: `PyEmulator` → `PyEmulatorBridge`，struct 名、所有内部引用更新
- Python: `_rundroid.Emulator` 保持不变（`#[pyclass(name = "Emulator")]` 不变），Python 用户代码无需改动
- 文件 `lib.rs` 不变名，只改内部 struct 名

## Risks / Trade-offs

- **[Risk] `RefTable::new_global` 的 handle 分配语义跟当前自增计数器不同** → new_global 内部也是自增，兼容。但 RefTable 的 `clear_frame` 会清除 local ref，需要确保 global ref 不受影响。
- **[Risk] Python 对象存在 `HostValue` 里，Rust 侧可以 Drop 它** → `ObjectStore::remove()` 会 drop `ObjectStorage`，其中的 `HostValue` 也会 drop。Python 侧如果还持有引用，可能导致 use-after-free。Mitigation: `release_java_instance` 改为同时调用 `ObjectStore::remove()` 和 `RefTable::delete()`，Python 侧只需调一个 API。
- **[Trade-off] Python→Python 调用仍需 GIL** → `wrap_python_method` 闭包内的 `Python::with_gil` 在 Rust dispatch 链中获取 GIL，没有性能回归，但 dispatch 链比直接 Python 调用多一层间接。

## Migration Plan

1. 重命名 `PyEmulator` → `PyEmulatorBridge`（纯内部，Python API 不变）
2. 新增 `PythonShimAdapter`，把 `class_types` + `method_names` 移入
3. 修改 `new_java_instance`：创建 Python 对象 → `ObjectStore::insert(HostValue)` → `RefTable::new_global(object_id)`
4. 修改 `call_java_method`：统一走 Rust dispatch
5. 修改 `release_java_instance`：`ObjectStore::remove()` + `RefTable::delete()`
6. 移除 `java_instances` HashMap
7. 更新所有内部引用 + 测试
