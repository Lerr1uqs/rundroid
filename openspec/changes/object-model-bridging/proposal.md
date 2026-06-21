## Why

`python-javashim-overrides` 已把 class/method/field 权威链路接入 `AndroidRuntime`，但 Python 侧 Java 对象的实例生命周期（创建、handle 分配、方法分派）仍然运行在 `PyEmulator` 的三个 adapter HashMap 上，完全没有进入 Rust `ObjectStore` + `RefTable` 体系。这导致两套对象模型并存——Rust dispatch 拿到 `ObjectId` 时找不到 Python backing object，Python 分派拿到 handle 时也查不到 Rust canonical state。

现在接入是因为：class 层已经收敛，对象层不接上的话，每次 `call_java_method` 都要在 Python direct path 和 Rust dispatch path 之间做分支判断，优先级逻辑散落在两处，后续再加 JNI function table 会更难统一。

## What Changes

- **BREAKING**: `PyEmulator` 重命名为 `PyEmulatorBridge`
- Python Java 对象实例化接入 `ObjectStore`（`HostValue` 变体持有 `Box<Py<PyAny>>`）
- handle 分配改用 `RefTable::new_global(obj_id)`，不再用自增 `u32` 计数器
- `call_java_method` 统一走 `runtime.refs().resolve(handle)` → `ObjectStore.get(ObjectId)` → `runtime.classes().dispatch_call()` 路径，Python override 和 framework stub 不再分叉
- `class_types` / `method_names` / `java_instances` 三个 adapter HashMap 从 `PyEmulatorBridge` 主状态中移除，下沉到 `PythonShimAdapter` 结构
- `MethodImpl::PythonShim(u64)` 变体继续保留不启用，Python handler 仍以 `RustNative` 闭包捕获 `Py<PyAny>` 的方式注册

## Capabilities

### New Capabilities
- `object-model-bridging`: Python Java 对象实例生命周期接入 Rust ObjectStore + RefTable，统一 dispatch 链路

### Modified Capabilities
- `python-javashim`: dispatch 路径从"先查 method_names 再 fallback Rust registry"改为统一走 Rust dispatch；adapter caches 下沉到专用结构，不作为 authority

## Impact

- `emulator/bindings/python/src/lib.rs` — `PyEmulator` → `PyEmulatorBridge`，移除 `class_types`/`method_names`/`java_instances`，新增 `PythonShimAdapter`，重写 `new_java_instance`/`call_java_method`/`release_java_instance`
- `emulator/bindings/python/src/javashim.rs` — 新增 instance 与 ObjectStore 同步逻辑
- `python/rundroid/__init__.py` — 导入名称不变（`Emulator` 仍对外暴露为 `Emulator`）
- `python/tests/` — 测试适配（如果有直接引用 `PyEmulator` 的地方）
