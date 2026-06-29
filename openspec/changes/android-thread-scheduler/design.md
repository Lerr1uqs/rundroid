## Context

Android native .so 严重依赖 pthread 线程模型：`pthread_create`（底层 clone syscall）、`pthread_mutex_lock`（底层 futex syscall）、`pthread_cond_wait`（底层 futex syscall）。rundroid 当前是单 Unicorn 实例，syscall 拦截是同步的——`futex(FUTEX_WAIT)` 如果仅返回一个值，调用线程继续跑，但 wait 语义要求**阻塞直到被唤醒**。在单 Unicorn 上这等价于无限循环或死锁，因为没有其他线程能运行来发出 wake。

已参考 unidbg 实现（`UniThreadDispatcher` ~350 行 Java）验证方案可行性：unidbg 100% 靠 syscall 拦截 + 调度器，没有任何 libc 函数 hook。其核心机制是 `uc_context_alloc/save/restore/free` 这套 Unicorn C API 做全寄存器保存/恢复。

当前状态：
- `rundroid-linux` crate 已有 syscall 拦截框架（`syscall.rs`），`SyscallResult` 目前有 `Done(u64)` / `Exit(i32)` / `Unimplemented` 变体
- `Kernel`（`linux/src/kernel/mod.rs`）持有 `LinuxRuntime` 的 OS 状态聚合
- `case-runner` 是装配层，持有 `GuestRuntime` + syscall dispatch 逻辑
- backend trait 已有 `emu_start`/`emu_stop`/`reg_read`/`reg_write`/`mem_read`/`mem_write` 等

## Goals / Non-Goals

**Goals:**
- 在单 Unicorn 实例上实现协作式多线程调度，支持 pthread 基本用例
- syscall 层新增 `SyscallResult::Yield` 和 `SyscallResult::Blocked` variant
- clone 拦截：支持 `CLONE_VM | CLONE_THREAD | CLONE_SETTLS | CLONE_CHILD_SETTID | CLONE_CHILD_CLEARTID`
- futex 拦截：`FUTEX_WAIT` → Blocked，`FUTEX_WAKE` → wake + Yield
- `sched_yield` / `sys_exit` / `sys_exit_group` 拦截
- 提供 `ThreadContext` 包装 unicorn context API（alloc/save/restore/free）
- 提供 `ThreadScheduler` 调度循环（round-robin runnable tasks）
- 调度器 opt-in，不影响现有单线程路径
- 端到端测试验证 clone + wait + wake 循环

**Non-Goals:**
- 抢占式调度（无时间片、无定时器中断）
- `CLONE_VFORK` / `CLONE_FILES` / `CLONE_SIGHAND` 等复杂 flags（Android pthread_create 只用子集）
- futex `FUTEX_WAIT` 带 timeout（暂返 ENOSYS）
- `FUTEX_CMP_REQUEUE` / `FUTEX_WAKE_OP` 等高级操作
- `FUTEX_PRIVATE_FLAG` 的优化（全局检查即可）
- 亲和性（CPU set）、优先级、cgroup 等调度策略
- thread-local storage（TLS）的堆分配或析构——`CLONE_SETTLS` 只设 `TPIDR_EL0`，guest 自己管理的 TLS 区域不变
- 多 Unicorn 实例 / 多后端并行

## Decisions

### Decision 1: 协作式调度（非抢占式）

**Choice**: 线程只在 `futex_wait` / `exit` / `sched_yield` / `futex_wake` / `clone` 时切换，无时间片。

**Rationale**:
- Android bionic pthread 的同步原语都是 futex-based，配合协作式调度足够
- 抢占式需要定时器中断（SIGALRM 或 Unicorn hook），在单实例上复杂且无必要
- unidbg 也是协作式，已验证此模式工作
- 避免引入 timer hook 的性能开销和竞态可能性

**Alternatives considered**:
- 抢占式（timer-based）：拒绝，复杂度高且 Android native 代码无此需求
- 混合式（协作+抢占 fallback）：拒绝，YAGNI

### Decision 2: ThreadContext 包装 unicorn context API

**Choice**: 直接封装 `uc_context_alloc` / `uc_context_save` / `uc_context_restore` / `uc_context_free`，若 Rust crate 未暴露则 FFI 直调。

**Rationale**:
- Unicorn 的 context API 是保存/恢复全部 CPU 寄存器的最高效方式（一次 `uc_context_save` = 批量寄存器读取）
- 比手动逐个读/写 30+ 个寄存器快得多，也避免了遗漏
- unicorn-engine crate 2.x 的 `Unicorn::context_alloc` 返回 `Context` handle，支持 `save`/`restore`
- 如果 crate 不暴露，用 `libc::dlsym` 或直接 link unicorn C lib 调用即可

**Alternatives considered**:
- 手动逐个寄存器保存/恢复（read_registers + write_registers）：拒绝，性能差、代码冗长、易漏寄存器

### Decision 3: SyscallResult 扩展而非用异常/回调

**Choice**: 在 `SyscallResult` 新增 `Yield(Option<i64>)` 和 `Blocked(Box<dyn Waiter>)` 变体，调度器消费这些结果。

**Rationale**:
- 现有 `SyscallResult` 已被 `dispatch` 返回，扩展比引入新机制更一致
- Rust 无异常，`Yield/Blocked` 比 `panic` + catch 更可控
- 调度器 loop 消费 `SyscallResult` 的流程清晰：
  ```
  loop {
      match dispatch(cpu, &mut rt, bridge) {
          Done(v) => continue (next task 或返回)
          Exit(c) => cleanup + switch
          Yield => save_context + next_task
          Blocked(w) => set_waiter + skip
      }
  }
  ```

**Alternatives considered**:
- 回调注入（`on_yield` / `on_blocked` 闭包）：拒绝，破坏现有 dispatch 的同步返回值契约
- `Result<u64, SchedulerAction>` 枚举：等价，但现有 `SyscallResult` 已用枚举，保持一致

### Decision 4: Waiter trait + FutexWaiter

**Choice**: `Waiter` trait 定义 `can_dispatch() -> bool` 和 `on_continue_run()`，`FutexWaiter` 是最初的唯一实现。

**Rationale**:
- trait 允许未来扩展其他阻塞原语（`ConditionWaiter`、`FileWaiter`、`SleepWaiter`）
- futex 只需要比较 `uaddr` 和存储的 `uaddr` 匹配即可唤醒
- `FutexWaiter` 存 `uaddr: u64` 和 `val: u32` 用于匹配检查

```rust
pub trait Waiter: Send {
    fn can_dispatch(&self) -> bool;
    fn on_continue_run(&mut self);
}

pub struct FutexWaiter {
    pub uaddr: u64,  // 用户空间 futex 地址
    pub val: u32,    // 期望值
}
```

### Decision 5: Scheduler 在 linux crate 内作为 Kernel 子模块

**Choice**: `thread` 模块放在 `runtime/os/linux/src/thread/` 下，Kernel 持有 `scheduler: Option<ThreadScheduler>`，clone/futex/sched_yield syscall handler 通过 `rt.scheduler` 访问。

**Rationale**:
- clone/futex 的语义是 Linux OS 的一部分，放在 linux crate 里自然
- syscall handler 已经持有 `&mut LinuxRuntime`，可直接访问 scheduler
- `Option` 允许现有单线程路径零开销（`None` 时 clone 返 ENOSYS）
- 调度器的 context API 通过 backend trait 的扩展（`context_alloc` / `context_save` / `context_restore` / `context_free`）访问 Unicorn

```rust
// linux/src/thread/scheduler.rs
pub struct ThreadScheduler {
    tasks: Vec<Box<dyn Task>>,
    current: Option<usize>,
}

impl ThreadScheduler {
    pub fn new() -> Self { ... }
    pub fn add_thread(&mut self, task: Box<dyn Task>) -> u32 { ... }
    pub fn wake_matching(&mut self, uaddr: u64, max_wake: u32) -> u32 { ... }
    pub fn run_loop(&mut self, cpu: &mut dyn Backend, bridge: &mut dyn MemoryBridge) -> i32 { ... }
}
```

**Alternatives considered**:
- 在 case-runner 里做调度（拒绝：syscall handler 需要直接访问 `add_thread`/`wake_matching`，跨 crate 调用增加耦合）
- Engine trait 加调度方法（拒绝：engine 不应感知线程模型，保持 backend 干净）

### Decision 6: clone flags 支持子集

**Choice**: 实现 Android 6.0+ bionic `pthread_create` 使用的 flags 子集：
`CLONE_VM | CLONE_THREAD | CLONE_SETTLS | CLONE_CHILD_SETTID | CLONE_CHILD_CLEARTID`

**Rationale**: 该子集是 `pthread_create` 在 arm64 Android 上实际传入的 flags。其他 flags（`CLONE_VFORK` / `CLONE_FILES` / `CLONE_SIGHAND`）在 pthread 场景未使用。

```rust
// clone flags 常量（来自 include/uapi/linux/sched.h）
const CLONE_VM: u64 = 0x100;
const CLONE_THREAD: u64 = 0x10000;
const CLONE_SETTLS: u64 = 0x80000;
const CLONE_CHILD_SETTID: u64 = 0x2000000;
const CLONE_CHILD_CLEARTID: u64 = 0x4000000;
```

### Decision 7: 调度循环集成在 case-runner 层

**Choice**: 调度循环不放在 backend trait 或 linux crate 内部，而是放在装配层（case-runner）。

**Rationale**:
- case-runner 已经持有 `GuestRuntime` + backend + syscall dispatch，是启动执行的自然位置
- 单线程路径（现有 `call_export`）保持不变
- 新入口 `run_with_scheduler` 与 `call_export` 并列

## Risks / Trade-offs

- **[协作式局限性] 若 guest 代码长时间无 syscall 的计算密集型操作，其他线程饥饿** → Mitigation: 这是有意识的设计选择。Android native 代码中长时间计算通常伴随 pthread_cond_wait / pthread_mutex_lock 等同步操作。若遇纯计算场景，用户可在合适位置插入 yield 点。
- **[context API 兼容性] unicorn-engine Rust crate 可能未暴露 context_alloc/save/restore/free** → Mitigation: 验证 crate API。若缺失，通过 FFI 直接调用 unicorn C 库的 `uc_context_*` 函数（已在 cmake 编译的 unicorn.dll 中存在）。
- **[futex 地址有效性] FUTEX_WAIT 读 uaddr 时 guest 内存可能无效** → Mitigation: 通过 MemoryBridge 读取，失败时返 EFAULT。
- **[tid 分配冲突] 自增 tid 可能回绕或与主线程 tid 冲突** → Mitigation: tid 分配器从 1 开始递增，主线程固定 tid=1，子线程 2..N。u32 范围足够大，短期不回绕。
- **[empty task list] 所有线程都阻塞时调度器空转** → Mitigation: 检测到无可运行线程时 panic 或返回错误（正常场景不会发生，因为至少主线程应可运行或已退出）。
- **[CLONE_CHILD_CLEARTID/CLONE_CHILD_SETTID 的 guest 内存写] 线程退出需写 guest 内存通知** → Mitigation: 线程退出时，若 `CLONE_CHILD_CLEARTID` 设置，经 MemoryBridge 写 0 到 `child_tid` 地址；`CLONE_CHILD_SETTID` 在创建时写 tid 到 `child_tid` 地址。

## Migration Plan

1. **ThreadContext + context API**：在 backend trait 加 `context_alloc/save/restore/free` 方法，unicorn backend 实现
2. **Waiter trait + FutexWaiter**：纯 data 结构，无 backend 依赖
3. **Task trait + MainTask + MarshmallowThread**：纯 data，无 backend 依赖
4. **ThreadScheduler**：内部逻辑（add/wake），不依赖 unicorn context API
5. **SyscallResult 扩展**：加 Yield/Blocked 变体
6. **clone syscall handler**：在 `syscall.rs` 中实现 `SYS_clone`/`SYS_clone3`
7. **futex syscall handler**：在 `syscall.rs` 中实现 `SYS_futex`/`SYS_futex_time64`
8. **sched_yield + exit handlers**：已有关联但需调整为返回 Yield
9. **调度循环集成**：case-runner `runtime.rs` 加 `run_with_scheduler` 入口
10. **测试**：编写端到端测试（clone + wait + wake 循环）

## Open Questions

- `clone3`（SYS_clone3 = 435）在 Android 6.0 上是否已被使用？若否，初期可仅实现 `SYS_clone`（220）
- `FUTEX_WAIT_BITSET` 与 `FUTEX_WAIT` 的语义差异——暂按 FUTEX_WAIT 处理是否足够？
- 调度器 loop 中 `emulate` 返回值的处理细节：Unicorn 在 sentinel 处停止，如何与调度器 loop 交互？
