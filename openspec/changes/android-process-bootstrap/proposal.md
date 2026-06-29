## Why

当前 `call_export` 使用最小 sentinel trick（映射一页放 `ret` 指令作 LR 哨兵，`emu_start` 跑到哨兵就读 x0），只能跑"纯导出调用"。真实 Android .so 的 constructor 和 JNI_OnLoad 需要完整的 guest 进程初始状态：

- **auxv 向量**：至少 AT_RANDOM(25) 用于 libc 栈保护 canary、AT_PAGESZ(6) 用于 mmap 对齐。getauxval 是 bionic libc 的标准行为，当前不存在 auxv 栈布局
- **main thread TLS**：TPIDR_EL0 指向线程 TLS 块（含 pthread_internal_t、errno 指针），libc 内部方法（如 `__errno`）依赖此结构。当前 PT_TLS 提取返回 None，TPIDR_EL0 未设置
- **栈布局**：argv/envp/auxv 指针排布。guest libc 的 `__libc_init` 和 JNI_OnLoad 框架层会遍历 auxv
- **constructor 执行**：`.init` / `.init_array` 在 relocation 之后、JNI_OnLoad 之前必须执行。当前 `InitPlan` 数据已提取、`init_order` 拓扑排序已计算，但没有任何代码实际调用 constructor

现状已暴露出 gap：case 2/3 的 .so 带有 constructor 依赖（如 `__cxa_finalize` `__stack_chk_fail`），没有初始进程状态时这些 constructor 会失败。同时，`android-os-rebrand` change 已经把 OS 层从 linux 重命名为 android，但`Kernel`（原 `LinuxRuntime`）不承担进程初始化职责。

unidbg 分析结论：AndroidElfLoader 没有 Zygote 概念，但分散在 loader 构造函数 + `initializeTLS()` 中的初始化逻辑可以被集中到一个 dedicated struct 中。unidbg 仅做 2 个 AT_* 值 + 手构造 TLS + 零参 constructor 就足以运行绝大多数真实 Android .so。

## What Changes

- 新增 `Zygote` struct（命名暂定，在 design 中作为 Open Question 列出备选）：承载 guest 进程初始化的全部流程
- auxv 构造：至少 AT_RANDOM(25) → 16B 随机数、AT_PAGESZ(6) → 0x1000，按 auxv 规范在栈上排布（{type, value} pair + null terminator）
- main thread TLS：在栈上手分配 ~512B TLS 块，填充 pthread_internal_t 指针（errno=0, tid=main），写入 TPIDR_EL0
- stack 布局：STACK_BASE 确定 + argv/envp/auxv 指针排布在栈顶
- constructor 执行调度：relocation 后按 `init_order` 逐个模块执行 `init_plan`（零参数调用），保证执行序 = 链接拓扑序
- `call_export` 能力增强：不再只设 SP/LR/PC/x0-x7 就执行，先经 Zygote 完成进程初始化
- integration：Zygote 在 case-runner assembly 阶段或 Kernel 构造时被实例化

## Capabilities

### New Capabilities

- `android-process-bootstrap`: 定义 Zygote/进程启动组件，覆盖 auxv 构造、main thread TLS 建立、栈布局 argv/envp/auxv、constructor 执行调度。

### Modified Capabilities

- none（新能力，但与现有的 `linux-layering` / `elf-runtime-interfaces` 的行为边界相关：constructor 排序和 TLS 地址控制）

## Impact

- **新模块/文件**：`emulator/os/android/src/zygote.rs`（与 Kernel 同级），承载 Zygote struct 与进程初始化逻辑
- **`emulator/case-runner/src/runtime.rs`**：改造 `load_and_link`，在 relocation 完成后、JNI_OnLoad 之前通过 Zygote 执行 constructor；`call_export` 不再独立管理初始状态
- **`emulator/elf/loader/src/model.rs`**：可能扩展 `InitPlan` 接口以适配 constructor 聚合调度（目前 `init_array` 存指针 slot 地址，constructor 执行时需要先读指针值）
- **`emulator/elf/linker/src/init.rs`**：`init_order` 的计算方式可能需要调整（当前用 Kahn 拓扑排序，可能已足够；验证后确认）
- **`emulator/backends/api/src/engine.rs`**：可能需要为 Arm64Reg 枚举增加 `TpidrEl0` 或系统寄存器写入能力（若当前不包含）
- **`emulator/memory/src/layout.rs`**：`TlsLayout` 当前只是预留地址范围，Zygote 需要实际分配和填充
- **测试**：
  - 新增 case 覆盖 constructor 执行 + auxv/TLS 初始状态 + JNI_OnLoad 在其后执行
  - 扩展 case 1 (smoke) 验证构造函数不破坏纯导出调用
  - 验证 constructor 内对 auxv/TLS 的依赖正确可用
