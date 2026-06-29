## 1. ThreadContext: Unicorn context API 包装

- [ ] 1.1 在 backend trait 添加 `context_alloc` / `context_save` / `context_restore` / `context_free` 方法
- [ ] 1.2 Unicorn backend 实现这四个方法，调用 unicorn-engine 的 context API（或 FFI 直调）
- [ ] 1.3 编写 `ThreadContext` 结构体包装 context handle，确保 `Drop` 时释放

## 2. Waiter trait + FutexWaiter

- [ ] 2.1 定义 `Waiter` trait：`can_dispatch() -> bool` + `on_continue_run()`
- [ ] 2.2 实现 `FutexWaiter`：存 `uaddr: u64` + `val: u32`，`can_dispatch` 始终 false（仅由 wake 清除）
- [ ] 2.3 确保 `Waiter: Send`，便于跨线程传递

## 3. Task trait + MainTask + MarshmallowThread

- [ ] 3.1 定义 `Task` trait：`dispatch() -> SyscallResult` / `is_main() -> bool` / `is_finished() -> bool`
- [ ] 3.2 实现 `MainTask`：包装现有 "call a function until sentinel" 逻辑
- [ ] 3.3 实现 `MarshmallowThread`（子线程）：存 fn/arg/stack/tls/ctid，首次 dispatch 设置 x0/SP/TPIDR_EL0/LR

## 4. SyscallResult 扩展

- [ ] 4.1 `SyscallResult` 新增 `Yield` 和 `Blocked(Box<dyn Waiter>)` 变体
- [ ] 4.2 更新所有匹配 `SyscallResult` 的 match 表达式（编译修复）

## 5. ThreadScheduler 核心

- [ ] 5.1 `ThreadScheduler` 结构体：`tasks: Vec<Box<dyn Task>>` + `current: Option<usize>`
- [ ] 5.2 `add_thread(task) -> u32`：添加任务并分配 tid
- [ ] 5.3 `wake_matching(uaddr, max_wake) -> u32`：遍历 task，唤醒匹配的 FutexWaiter
- [ ] 5.4 `run_loop(cpu, bridge) -> i32`：round-robin 调度循环，处理 Yield/Blocked/Exit

## 6. clone syscall 拦截

- [ ] 6.1 在 `syscall.rs` 注册 `SYS_clone`(220) / `SYS_clone3`(435) handler
- [ ] 6.2 解析 flags：`CLONE_VM | CLONE_THREAD | CLONE_SETTLS | CLONE_CHILD_SETTID | CLONE_CHILD_CLEARTID`
- [ ] 6.3 创建 `MarshmallowThread` 并调用 `scheduler.add_thread`
- [ ] 6.4 在 `CLONE_CHILD_SETTID` 情况下经 MemoryBridge 写 tid 到 guest `child_tid` 地址
- [ ] 6.5 返回子线程 tid 给父线程 + `Yield` 触发调度器切换

## 7. futex syscall 拦截

- [ ] 7.1 在 `syscall.rs` 注册 `SYS_futex`(94) / `SYS_futex_time64`(422) handler
- [ ] 7.2 `FUTEX_WAIT`：读 uaddr 值，与 val 比较，等则返回 Blocked(FutexWaiter)，不等返 0
- [ ] 7.3 `FUTEX_WAKE`：调用 `scheduler.wake_matching`，返回唤醒数 + Yield
- [ ] 7.4 `FUTEX_WAIT_BITSET`：与 WAIT 同等处理

## 8. sched_yield / exit syscall 拦截

- [ ] 8.1 `sched_yield`(SYS_sched_yield=124)：返回 `Yield`
- [ ] 8.2 `exit`(SYS_exit=93) / `exit_group`(SYS_exit_group=94)：清理 task + 返回 Yield

## 9. 调度循环集成到 case-runner

- [ ] 9.1 case-runner `runtime.rs` 新增 `run_with_scheduler(&mut self, ...)` 入口
- [ ] 9.2 单线程路径保留现有 `call_export`，调度器 opt-in 不破坏现有路径
- [ ] 9.3 `LinuxRuntime` 持有 `Option<ThreadScheduler>`，clone/futex handler 通过 `rt.scheduler` 访问

## 10. 测试

- [ ] 10.1 编写 fixture：编译一个调用 pthread_create + pthread_mutex_lock/unlock 的 Android .so
- [ ] 10.2 ThreadContext 单元测试（alloc/save/restore/free 循环）
- [ ] 10.3 ThreadScheduler 单元测试（add/wake_matching/round-robin）
- [ ] 10.4 端到端测试：加载 fixture，验证 clone + wait + wake 循环输出
- [ ] 10.5 `cargo test --workspace` 全量通过
