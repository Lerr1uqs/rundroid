## Why

真正的 Android native 目标最终一定会撞到：

- `RegisterNatives`
- `Java_*` 符号查找
- `JNI_OnLoad`

这些能力虽然和 `jni-shim-foundation` 相关，但它们是单独的一条 native lifecycle 主线，应该独立成 change。

## What Changes

这个 change 定义 native JNI lifecycle。

本次变更引入：

- 新 capability：`native-jni-lifecycle`
- `RegisterNatives` registry
- `Java_*` dynamic lookup fallback
- `JNI_OnLoad` 生命周期
- JNI version validation

本次变更不要求：

- 完整 framework stub
- Python decorator 细节

## Capabilities

- native-jni-lifecycle
- testing-harness
