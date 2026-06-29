## Why

RegisterNatives 是 `JNI_OnLoad` 的标准动作——native so 通过它把 C 函数指针绑定到 Java 方法上。rundroid 的 `JniEnvSurface.register_natives` 已经能正确接收绑定信息、存入 `NativeRegistry`（通过 `MethodId → GuestPtr` 映射）。

但 RegisterNatives 绑完之后的**调用**有缺口——`jnienv.rs:382` 和 `:523` 直接返回错误：

> "method 已通过 RegisterNatives 绑定 guest native ({:#x})，但 guest native 调用链尚未接入"

当 framework/Python 通过 `CallXxxMethod` 命中一个已绑定的 native 方法时，rundroid 必须"反向进 guest 执行那段 native 函数指针"——当前不通。

rundroid 已有 `GuestRuntime.call_export`（sentinel trick：映射一页放 `RET`，`LR=sentinel`，`emu_start` 跑到哨兵停）能调 guest 导出函数。所以**机制底座已经有了**，缺的是把 RegisterNatives 的 `natives_map` + `Java_*` 符号查找 + 参数打包接到 JNI 调用主线。

unidbg 的完整链路已验证：
1. `DvmObject.callJniMethod(emulator, "methodName", args)` → `objectType.findNativeFunction(emulator, "methodName")` **先查 nativesMap**（RegisterNatives 绑定的），找不到再构造 `Java_com_example_Class_method` 符号名查所有 loadedModules 符号表
2. 参数打包：`JNIEnv指针` + `jobject(obj.hashCode)` + `user args`
3. `Module.emulateFunction(emulator, fnPtr, args)` → 内部 x0..x7 放参数 → LR=sentinel → `emu_start(fnPtr, until=sentinel)` → 读 x0 返回
4. **嵌套支持**：native 里调 JNI（guest→host） → trampoline callback → 再调 `callJniMethod` → 再 `emu_start(fn)` → Unicorn 原生支持嵌套 `emu_start`

本 change 补齐这个缺口。

## What Changes

本次变更引入：

- 新 capability：`jni-native-callback`
- `call_export` 泛化为 `call_guest_function`（从 GuestRuntime 的方法抽象为 trait，供 JNI 层调用）
- `findNativeFunction`：`NativeRegistry` / `JniEnvSurface` 上新增 `find_native_guest_fn(class, method, sig) → Option<u64>`，先查 RegisterNatives 表，再查 `Java_*` mangled 符号
- JNI dispatch 主线（`dispatch_by_method_id` / `dispatch_static_by_method_id`）补 native 分支：method 有 native binding 时不再报错，改为包装参数 → 调 `call_guest_function` → 读返回值
- 返回值分发：按 `JType` 把 x0 映射回 `JValue`（Int/Long/Object/Void 等）
- 嵌套 `emu_start` 支持：Unicorn 原生支持，不需额外工程

本次变更不要求：

- 修改 `JniEnvSurface` 的类/对象/引用管理核心逻辑
- 增加新的 capability 或修改现有 capability 的 spec
- Python stub 层改动
- 所有 JNI 返回类型全覆盖（从 Int/Long/Object/Void 开始）

## Capabilities

- jni-native-callback
- testing-harness
