## 1. Python -> Rust 编组

- [x] 1.1 修改 `emulator/bindings/python/src/lib.rs` 中 `convert_pyargs_to_jniargs`，为 `None`、`bool`、`int`、`float`、`str`、`bytes` 建立明确编组规则
- [x] 1.2 修改 `emulator/bindings/python/src/javashim.rs` 中 `py_object_to_jvalue`，让 Python 返回值中的 `str`、`bytes` 不再吞成 `Null`
- [x] 1.3 为 `str` / `bytes` 的编组结果接入 `ObjectStore` 对应存储，而不是仅返回空占位

## 2. Rust -> Python 回编组

- [x] 2.1 修改 `emulator/bindings/python/src/javashim.rs` 中 `jvalue_to_py_object`，按 `ObjectStorage` 类型恢复 `str`、`bytes`、primitive 值与 `None`
- [x] 2.2 修改 `emulator/bindings/python/src/lib.rs` 中 Rust 回 Python 的调用路径，确保 `JValue::Object` 不再统一退化为 `None`
- [x] 2.3 补齐对象回传时的 storage-aware 分发逻辑，保留 identity-sensitive wrapper 的入口

## 3. 显式内置值对象

- [x] 3.1 在 Python 侧新增 `JavaString` / `JavaByteArray` 的显式 wrapper API，保持与现有 `JavaClass` / `JavaObject` 风格一致
- [x] 3.2 在 Rust 侧为 wrapper 提供对应构造/识别路径，复用 `ObjectStorage::String`、`ObjectStorage::PrimitiveArray` 与 `ObjectId`

## 4. 回归测试

- [x] 4.1 新增 `Signature([B)V` 的端到端测试，验证 Python `bytes` 进出都不是 `None`
- [x] 4.2 新增 `java/lang/String` 的端到端测试，验证 Python `str` 进出都不是 `None`
- [x] 4.3 新增 unsupported 值 fail-fast 测试，确保不会再静默吞成 `Null`

## 5. 验证

- [x] 5.1 在 `python/` 项目上下文运行相关 pytest 用例
- [x] 5.2 运行 `openspec validate --type change python-jni-value-marshalling --strict`
