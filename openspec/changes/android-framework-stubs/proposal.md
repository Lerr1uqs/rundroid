## Why

当前 `jni-shim-foundation` 只规定了 shim 机制，没有规定 Android framework 行为到底怎么建。

如果继续沿用 unidbg `AbstractJni` 那种 giant switch，Rust 重构很快又会掉回同样的维护陷阱。

需要单独一条 change，把 framework stub 收敛成 class-oriented registry。

## What Changes

这个 change 定义 Android framework stubs。

本次变更引入：

- 新 capability：`android-framework-stubs`
- class-spec 驱动的 framework registry
- service registry
- APK-backed `PackageManager` / `PackageInfo` / `Signature` / `AssetManager` / `Bundle` 等最小 stub 集

本次变更不要求：

- Python override
- JNI ABI table 全量覆盖
- 所有 Android framework class

## Capabilities

- android-framework-stubs
- testing-harness
