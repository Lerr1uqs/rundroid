## Context

OS 层现状（`emulator/os/linux/`，crate `rundroid-linux`）：

- `LinuxRuntime` 是 OS 聚合根，扁平持有 vfs/device_registry/fds/next_mmap/brk/stdout/exit_code/rng_seed/telemetry。kernel 域语义按 `kernel/{fd_io,mem,random}.rs` 分文件 `impl LinuxRuntime`。
- syscall ABI 边界全堆在 `syscall.rs` 的 `impl LinuxRuntime`：`dispatch`（大 match）+ 16 个 `sys_*` handler。未实现分支 `_ => emit("syscall.unknown") + ENOSYS`。
- 下游耦合：case-runner `SyscallDispatcher` 用 `Arc<Mutex<LinuxRuntime>>` + 裸指针绕"重叠 mut 借用"调 `linux.dispatch(...)`；Python bindings `lib.rs` 4 处持 `Arc<Mutex<LinuxRuntime>>`。`backends/api/engine.rs` 仅注释提及，无真依赖（backend 边界干净）。
- 已有 `linux-layering` 权威 spec 规定 kernel/syscall 分层 + 单一 `MemoryBridge` + errno 集中 + 域分文件 + 语义可单测。

本次 change 是在已分层基础上做**命名收敛 + syscall 层对象化 + 未实现 fail-fast + 全量号表骨架**，不是从零造。

## Goals / Non-Goals

**Goals:**

- OS 层品牌/命名收敛到 Android（crate/目录/类型）。
- syscall 层从 `impl LinuxRuntime` 独立成 `Syscall` 类型，Kernel 只持有不实现，syscall 集后续扩张不再撑胖 Kernel。
- 未实现 syscall 从静默 `ENOSYS` 改为显式 `Unimplemented` + 上层策略，默认 fail-fast。
- 铺好全量 ARM64 Linux/Android syscall 号表（宏生成），后续补实现 = 改一条 arm。

**Non-Goals:**

- 不实现任何新的 `stub` syscall（只铺骨架 + `Unimplemented`）。
- 不把 `UnimplementedPolicy` 接入 `RuntimeConfig`（本期上层硬编码默认 `Panic`）。
- 不重建 `rundroid-linux` crate（只靠 crate 名留出未来命名空间位）。
- 不动 backend / jni / ELF 边界，不碰 RELRO/TLS 遗留。
- 不改变已实现 syscall 的任何可观察行为（pure rename + 重组）。

## Decisions

### Decision 1: 命名空间由 crate 名承载，不套内部 mod

`rundroid_android::Kernel` / `rundroid_android::Syscall` 已能与未来 `rundroid_linux::Kernel` 区分。crate 内再套 `android` mod 会得到冗余的 `rundroid_android::android::Kernel`。

- **Alternatives**：(a) 单 `rundroid-os` crate 内 `mod android{...}` + `mod linux{...}` 并存——改动更大、要合并现有 crate 结构，且当前只有 Android 一个 OS 实例，过早抽象；(b) 保持 crate 名 `rundroid-linux` 只改类型名——与"品牌收敛到 Android"目标矛盾。均不取。

### Decision 2: Syscall 对象化 + 关联函数规避双重借用

`Kernel` 持 `syscalls: Syscall` 字段后，若 dispatch 写成 `kernel.syscalls.dispatch(&mut kernel, ...)`，同一表达式里 `kernel` 被借两次（一次取字段、一次传 `&mut` 整体），编译不过——因为 dispatch 内部要读写 `kernel.fds`/`kernel.vfs` 等 OS 状态。

- **解法**：`Syscall::dispatch` 设计为**关联函数** `fn dispatch(kernel: &mut Kernel, nr, x0..x5, mem: &mut dyn MemoryBridge) -> SyscallResult`；`Kernel::dispatch(&mut self, ...)` 作为一行薄转发 `{ Syscall::dispatch(self, ...) }`。这样 `Kernel` 字段上确有 `syscalls: Syscall`（语义归属 + 预留扩展位），sys_* 全在 `impl Syscall`，调用方仅改类型名（`linux.dispatch` → `kernel.dispatch`），不引入 unsafe、不扩大裸指针边界。
- **sys_* 签名迁移**：原 `fn sys_read(&mut self, fd, buf, count, mem)`（self=`LinuxRuntime`）改为 `fn sys_read(kernel: &mut Kernel, fd, buf, count, mem)`（`impl Syscall`，OS 操作改走 `kernel.read(...)`）。16 个 handler 机械迁移。
- **Alternatives**：让 `Syscall` 持 OS 状态子集引用——生命周期无法表达（Syscall 与 Kernel 同生命周期，自引用）；保持 `impl Kernel` 只把 dispatch 抽到模块——不满足"Kernel 不直接持有 syscall 方法"诉求。均不取。

### Decision 3: `SyscallResult::Unimplemented` + 策略上移，dispatch 不自行 panic

`Syscall` 层只产出 `Unimplemented { nr, name }`，panic/ENOSYS 决策交给上层 `UnimplementedPolicy`。

- **Rationale**：dispatch 内直接 `unimplemented!` 会让任何真实 `.so` 触发未实现号时整体崩、无降级路径；策略上移后开发期默认 `Panic`（fail-fast 信号不变），运行期可配 `Enosys` 优雅降级。`Syscall` 保持策略无关、纯函数化、易单测。
- **`SyscallResult` 扩展**：`Done(u64) | Exit(i32) | Unimplemented { nr: u64, name: &'static str }`。所有持 `SyscallResult` 的 match（case-runner dispatcher、Python hook）须补 `Unimplemented` 分支按策略处理。
- **Alternatives**：dispatch 内 `unimplemented!`——最简但无降级；保持 `ENOSYS` + 强告警——fail-fast 信号弱。均不取（取折中）。

### Decision 4: `define_android_syscalls!` 宏生成全量号表（impl/stub 两态）

宏声明全量 ~460 条号，每条标 `impl <handler>` 或 `stub`，展开为号常量 + `syscall_name(nr)` 诊断函数 + `dispatch` match。

```rust
define_android_syscalls! {
    // 已实现：nr => name => impl <handler>
    29  => ioctl    => impl sys_ioctl;
    56  => openat   => impl sys_openat;
    63  => read     => impl sys_read;
    // ... 其余 13 个已实现
    // 未实现：nr => name => stub   (约 440 条，数据源 include/uapi/asm-generic/unistd.h)
    0   => io_setup   => stub;
    1   => io_destroy => stub;
    98  => futex      => stub;
    // ...
}
```

- **Rationale**：编译期完整覆盖、无运行时查找；号表集中一处；补实现 = 把 `stub` 改 `impl <handler>` + 提供 handler，不动 Kernel/dispatch 骨架；诊断名随 arm 声明，`Unimplemented` 消息可读。
- **数据源**：ARM64 Android 用标准 Linux syscall 号（`include/uapi/asm-generic/unistd.h`），~460 条。stub 名直接取该头里的 `__NR_<name>`。
- **Alternatives**：(a) `const TABLE: &[(u64, &str, Option<handler>)]` 数据驱动表 + 运行时查找——符合项目"声明式 catalog"风格但 `Unimplemented`/handler 异构难统一进单表，且运行时查找有（极小）开销；(b) 460 arm 手写 match——直白但维护痛、易漏。用户选定宏方案。

### Decision 5: `linux-layering` 用 MODIFIED 命名跟随，不废弃/新建 capability

`linux-layering` 的分层核心契约（kernel/syscall 分离、单一 `MemoryBridge`、errno 集中、域分文件、可单测）语义不变，仅类型/crate 名跟随（`LinuxRuntime`→`Kernel`、`linux crate`→`android crate`）+ syscall 承载主体对齐为 `Syscall` 类型。新契约（Syscall 对象化、Unimplemented、全量表、命名空间）进新 capability `android-syscall-surface`。

- **Rationale**：废弃 `linux-layering` 整个 cap（REMOVE 全部 requirement + 新建）成本高、收益低；capability 名是稳定契约标识，保留无伤语义。#6 requirement 文本澄清为"保持**已实现** syscall 行为"，避免与 Unimplemented 新行为矛盾。
- **Alternatives**：新建 `android-os-layer` 取代 `linux-layering` 并 REMOVE 旧 cap——改动过重。不取。

## Risks / Trade-offs

- **[重命名波及面广]** → case-runner 裸指针 dispatcher + Python 4 处 `Arc<Mutex<...>>` + workspace `Cargo.toml` + case.toml + ROADMAP。tasks 逐项列出，每步配 `cargo test --workspace` + `cargo run -p rundroid-cli -- case ...` 验证；纯机械改名，风险低。
- **[宏 ~460 条号数据正确性]** → 从 `asm-generic/unistd.h` 权威头生成，加"号→名"映射单测（抽样校验已知号如 read=63/write=64/futex=98/exit=93）；`syscall_name` 对已实现号必须返回正确名。
- **[默认 Panic 撞已有 case]** → case `01-04` 只触已实现号，不会命中 `Unimplemented`；补一个"未实现号默认 panic"单测显式锁定策略。
- **[`SyscallResult` 新增变体漏处理]** → 编译器强制穷尽 match，所有 match 处不补分支即编译失败，自然防漏。
- **[宏调试不直观]** → 宏展开产物（dispatch match / syscall_name）配单测覆盖；展开后逻辑等价于原 match，可读性靠 arm 声明集中保证。
- **[Trade-off: 宏 vs 数据表]** → 宏牺牲了一点"数据驱动"的运行时可枚举性（如想运行时遍历全部号需额外展开一个表），换取编译期完整 + 零查找开销；本期无运行时遍历号表的需求，可接受。

## Migration Plan

- 纯重命名 + 结构重组 + 新增宏/变体，**无数据/状态迁移**，无兼容层。
- 单 change 原子提交；回滚 = `git revert`。
- 顺序：先 crate/目录/类型改名（保编译绿）→ 抽 `Syscall` 类型迁 sys_* → 加 `Unimplemented` 变体 + 上层策略 → 引宏建全量表 → 全量测试 + case 回归。

## Open Questions

- 全量号表是否需要同时展开一个 `const ANDROID_SYSCALL_NAMES: &[(u64, &str)]` 供未来运行时枚举/文档生成？本期**不展开**（无消费者），宏内部供 `syscall_name` 单点查询即可；若后续 tracing/文档有需求再扩宏产物。
