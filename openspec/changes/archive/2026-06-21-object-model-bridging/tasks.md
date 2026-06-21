## 1. 重命名

- [x] 1.1 `PyEmulator` → `PyEmulatorBridge`（struct 名 + `lib.rs` 内所有引用 + `javashim.rs` 注释），`#[pyclass(name = "Emulator")]` 保持不变

## 2. PythonShimAdapter

- [x] 2.1 新增 `PythonShimAdapter` struct，存放 `class_types: HashMap<String, Py<PyType>>` 和 `method_names: HashMap<(String, String), String>`
- [x] 2.2 `PythonShimAdapter` 上提供 `insert_class` / `insert_method_name` / `resolve_class_type` / `resolve_method_name` 方法
- [x] 2.3 `PyEmulatorBridge` 上新增 `shim: PythonShimAdapter` 字段，移除直接的 `class_types` / `method_names` / `java_instances` 字段

## 3. Object Model Bridging

- [x] 3.1 修改 `new_java_instance`：创建 Python 对象 → `ObjectStore::insert(HostValue { data: Box::new(py_obj) })` → `RefTable::new_global(object_id)` → 返回 handle
- [x] 3.2 修改 `call_java_method`：`RefTable::resolve(handle)` → `ObjectStore::get(ObjectId)` → 从 `HostValue` 取出 `Py<PyAny>` → 查 `PythonShimAdapter::resolve_method_name` → `Python::with_gil` 调用 → 返回值校验
- [x] 3.3 framework stub 回退：当 `ObjectStore` 中的 storage 不是 `HostValue`（是 `StubInstance`）时，走 `runtime.classes().dispatch_call()`；且在 HostValue 路径中 method_names 未命中时也回落 Rust dispatch
- [x] 3.4 修改 `release_java_instance`：`RefTable::resolve(handle)` → `ObjectStore::remove(ObjectId)` + `RefTable::delete(handle)`
- [x] 3.5 更新 `register_java_class`：用 `PythonShimAdapter::insert_class` / `insert_method_name` 替代直接操作 HashMap

## 4. 清理与测试

- [x] 4.1 更新 `register_framework_stub` 兼容新 dispatch 路径（改用 `register_or_merge_class`）
- [x] 4.2 运行 `cargo test -p rundroid-jni`，确保 JNI crate 测试全通过
- [x] 4.3 运行 `python/tests/test_javashim.py` 全部测试，确保 override 和 bad-annotation case 仍通过
- [x] 4.4 运行 `cargo test --workspace` 确保全 workspace 测试通过
- [x] 4.5 运行 `openspec validate --type change object-model-bridging --strict`
