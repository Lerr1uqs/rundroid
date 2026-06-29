## Why

真实 Android .so 大量使用 pthread（线程创建、锁、futex 同步）。rundroid 是单 Unicorn 实例，仅实现 clone/futex syscall 号会让 futex_wait 永久阻塞、线程无法交替执行。需要一个协作式调度器在 syscall 层之上管理 context save/restore + 线程切换，使多线程 .so 能在单 Unicorn 上正确运行。

## What Changes

- 新增 `thread.rs` 模块（`src/thread/`）：ThreadScheduler + ThreadContext + Task 体系
- `SyscallResult` 新增 `Yield` 和 `Blocked` 变体
- clone syscall 拦截：创建 MarshmallowThread → `add_thread`
- futex syscall 拦截：WAIT → Blocked, WAKE → `wake_matching` + Yield
- sched_yield/exit syscall 拦截
- 调度循环集成到 os linux crate 的 Kernel / case-runner 装配层
- 新 capability `android-thread-scheduler`：协作式多线程调度契约

## Capabilities

### New Capabilities
- `android-thread-scheduler`: 单 Unicorn 实例上的协作式多线程调度，管理线程上下文保存/恢复、futex 阻塞/唤醒、线程创建/退出

### Modified Capabilities
- `linux-layering`: syscall 返回路径需支持 Yield/Blocked 变体；Kernel 结构体需持有 ThreadScheduler
- `runtime-core`: 调度循环集成到运行时装配层，engine 层面的 emulate 调用需适配调度循环

## Impact

- **新文件**: `runtime/os/linux/src/thread/`（mod.rs, context.rs, waiter.rs, task.rs, scheduler.rs）
- **修改文件**: `runtime/os/linux/src/syscall.rs`（SyscallResult 新变体 + clone/futex/sched_yield 处理）、`runtime/os/linux/src/kernel/mod.rs`（Kernel 持有 scheduler）、`runtime/os/linux/src/kernel/mem.rs`（clone 栈分配）
- **API 变更**: `SyscallResult::Done(u64)` 之外新增 `Yield(Option<i64>)` 和 `Blocked(Box<dyn Waiter>)`，调用者必须处理这两种新变体
- **依赖**: unicorn-engine 的 context API（`uc_context_alloc/save/restore/free`），若 Rust binding 未暴露则 FFI 直调
