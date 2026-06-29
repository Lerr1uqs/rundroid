## ADDED Requirements

### Requirement: Syscall surface is a dedicated object owned by Kernel

android crate SHALL 把 syscall 分派入口与全部 `sys_*` handler 收敛到独立的 `Syscall` 类型（落在 `impl Syscall`），`Kernel` SHALL 持有 `syscalls: Syscall` 字段，且 SHALL NOT 在 `impl Kernel` 上承载任何 `sys_*` handler 或 dispatch 逻辑——Kernel 只保留一行薄转发入口。`Syscall::dispatch` SHALL 设计为关联函数形式（接收 `&mut Kernel` + syscall 号 + 参数寄存器 + `&mut dyn MemoryBridge`），以避开"`kernel.syscalls` 字段访问 + 同一表达式再借整个 `Kernel`"的双重借用约束。`Syscall` 起步为不持有 OS 状态的分派器（syscall 层专属状态如 tracing 计数后续再扩），所有 OS 状态访问 SHALL 经入参 `&mut Kernel` 完成。

#### Scenario: sys_* handlers live on impl Syscall not impl Kernel

- **WHEN** 审视 android crate 的 syscall 实现
- **THEN** 所有 `sys_openat` / `sys_read` / `sys_write` / ... handler SHALL 定义在 `impl Syscall`
- **AND** `impl Kernel` 上 SHALL NOT 出现任何 `sys_*` handler 或 dispatch match 逻辑

#### Scenario: Kernel dispatch is a thin forwarder

- **WHEN** 调用方触发一次 syscall
- **THEN** `Kernel::dispatch` SHALL 仅以一行转发到 `Syscall::dispatch(self, ...)`
- **AND** SHALL NOT 在 Kernel 侧重复参数解码或 handler 路由

#### Scenario: Kernel field owns the Syscall object

- **WHEN** 构造 `Kernel`
- **THEN** 它 SHALL 持有 `syscalls: Syscall` 字段
- **AND** Syscall 的 dispatch 经关联函数 + Kernel 转发调用，不产生同表达式双重借用

### Requirement: Unimplemented syscalls surface explicitly

android crate SHALL NOT 把未实现的 syscall 静默映射为 `ENOSYS`。`SyscallResult` SHALL 新增 `Unimplemented { nr: u64, name: &'static str }` 变体；`dispatch` 命中号表中标 `stub` 的号时 SHALL 返回该变体（携带 syscall 号 + 名字）。完全不在号表中的号 SHALL 同样返回 `Unimplemented`（`name` 为 `"unknown"` 等占位），而不是静默 `ENOSYS`。

#### Scenario: Unimplemented variant carries nr and name

- **WHEN** dispatch 命中一个标 `stub` 的 syscall 号（如 `io_setup` nr=0）
- **THEN** 它 SHALL 返回 `SyscallResult::Unimplemented { nr: 0, name: "io_setup" }`
- **AND** SHALL NOT 返回 `ENOSYS` 数值

#### Scenario: Unknown number also surfaces as Unimplemented

- **WHEN** dispatch 收到一个号表未覆盖的号
- **THEN** 它 SHALL 返回 `SyscallResult::Unimplemented { nr, name: "unknown" }`
- **AND** SHALL NOT 静默返回 `ENOSYS`

#### Scenario: No silent ENOSYS for unimplemented numbers

- **WHEN** 任意未实现或未知 syscall 号进入 dispatch
- **THEN** 结果 SHALL 是 `Unimplemented` 变体
- **AND** SHALL NOT 是携带 `ENOSYS` 的 `Done`

### Requirement: Unimplemented handling policy is decided at the assembly layer

android crate 的 `Syscall` 层 SHALL 保持策略无关：它只产出 `SyscallResult::Unimplemented`，SHALL NOT 自行 panic 或返回 `ENOSYS`。策略决策 SHALL 只发生在持有 `SyscallResult` 的上层——case-runner `SyscallDispatcher` 与 Python bindings hook 各自 SHALL 持有 `UnimplementedPolicy { Panic, Enosys }`，**默认 `Panic`**（fail-fast：panic 并在消息中指明缺失的 syscall 号 + 名字），可配 `Enosys`（降级返回 `ENOSYS` 继续执行）。

#### Scenario: Default policy panics fast with diagnostic

- **WHEN** 上层收到 `SyscallResult::Unimplemented { nr, name }` 且策略为默认 `Panic`
- **THEN** 它 SHALL panic
- **AND** panic 消息 SHALL 包含 syscall 号与名字（便于定位缺失实现）

#### Scenario: Enosys policy degrades gracefully

- **WHEN** 策略配置为 `Enosys`
- **THEN** 上层 SHALL 把 `Unimplemented` 降级为返回 `ENOSYS` 并继续执行
- **AND** SHALL NOT panic

#### Scenario: Syscall layer is policy-agnostic

- **WHEN** `Syscall::dispatch` 产出 `Unimplemented`
- **THEN** Syscall 层 SHALL NOT 自行决定 panic 或 ENOSYS
- **AND** 策略选择 SHALL 完全由上层 `UnimplementedPolicy` 表达

### Requirement: Full Android syscall number table via macro

android crate SHALL 通过 `define_android_syscalls!` 宏（crate 内 `macro_rules!`）声明全量 ARM64 Linux/Android syscall 号表，覆盖 `include/uapi/asm-generic/unistd.h` 的号空间（约 460 条）。宏 SHALL 为每条 arm 标注两态之一——`impl <handler>`（已实现，路由到对应 `sys_*`）或 `stub`（未实现，返回 `Unimplemented`）——并展开为三类产物：syscall 号常量、`syscall_name(nr: u64) -> Option<&'static str>` 诊断函数、以及 `dispatch` 的 match（`impl` arm 调对应 `sys_*`，`stub` arm 返回 `Unimplemented`）。已实现号集（openat / close / read / pread64 / write / ioctl / fstat / exit / exit_group / brk / mmap / munmap / getrandom / dup / dup3）SHALL 标 `impl`，其余 SHALL 标 `stub`。

#### Scenario: Macro emits constants name table and dispatch match

- **WHEN** 展开 `define_android_syscalls!`
- **THEN** 它 SHALL 产出 syscall 号常量、`syscall_name(nr)` 诊断函数、`dispatch` match 三类产物
- **AND** 所有号集中在一个声明处，而非散落

#### Scenario: Implemented numbers route to sys_* handlers

- **WHEN** dispatch 收到一个标 `impl` 的号（如 `read` nr=63）
- **THEN** 它 SHALL 调用对应 `sys_read` handler 并返回其 `SyscallResult`
- **AND** SHALL NOT 返回 `Unimplemented`

#### Scenario: Stub numbers surface as Unimplemented

- **WHEN** dispatch 收到一个标 `stub` 的号（如 `futex` 未实现时）
- **THEN** 它 SHALL 返回 `SyscallResult::Unimplemented { nr, name }`
- **AND** `name` SHALL 匹配号表中该号的名字

#### Scenario: Adding an implementation is a single arm change

- **WHEN** 后续为某个 `stub` 号补实现
- **THEN** 仅需把该 arm 从 `stub` 改为 `impl <handler>` 并提供 handler
- **AND** SHALL NOT 需要修改 Kernel 或 dispatch 骨架

### Requirement: Android namespace carried by crate name

android OS 层的命名空间 SHALL 由 crate 名承载：公开类型 SHALL 以 `rundroid_android::Kernel` / `rundroid_android::Syscall` / `rundroid_android::SyscallResult` 形式可达。crate 内 SHALL NOT 额外套 `android` mod（避免 `rundroid_android::android::Kernel` 冗余路径），从而与未来可能重建的 `rundroid_linux::Kernel` 靠 crate 名天然区分。

#### Scenario: Public types reachable under crate name

- **WHEN** 下游（case-runner / Python bindings）引用 OS 层类型
- **THEN** 路径 SHALL 为 `rundroid_android::Kernel` / `rundroid_android::Syscall`
- **AND** SHALL NOT 出现额外的 `android` 模块段

#### Scenario: No redundant inner android module

- **WHEN** 审视 `rundroid-android` crate 结构
- **THEN** 它 SHALL NOT 定义 `pub mod android`
- **AND** 命名空间区分 SHALL 依赖 crate 名而非内部 mod 包裹
