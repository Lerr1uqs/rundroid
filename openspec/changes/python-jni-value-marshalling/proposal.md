## Why

当前 JNI foundation 的对象、类型、方法签名模型已经成立，但 Python ↔ Rust 的值编组没有打通，导致 `str`、`bytes`、`Object` 在跨边界时被静默吞掉或退化成 `None`。这使得 `Signature([B)V`、`java/lang/String`、primitive array 以及 guest/native 回调的基础用例都无法自然成立，必须先补这层桥。

## What Changes

- 打通 Python 侧到 Rust 侧的值编组路径，让 `str`、`bytes`、`None`、`bool`、`int`、`float` 能稳定转换为 `JValue` / `JniArgs`。
- 打通 Rust 侧回 Python 的值编组路径，让 `JValue::Object`、`JValue::Null`、primitive 值、`java/lang/String`、primitive array 能回到合适的 Python 表示。
- 为 `java/lang/String` 与 `byte[]` 提供默认无痛 coercion，避免脚本层必须手工包装。
- 引入显式 wrapper 作为可选身份层，例如 `JavaString` / `JavaByteArray`，用于需要复用同一对象身份的场景。
- 补充端到端测试，覆盖 `Signature([B)V`、`String`、`byte[]`、对象回传与返回值校验。

## Capabilities

### New Capabilities

- `python-jni-value-marshalling`: Python 值与 JNI 值之间的自动编组与回编组
- `python-builtin-java-values`: Python 侧显式 wrapper，用于 `String` / `byte[]` 等有身份对象

### Modified Capabilities

无。

## Impact

- 受影响代码：
  - `emulator/bindings/python/src/javashim.rs`
  - `emulator/bindings/python/src/lib.rs`
  - `emulator/jni/src/object.rs`
  - `emulator/jni/src/jnienv.rs`
  - `emulator/case-runner/src/jni_hook.rs`
  - `python/rundroid/javashim` 相关 Python API
- 对外行为：
  - `str` / `bytes` 不再被静默吞成 `None`
  - `Object` 返回值不再统一退化为 `None`
  - `Signature([B)V` 等基础 JNI 用例将可直接跑通
- 需要新增回归测试来锁定 marshalling 语义，避免以后再回退到 null 占位。
