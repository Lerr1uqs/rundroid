## Context

当前 `rundroid` 的 guest 进程初始状态由 `call_export` 随意构建：

```text
call_export 中的进程初始化：
  SP = STACK_BASE + STACK_SIZE
  LR = SENTINEL_ADDR（放 ret 指令）
  PC = entry
  x0..x7 = 调用参数
```

没有 argv/envp/auxv 栈布局、没有 TLS、没有 TPIDR_EL0、没有 constructor 执行。这在"纯导出调用"（如 `rd_add(1,2)`）时可以工作，但一旦导入的 .so 依赖构造函数或 libc 初始化，就会以各种方式失败。

unidbg 调研确认：

| 初始化项 | unidbg 做法 | rundroid 现状 |
|---|---|---|
| auxv | 仅 AT_RANDOM + AT_PAGESZ，硬编码于 `AndroidElfLoader` 构造函数 | 不存在 |
| TLS | 手分配 512B 栈空间，填 pthread_internal_t 指针+errno 地址，TPIDR_EL0 设值 | 不存在，PT_TLS 提取返回 None |
| stack | STACK_BASE=0xe5000000，argv/envp/auxv 手工排布 | 无布局，仅有 SP 设值 |
| constructor | 零参数调用，执行序=模块加载序（非拓扑序），在 relocation 后、JNI_OnLoad 前 | InitPlan 已提取、init_order 已计算，但无执行代码 |
| getauxval syscall | 不存在（auxv 只给 libc 自扫描栈读） | 不存在 |
| environ 导出 | dlsym("environ") → argv[2] 地址 | 不存在 |

unidbg 证明：仅 2 个 AT_* 值 + 手构造 TLS + 零参 constructor 就足够运行绝大多数真实 Android .so。这说明进程初始化的"最小有用子集"很小，适合在 bootstrap 阶段集中实现。

同时，`android-os-rebrand` change 已经把 OS 层从 `linux` 更名为 `android`（`LinuxRuntime` → `Kernel`），但 Kernel 不承担进程初始化职责。`Zygote` 将是与 Kernel 平级的模块，共同构成 android OS crate 的根。

## Goals / Non-Goals

**Goals:**

- 定义 `Zygote` struct 作为 guest 进程初始化的唯一入口和集中承载
- auxv 构造：AT_RANDOM(25) + AT_PAGESZ(6)，按 Linux 标准 auxv 栈布局（{type, value} pair + AT_NULL 终止）
- main thread TLS：在栈上手工分配 ~512B TLS 块，填充 pthread_internal_t 结构（至少 errno=0, tid=1），设置 TPIDR_EL0
- stack 布局：定义 STACK_BASE，在栈顶排布 argc→argv→envp→auxv 指针序列
- constructor 执行调度：relocation 后按 init_order（拓扑序）逐个模块跑 init_plan，零参数调用
- `call_export` 改造：支持"已初始化进程上下文"下的调用（使用已有 stack/TLS，不再重复初始化）
- Zygote 自包含：不泄漏到 syscall handler、linker 或其它 consumer

**Non-Goals:**

- 不实现完整 auxv 向量（AT_PHDR / AT_ENTRY / AT_HWCAP / AT_UID 等——在 bootstrap 阶段不需要）
- 不实现 getauxval syscall handler（auxv 只给 guest libc 自扫描栈读；不提供 syscall 路径）
- 不实现 PT_TLS 模板处理（unidbg 证明不需要，bootstrap 阶段手构造 TLS 足够）
- 不实现 __environ 导出为独立全局符号（unidbg 用 dlsym 返回 argv[2] 地址，rundroid 第一版可暂不做）
- 不实现 linker 重写（constructor 执行在现有 link 流程之外调度，不改 linker 内部）
- 不实现 multi-thread TLS（仅 main thread）
- 不实现 `.preinit_array`（Android bionic 不使用，规范标准 DT_PREINIT_ARRAY 可忽略）

## Decisions

### 1. Zygote 集中承载进程初始化，而非分散到 loader / kernel / syscall 多处

unidbg 把初始化逻辑分散在 `AndroidElfLoader` 构造函数和 `initializeTLS()` 等多处。rundroid 选择在一个 dedicated struct 中聚合所有进程初始化逻辑，理由：

- 初始状态构造是**一个完整概念**（auxv + TLS + stack + constructor 必须放在一起考虑），分散后无法看到全貌
- 单一模块便于单测和 mock
- 新增 cap（如更多 AT_*、加载时 signalfd 等）时不会散得到处都是

**为什么不是放在 Kernel 里？**

Kernel（原 LinuxRuntime）管理 OS 状态（VFS/fd/brk/syscall 语义），进程初始化是"guest 进程的 initial context"，是 OS 层之上的概念。放在一起会使 Kernel 膨胀，违反单一职责。

### 2. 命名：当前使用 `Zygote`，作为 Open Question

备选命名：

| 命名 | 理由 |
|---|---|
| `Zygote` | 简短，在 Android 上下文中识别度高（虽然不是 Android Zygote 进程语义） |
| `GuestProcess` | 准确描述"guest 进程"概念 |
| `ProcessImage` | 强调"初始映像"而非运行时进程 |
| `Bootstrap` | 中性，强调"启动"阶段 |

**当前选择 `Zygote`**，因为用户偏好且足够简洁。Open Question 保留到实现阶段：如果实现中发现命名导致理解困难（如与 Android Zygote 进程混淆），可改名为 `ProcessImage`。

### 3. 最小化 auxv：只做 AT_RANDOM + AT_PAGESZ

unidbg 仅 2 个 AT_* 值足以运行真实 .so。auxv 越多表示与真实 Android 环境的接近度越高，但 bootstrap 阶段不需要。具体值：

| 类型 | 编号 | 值 | 说明 |
|---|---|---|---|
| AT_RANDOM | 25 | 栈上 16B 随机数 | 用于 libc canary（`__stack_chk_guard`） |
| AT_PAGESZ | 6 | 0x1000 | 页面大小，bionic libc 的 getpagesize/`mmap` 对齐依赖 |
| AT_NULL | 0 | {0, 0} | auxv 终止标记 |

不做 AT_PHDR/AT_ENTRY/AT_HWCAP/AT_UID/AT_SECURE 等——libc 在 canary 依赖之外不需要这些。

**不提供 getauxval syscall handler**：getauxval 在 bionic 中是 libc 内部函数，直接在 libc 的 data segment 读 auxv 副本，不触发 syscall。因此不需要在 Linux kernel syscall 层加 getauxval handler。

### 4. 手构造 TLS，不走 PT_TLS

unidbg 的 `initializeTLS()` 也手构造 TLS+TCB，PT_TLS 的处理和静态 TLS 模板提取被 skip（定义了的常量但 loader 跳过）。rundroid 采用同样的决策：

- 栈上分配 ~512B TLS 块（对齐 8B）
- 在 TLS 块内构建最小 pthread_internal_t：
  - `errno` 的指针/偏移 → 0（不是系统调用返回值时的 errno 值）
  - tid = 1（main thread）
- TPIDR_EL0 设置为 TLS 块地址
- TPIDR_EL0 的写入通过 `Engine::reg_write(Arm64Reg::TpidrEl0, addr)`，需确认 Arm64Reg 枚举包含该变体

**为什么不复用 PT_TLS 模板？**

PT_TLS 提取在 parser 层没有实现（`tls.rs` 目前返回 None）。为了跑 constructor 而实现 PT_TLS 解析 + 模板复制 + TCB 布局，比手构造 TLS 块更重、测试面更大。bootstrap 阶段用 hand-rolled TLS 足够。

### 5. constructor 零参数调用，执行序 = 拓扑序

当前 linker 已经用 Kahn 算法计算了 `init_order`（拓扑序）。本 change 直接复用：

1. relocation 完成 → LinkReport 产生
2. 按 `init_order` 遍历 modules
3. 对每个 module 执行其 `InitPlan`：
   - 若 `legacy_init (DT_INIT)` 非 None，调用该函数
   - 若 `init_array` 非空，逐个调用数组中的函数指针
4. 所有 constructor 执行完毕 → JNI_OnLoad

**参数处理**：constructor 零参数调用。ARM64 约定下 x0-x7 可以不设值（或者 x0=0/mold_id）。unidbg 对 constructor 不传参，我们采用相同策略。

**地址解析**：`InitPlan` 当前存的是指针 slot 的 guest 地址（`base + i*8 + load_bias`），不是函数指针值。constructor 调用前需要通过 `GuestCPU::mem_read` 读取 slot 中存放的函数指针值。

### 6. Zygote 放在 `emulator/os/android/src/zygote.rs`，与 Kernel 同级

`Zygote` 与 `Kernel` 都是 android OS crate 的根级类型。Zygote 负责"guest 进程初始状态"，Kernel 负责"运行时的 OS 状态与 syscall 语义"。

但 android OS crate 当前不存在（`android-os-rebrand` change 还未实现）。本 change 有两种方案：

**方案 A（推荐）**：在本 change 内新建 `emulator/os/android/` crate，只放 `Zygote`。`android-os-rebrand` change 后续把 linux crate 移入。

**方案 B**：先放 `emulator/case-runner/src/`，等 android crate 创建后再搬。

选择方案 A。因为：
- Zygote 是一个独立概念，不应该寄生在 case-runner 内
- 新建一个仅包含 Zygote 的 crate 成本很低
- `android-os-rebrand` 的 merge 冲突可控（只需搬 Zygote 目录）

### 7. `call_export` 不变——Zygote 在 relocation 后、call_export 前执行

本 change 不改变 `call_export` 的签名或内部 sentinel trick。调整的是调用时序：

```
load_and_link:
  1. parse + load + relocate + link
  2. Zygote::bootstrap(...)  ← 新步骤：auxv + TLS + stack + constructor
  3. detect_jni_onload(...)  ← 已有步骤
```

`call_export` 仍然设 SP/LR/PC/x0-x7，但此时的 SP 已经是 Zygote 布置过的栈顶（而非随意值），TLS 已经可用。

如果某个导出调用需要独立的栈上下文（如重入），可以由 caller 在调用前后保存/恢复 SP，不在本 change 范围内。

### 8. error 处理：constructor 失败应传播，不静默吞掉

如果某个 constructor 执行中 guest 触发未处理异常（如 `__stack_chk_fail` 因 canary 不匹配），引擎应返回错误。本 change 要求：

- `emu_start` constructor 时如果返回 error（`BackendError` 或超时），立即上抛给 assembly 层
- 不静默跳过失败的 constructor
- 不区分"constructor 致命/非致命"（bootstrap 阶段 fail-fast）

## Risks / Trade-offs

- **[依赖尚未完成的 android crate]** → 缓解：本 change 新建 `emulator/os/android/` crate 仅放 Zygote。如果 `android-os-rebrand` change 先完成，Zygote 直接放入已有 crate 即可。
- **[Arm64Reg 枚举缺少 TPIDR_EL0]** → 缓解：对 engine crate 增加 `TpidrEl0` 变体，或在 Zygote 中用通用 `msr` 指令通过代码执行来写系统寄存器。备选方案：若 Engine trait 不支持系统寄存器，可映射一页含 `msr tpidr_el0, x0; ret` 指令，通过 `emu_start` 执行来设置。
- **[constructor 的 init_array 函数指针值需要 mem_read 解析]** → 缓解：Zygote 需要访问 `GuestCPU::mem_read` 或 `MemoryBridge::read`；设计时明确 Zygote 依赖窄接口，不直接依赖 engine。
- **[execution ordering：constructor 与 JNI_OnLoad 交叉依赖]** → 缓解：按规范 constructor（relocation 后立即执行）先于一切导出函数调用（包括 JNI_OnLoad）。如果发现真实 Android .so 依赖反向，后续再调整。
- **[stack 地址的选择：unidbg 用 0xe5000000，rundroid 当前用 0x7F_E000_0000]** → 缓解：保持 rundroid 当前 STACK_BASE（0x7F_E000_0000，已在 runtime.rs 中定义），不改为 unidbg 的值。这是个实现细节，不改变能力边界。
- **[TLS 块里只放 errno 和 tid，不够完整]** → 缓解：bootstrap 阶段只做最小子集。若遇到依赖更完整 TLS（如 svelte 的 `__get_tls()` 访问更多字段），后续扩展。
