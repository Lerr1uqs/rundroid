## ADDED Requirements

### Requirement: JNI trampoline execution is a reusable primitive

JNI trampoline code-hook + `dispatch_jni_call` SHALL 位于一个共享 crate
（`rundroid-jni-trampoline`），被 case-runner 和 Python 绑定层共同消费，不复制。

#### Scenario: No duplicated dispatch

- **WHEN** case-runner 或 Python 绑定层需要 guest JNI dispatch
- **THEN** 两者 SHALL 消费同一 `rundroid-jni-trampoline` crate
- **AND** `dispatch_jni_call` SHALL 只存在于一处

### Requirement: JNI dispatch is observable via verbose trace

runtime SHALL 支持在 guest JNI dispatch 时打印 unidbg 式 verbose trace，便于逆向调试与测试断言。

#### Scenario: Verbose prints each JNI call

- **WHEN** verbose 开启且 guest 调一个 JNI 函数
- **THEN** SHALL 打印一行，含 slot 名（如 `FindClass`）与关键参数（如 class 名）
- **AND** 调用完成后 SHALL 打印返回值
- **AND** 格式 SHALL 对齐 unidbg 风格（如 `[I] JNIEnv->FindClass(name="com/scene/Signer")`）

#### Scenario: Verbose is toggleable after hook install

- **WHEN** hook 已安装后再 toggle verbose
- **THEN** 后续 dispatch SHALL 反映最新开关状态
- **AND** 默认 SHALL 为关闭
