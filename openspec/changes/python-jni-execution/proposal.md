## Why

当前 Python 绑定层（`PyEmulatorBridge`）**无法驱动使用 JNI 的 native .so**。它能 `emu.call(纯导出符号)`（libsmoke 的算术 / syscall），但 guest 一旦走 `(*env)->FindClass(...)` 这类 JNI 函数表回调就会崩——因为绑定层没映射 JNIEnv / JavaVM ABI 表、没装 trampoline code hook、没有 `init_jni` / `jni_env_pointer` / `jni_onload`。全套 JNI guest 执行能力只活在 case-runner 这个 Rust-only 装配层。

要让 Python 脚本能像 unidbg 一样加载真实 NDK 编译的 .so、注册 Java shim class、让 guest 经 JNI 函数表回调进 Python 注册的类，并打出 unidbg 式调用 trace，必须把 JNI 执行能力桥到 Python 绑定层。

桥接暴露一个现实约束：`JniTrampolineHook` 钳死要 `Arc<Mutex<AndroidVM>>`（hook 在 `emu_start` 期间触发，必须捕获能跨 guard 存活的句柄）。要让绑定层和 hook 共享同一个 VM，VM 必须抬升成 `Arc<Mutex<AndroidVM>>`。`AndroidRuntime` 当前又是一个几乎零状态的转发壳，因此本 change 同时把 VM authority 收敛到 `AndroidVM`，避免绑定层和 hook 分别持有两套视图。

## What Changes

这个 change 让 Python 绑定层能驱动使用 JNI 函数表的 guest native 代码，并把 JNI guest execution surface 正式暴露给 Python。

本次变更引入：

- 把 `JniTrampolineHook` + `dispatch_jni_call` 抽到共享 crate `rundroid-jni-trampoline`，case-runner 与 Python 绑定层共同消费，不复制
- Python `Emulator` 正式暴露 JNI guest-execution 表面：`init_jni` / `jni_env_pointer` / `java_vm_pointer` / `jni_onload` / `read_guest` / `set_jni_verbose`
- JNI 调用 verbose trace（至少可观察到 slot 名与关键调用）
- `Emulator` 直接持有 `AndroidVM`（`Arc<Mutex<AndroidVM>>`），不再让包装层成为 VM authority 歧义源
- 两层测试闭环：一个最小 JNI fixture 验证 surface 打通；一个 richer `libscene.so` fixture 压测 JNI dispatch / 继承 / primitive 参数 marshalling / syscall / verbose 的交叉正确性

本次变更不要求：

- 新增 JNI ABI slot 覆盖（沿用现有已桥接 slot 集）
- 多线程 guest 仿真（单线程；Python override 在 guest dispatch 期间不得再入 VM）
- RELRO / TLS 等既有遗留边界的推进

## Capabilities

- android-vm-model
- python-jni-surface
- python-javashim
- jni-abi-surfaces
- testing-harness
