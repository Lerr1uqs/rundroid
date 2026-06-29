## Why

现状 OS 层是 bootstrap 期按"Linux 用户态运行时"命名的（crate `rundroid-linux`、类型 `LinuxRuntime`），但项目目标明确是 **ARM64 + Android + Unicorn**，品牌/命名应当收敛到 Android。更紧要的是两个结构问题：

1. **syscall 表面缺失信号**：`dispatch` 只覆盖 ~16 个已实现 syscall 号，未实现分支统一返回 `ENOSYS` 并静默吞掉。跑真实 Android `.so` 时，某个没实现的 syscall 被触发后毫无信号、极难定位，违背项目的 fail-fast 调试原则。
2. **Kernel 持续膨胀**：`dispatch` 入口与全部 `sys_*` handler 都堆在 `impl LinuxRuntime` 上。syscall 集后续要持续扩张（socket / signal / procfs / futex / ...），Kernel 会越来越胖，OS 状态语义与 ABI 边界职责纠缠。

本次 change 把 OS 层命名收敛到 Android，把 syscall 层独立成 `Syscall` 对象，未实现改成显式 `Unimplemented` + 上层策略，并铺好**全量 ARM64 Linux/Android syscall 号表骨架**，让后续逐个补实现变成"改一条宏 arm"而不是"再给 Kernel 加方法"。

## What Changes

- **BREAKING**：crate `rundroid-linux` → `rundroid-android`，目录 `emulator/os/linux` → `emulator/os/android`。
- **BREAKING**：OS 聚合根类型 `LinuxRuntime` → `Kernel`。
- **syscall 层对象化**：新增 `Syscall` 类型，`Kernel` 持有 `syscalls: Syscall` 字段；所有 `sys_*` handler 从 `impl LinuxRuntime` 搬到 `impl Syscall`。`Syscall::dispatch` 设计为关联函数（接收 `&mut Kernel`）以避开"Kernel 字段 + 同表达式再借 Kernel"的双重借用；`Kernel` 保留 `dispatch` 作为一行薄转发入口，调用方仅改类型名。
- **BREAKING**：`SyscallResult` 新增 `Unimplemented { nr, name }` 变体；未实现 syscall 不再静默 `ENOSYS`，而是携带号 + 名字上抛，由上层决定如何处理。
- **未实现策略可配**：上层（case-runner `SyscallDispatcher` + Python bindings hook）各持一份 `UnimplementedPolicy { Panic, Enosys }`，**默认 `Panic`**（保 fail-fast），可配 `Enosys` 降级。策略来源本期硬编码默认，后续可挂 `RuntimeConfig`。
- **全量 syscall 号表**：新增 `define_android_syscalls!` 宏（crate 内 `macro_rules!`），生成全量 ~460 条 ARM64 Linux/Android syscall 的号常量、`syscall_name(nr)` 诊断表与 `dispatch` match。每条 arm 标 `impl <handler>` 或 `stub`：已实现的 16 个走 `impl`，其余走 `stub` 并返回 `Unimplemented`。
- **命名空间收敛**：靠 crate 名 `rundroid_android::Kernel` / `rundroid_android::Syscall` 与未来 `rundroid_linux::Kernel` 区分，**不**额外套 mod（避免 `rundroid_android::android::Kernel` 冗余）。

## Capabilities

### New Capabilities

- `android-syscall-surface`: Android OS 层 syscall 表面契约——`Syscall` 对象化（Kernel 委托不实现）、`SyscallResult::Unimplemented` + `UnimplementedPolicy`、全量 `define_android_syscalls!` 号表（impl/stub 两态）。

### Modified Capabilities

- `linux-layering`: 类型/crate 命名跟随重命名（`LinuxRuntime`→`Kernel`、`linux crate`→`android crate`、kernel 模块路径），分层核心契约（kernel 产数据不碰 `MemoryBridge` / syscall 经单一 `MemoryBridge` 回写 `EFAULT` / errno 集中 / 域分文件 / 语义可单测）语义不变。

## Impact

- **代码**：
  - `emulator/os/linux/**` 整个 crate 重命名 + 重组（`os/linux`→`os/android`，新增 `Syscall` 类型 + `define_android_syscalls!` 宏，`sys_*` 迁移到 `impl Syscall`）。
  - `emulator/case-runner/src/runtime.rs`（`SyscallDispatcher` + `Arc<Mutex<Kernel>>` + 裸指针桥接 + `Unimplemented` 分支处理）。
  - `emulator/case-runner/src/case.rs`、`emulator/case-runner/Cargo.toml`。
  - `emulator/bindings/python/src/lib.rs`（4 处 `Arc<Mutex<LinuxRuntime>>` + hook 的 `Unimplemented` 分支）。
  - workspace `Cargo.toml`、`Cargo.lock`（自动）、`ROADMAP.md`（路径索引）、`tests/cases/0{2,3}-*/case.toml`。
- **API**：`rundroid_linux::*` 全部公开导出改名（`Kernel` / `Syscall` / `SyscallResult` 新增变体），下游 case-runner / Python bindings 全部跟随；无新增外部 crate。
- **行为**：已实现 syscall 的返回值 / errno / 目标侧可见状态**保持不变**（pure rename + 结构重组）；唯一新行为是未实现 syscall 从静默 `ENOSYS` 变为显式 `Unimplemented`（默认 panic）。
- **测试**：现有 syscall 单测（含 pread64 四测）+ case `01/02/03/04` 必须保持绿；新增 `Unimplemented` 策略测试 + 全量表覆盖/诊断测试。
