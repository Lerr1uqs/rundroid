## Why

`jni-shim-foundation` 只规定了最小 `JNIEnvSurface` / `JavaVMSurface` 概念，但还没有把 guest 真实可见的 `JavaVM*` / `JNIEnv*` ABI 表面单独收敛出来。

unidbg 的关键价值之一，就是它不是纯 host-side mock，而是真的在 guest 里造出 JNI function table，再让 native so 按 ABI 调进来。

如果 Rust 重构失去这一点，很多 JNI 样本会变得不可调试，也无法和 unidbg 行为对齐。

## What Changes

这个 change 定义 JNI ABI surfaces。

本次变更引入：

- 新 capability：`jni-abi-surfaces`
- guest-side `_JavaVM` / `_JNIEnv` pointer model
- `_JNIInvokeInterface` 与 `JNIEnv function table` 的槽位语义
- ABI slot -> Rust handler 的映射模型
- `GetEnv` / `AttachCurrentThread` / `FindClass` / `GetMethodID` / `Call*Method` 等最小主线

本次变更不要求：

- framework stub 具体实现
- Python shim 注册
- 所有 JNI entry 一次覆盖完成

## Capabilities

- jni-abi-surfaces
- testing-harness
