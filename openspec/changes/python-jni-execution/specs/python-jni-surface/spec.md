## ADDED Requirements

### Requirement: Python Emulator exposes JNI execution surface

Python 绑定层 SHALL 暴露一组明确、可测试的 JNI guest-execution API，使 Python 测试和脚本可以驱动依赖 `JNIEnv` / `JavaVM` 函数表的 native `.so`。

#### Scenario: Python initializes JNI ABI and hook installation

- **WHEN** Python 调用 `Emulator.init_jni()`
- **THEN** 绑定层 SHALL 在 guest 内存中映射 `JNIEnv` 与 `JavaVM` ABI 表
- **AND** SHALL 安装 JNI trampoline hook
- **AND** SHALL 缓存可复用的 `JNIEnv*` 与 `JavaVM*` guest 指针

#### Scenario: Python retrieves guest JNI pointers

- **WHEN** Python 在 `init_jni()` 后调用 `jni_env_pointer()` 或 `java_vm_pointer()`
- **THEN** 绑定层 SHALL 返回先前映射到 guest 内存中的稳定指针值
- **AND** 这些指针 SHALL 可直接作为 native 导出函数的 `JNIEnv*` / `JavaVM*` 实参

#### Scenario: Python drives JNI_OnLoad

- **WHEN** Python 调用 `jni_onload()`
- **THEN** 绑定层 SHALL 调用已装载模块中的 `JNI_OnLoad(JavaVM*, void*)`
- **AND** SHALL 把绑定层维护的 `JavaVM*` 传给 guest
- **AND** SHALL 校验返回的 JNI version 合法

#### Scenario: Python inspects guest memory for JNI test assertions

- **WHEN** Python 调用 `read_guest(addr, len)`
- **THEN** 绑定层 SHALL 返回对应 guest 地址范围的字节内容
- **AND** 该能力 SHALL 可用于测试断言 native/JNI 调用对缓冲区的回写结果

### Requirement: Python Emulator exposes observable JNI verbose mode

Python 绑定层 SHALL 提供可切换的 JNI verbose 模式，至少让测试能观察到关键 JNI slot 的调用事实。

#### Scenario: Verbose mode surfaces key JNI slot names

- **WHEN** Python 调用 `set_jni_verbose(true)` 并驱动一个 JNI guest 调用链
- **THEN** 输出中 SHALL 可观察到关键 slot 名称
- **AND** 至少 `FindClass`、`GetMethodID`、`CallIntMethod`、`RegisterNatives` 这些调用在被触发时 SHALL 可被观测到
- **AND** 关闭 verbose 后，绑定层 SHALL 不再输出同等级别的 JNI trace
