## ADDED Requirements

### Requirement: ThreadScheduler manages per-thread context via Unicorn context API

The system SHALL provide a `ThreadScheduler` that manages multiple thread contexts on a single Unicorn instance using the Unicorn context API (`uc_context_alloc` / `uc_context_save` / `uc_context_restore` / `uc_context_free`). Each thread SHALL have an associated context handle that captures the full CPU register state.

#### Scenario: Context is allocated and saved on thread creation

- **WHEN** a new thread is created via clone syscall
- **THEN** the scheduler SHALL allocate a Unicorn context handle for the thread
- **AND** the current Unicorn register state SHALL be captured into the context after the clone handler returns

#### Scenario: Context is restored before dispatching a thread

- **WHEN** the scheduler selects a thread to run
- **THEN** it SHALL restore the thread's context via `uc_context_restore`
- **AND** the Unicorn instance register state SHALL match the saved thread state

#### Scenario: Context is freed on thread exit

- **WHEN** a thread exits (sys_exit / sys_exit_group / returns from entry function)
- **THEN** the scheduler SHALL free the thread's context handle
- **AND** the thread SHALL be removed from the scheduler's task list

### Requirement: clone syscall creates new task with allocated stack, TLS, and TPIDR_EL0

The system SHALL intercept the clone syscall to create a child thread task. On successful creation, the child task SHALL be registered with the scheduler's task list and a `Yield` result SHALL be returned to the caller so the scheduler can switch to the next runnable task.

#### Scenario: Clone creates child thread with full register setup

- **WHEN** the guest calls `clone(flags, child_stack, parent_tid, child_tls, child_tid)`
- **AND** `flags` contain `CLONE_VM | CLONE_THREAD | CLONE_SETTLS`
- **THEN** the system SHALL allocate a new `ThreadTask` with the child's entry function pointer, argument (`x0` for first dispatch), stack pointer (`child_stack`), TLS value (`child_tls`), and `CLONE_CHILD_SETTID`/`CLONE_CHILD_CLEARTID` if present in flags
- **AND** the child SHALL be added to the scheduler's task list in pending state
- **AND** the syscall SHALL return the child's tid to the parent in register `x0`

#### Scenario: Clone validates flags subset

- **WHEN** the guest calls `clone` with flags outside the supported set (`CLONE_VM | CLONE_THREAD | CLONE_SETTLS | CLONE_CHILD_SETTID | CLONE_CHILD_CLEARTID`)
- **THEN** the system SHALL return `-ENOSYS` (or `-EINVAL` for conflicting combinations)

#### Scenario: Child thread begins execution at entry function

- **WHEN** a child thread is dispatched for the first time
- **THEN** the system SHALL set the child's registers: `x0 = argument`, `SP = child_stack`, `TPIDR_EL0 = tls`, `LR = sentinel` address
- **AND** begin emulation from the entry function address until the sentinel is reached

### Requirement: futex(FUTEX_WAIT) blocks current task with FutexWaiter

The system SHALL intercept the futex syscall. When `FUTEX_WAIT` is called, the current task SHALL be associated with a `FutexWaiter` that tracks the futex address and expected value, and the syscall SHALL return a `Blocked` result to trigger a scheduler context switch.

#### Scenario: FUTEX_WAIT creates waiter and yields

- **WHEN** the guest calls `futex(uaddr, FUTEX_WAIT, val, timeout)` or `futex(uaddr, FUTEX_WAIT_BITSET, val, timeout, bitset)`
- **THEN** the syscall handler SHALL read `val` bytes from guest address `uaddr`
- **AND** if `timeout` is non-null, the futex SHALL return `-ENOSYS` (timeout not supported in cooperative mode)
- **AND** compare the read value with `val`: if equal, set the current task's `waiter` to `FutexWaiter { uaddr, val }` and return `Blocked`; if not equal, return `0` immediately (no blocking)
- **AND** the scheduler SHALL save the current task's context and skip it in subsequent rounds until the waiter is resolved

#### Scenario: FUTEX_WAIT with mismatched value returns immediately

- **WHEN** the guest calls `futex(uaddr, FUTEX_WAIT, val, ...)`
- **AND** the current value at `uaddr` does NOT equal `val`
- **THEN** the syscall SHALL return `0` immediately
- **AND** no waiter SHALL be created

### Requirement: futex(FUTEX_WAKE) wakes matching tasks by uaddr

The system SHALL intercept the futex syscall for `FUTEX_WAKE`. When called, all tasks whose `FutexWaiter` matches the given `uaddr` SHALL be woken (up to `val` count), and the syscall SHALL return a `Yield` result to allow the scheduler to re-evaluate runnable tasks.

#### Scenario: FUTEX_WAKE wakes matching blocked tasks

- **WHEN** the guest calls `futex(uaddr, FUTEX_WAKE, val)`
- **THEN** the system SHALL iterate the scheduler's task list
- **AND** for each blocked task whose `FutexWaiter.uaddr == uaddr`, clear its waiter and mark it runnable
- **AND** wake at most `val` tasks
- **AND** return the number of woken tasks as the futex result
- **AND** return `Yield` to allow the scheduler to dispatch the newly woken tasks

#### Scenario: FUTEX_WAKE with no matching waiters returns zero

- **WHEN** the guest calls `futex(uaddr, FUTEX_WAKE, val)`
- **AND** no task is blocked on `uaddr`
- **THEN** the system SHALL return `0` as the number of woken tasks

### Requirement: Scheduler loop round-robins runnable tasks

The system SHALL implement a scheduling loop that continuously iterates over all tasks, dispatching runnable ones and skipping blocked ones, until all tasks have finished or the main thread exits.

#### Scenario: Scheduler loop dispatches runnable tasks in order

- **WHEN** the scheduler loop runs
- **THEN** it SHALL iterate the task list in order
- **AND** for each task: if `can_dispatch()` is true, save the current context (if switching from another task), restore the selected task's context, and resume emulation
- **AND** if `can_dispatch()` is false, skip to the next task

#### Scenario: Syscall results drive scheduler decisions

- **WHEN** a syscall handler returns `Yield`
- **THEN** the scheduler SHALL save the current task's context
- **AND** move to the next runnable task
- **WHEN** a syscall handler returns `Blocked(waiter)`
- **THEN** the scheduler SHALL associate the waiter with the current task
- **AND** save the context
- **AND** skip this task until the waiter resolves
- **WHEN** a syscall handler returns `Exit(code)`
- **THEN** the scheduler SHALL mark that thread as finished
- **AND** free its context

#### Scenario: Scheduler loop terminates when main thread exits

- **WHEN** the main thread exits (sys_exit / sys_exit_group) or all threads have finished
- **THEN** the scheduler loop SHALL terminate
- **AND** return the main thread's exit code

### Requirement: Thread exit cleans up task resources

The system SHALL clean up all task-related resources (context handle, stack allocation tracking, scheduler entry) when a thread exits.

#### Scenario: Exit removes task and frees context

- **WHEN** a thread calls `sys_exit` or `sys_exit_group`
- **THEN** the system SHALL mark the thread as finished
- **AND** free its Unicorn context handle
- **AND** remove it from the scheduler's runnable set
- **AND** return `Yield` to switch to the next runnable task

### Requirement: Cooperative scheduling only (no preemption or timeslice)

The system SHALL NOT implement preemptive scheduling. Thread switches SHALL only occur at explicit syscall yield points: `futex_wait`, `futex_wake`, `sched_yield`, `exit`, and clone.

#### Scenario: No timer-based preemption

- **WHEN** a thread is running a long computation without any syscall
- **THEN** the scheduler SHALL NOT interrupt it
- **AND** no context switch SHALL occur

#### Scenario: sched_yield explicitly yields control

- **WHEN** the guest calls `sched_yield`
- **THEN** the system SHALL return `Yield`
- **AND** the scheduler SHALL save the current task's context and move to the next runnable task

### Requirement: SyscallResult extended with Yield and Blocked variants

The `SyscallResult` enum SHALL gain two new variants to support scheduler-driven execution flow.

#### Scenario: SyscallResult drives scheduler action

- **WHEN** a syscall handler produces `Yield`
- **THEN** the scheduler entry point SHALL interpret this as "save context, switch to next task"
- **WHEN** a syscall handler produces `Blocked`
- **THEN** the scheduler entry point SHALL interpret this as "save context, mark current task blocked, switch to next task"
