## Why

`rundroid` 现在已经有了 ELF、loader、linker、Linux runtime、driver 和 Python 绑定的最小骨架，但 Android Native crackme 真正往前推进时，很快就会撞到下一个硬门槛：JNI。

如果 JNI 这一层继续沿用 `unidbg` 里常见的“按签名字串做中心化分派”思路，后续会立刻出现几个问题：

- 新增一个 Java class / method 往往要编辑中心注册或分派逻辑
- Python 脚本层虽然易写，但没有稳定的类型契约，method descriptor 和 Python 注解很容易漂移
- `JNIEnv` / `JavaVM` / `JNI_OnLoad` 的边界不清楚，后面容易把完整 Java VM、Android framework stub、对象模型混成一个大杂糅层
- 没有正式 spec 的情况下，不同 agent 很容易对“这一阶段的 JNI 到底要做到什么”产生分歧

当前阶段最需要的不是完整 Java VM，而是先建立一个可以长期稳定扩展的 JNI / shim foundation：

- Rust 持有 JNI 核心状态与分派权威
- Python 只负责声明 class/method/field shim
- 注册阶段先做 descriptor 和注解校验
- 调用阶段统一走 registry / dispatch，而不是 switch-case

## What Changes

这个 change 用来定义 `rundroid` 的第一版 JNI / shim foundation。

本次变更引入：

- 新 capability：`jni-shim`
- 顶层 `runtime/` 目录重构为 `emulator/` 的架构约束
- 对外主入口从 `Runtime` 收敛为 `Emulator` 的架构约束
- `emulator/jni` 与 Python shim 包的正式骨架要求
- Rust 侧 `emulator/jni` crate 的稳定边界
- `JType` / `JValue` / `MethodSig` / `FieldSig` 的 canonical model
- descriptor parser、registry、dispatch、reference table 的基础语义
- `JNIEnv` / `JavaVM` 的最小 surface
- Python shim decorator 与 registration metadata 规则
- Python 注解和 Java descriptor 的严格校验要求
- 一组最小 JNI regression / harness case，用于验证注册失败、成功 dispatch 和 `JNI_OnLoad` 主线

本次变更不要求：

- 完整 Java VM
- Dalvik/ART 字节码解释执行
- 全量 Android framework class stub
- 反射、classloader、异常系统的完整兼容
- ARM32/Thumb JNI 调用约定
- 自动生成所有 `JNIEnv` 函数表条目

## Capabilities

这个 change 会新增或定义：

- jni-shim
- testing-harness

## Impact

实现方完成本 change 后，后续新增一个 Java shim class 或 method，不应再需要编辑中心化 switch-case 分派代码。

review 阶段应优先看：

- descriptor / 注解是否在注册时严格校验
- emulator 是否由 Rust registry 持有 JNI 权威状态
- 对外装配层是否以 emulator 为中心，而不是继续暴露 `Runtime` 作为稳定用户入口
- 顶层 crate 目录是否已从 `runtime/` 收敛到 `emulator/`
- Python decorator 是否只声明 metadata，而不是 import 即污染全局
- `JNIEnv` / `JavaVM` / `JNI_OnLoad` 的当前阶段边界是否足够清晰
