## ADDED Requirements

### Requirement: Minimal Python-driven JNI execution fixture

SHALL 提供一个最小但完整的 JNI fixture，用于验证 Python 绑定层已经真正打通 JNI ABI 表、trampoline hook、`JNI_OnLoad` 与基本 method dispatch。

#### Scenario: minimal fixture validates JNI execution surface end to end

- **WHEN** 一个最小 JNI fixture 从 Python 被加载、初始化并调用
- **THEN** 测试 SHALL 经 `init_jni()`、`jni_env_pointer()` 或 `java_vm_pointer()`、`jni_onload()` 这些 surface 驱动 guest 执行
- **AND** SHALL 至少覆盖 `JNI_OnLoad`、`FindClass`、`GetMethodID`、`NewObject`、`CallIntMethod`
- **AND** SHALL 断言一个确定性返回值或缓冲区结果

### Requirement: Rich native-.so Python-driven integration scene

SHALL 提供一个场景丰富的 NDK 编译 fixture + Python 驱动的端到端测试，模拟真实 Android
逆向逻辑，用于压测 JNI dispatch / 继承 / primitive 参数 marshalling / syscall / verbose 的交叉正确性。

#### Scenario: fixture exercises cross-class JNI + syscalls + algorithm

- **WHEN** fixture `libscene.so` 从 Python 加载并执行
- **THEN** SHALL 覆盖：`JNI_OnLoad` + `RegisterNatives`、跨多 class 的 `FindClass` / `GetMethodID` / `Call*Method`（static + instance + 继承 + 交叉依赖）、syscall（`openat` / `read` / `getrandom` / `mmap`）、一个 checksum / hash 算法
- **AND** 一个 Python 测试 SHALL 经 Emulator JNI-execution surface 驱动它并断言确定性结果
- **AND** 测试 SHALL 观察到关键 JNI 调用的 verbose trace
