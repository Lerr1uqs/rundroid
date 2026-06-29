## ADDED Requirements

### Requirement: SyscallResult carries scheduler-relevant variants

`SyscallResult` SHALL be extended with `Yield` and `Blocked(Box<dyn Waiter>)` variants beyond the existing `Done(u64)` and `Exit(i32)`. The scheduler entry point consuming `SyscallResult` SHALL interpret these variants to drive context save/restore and task selection.

#### Scenario: Yield variant triggers context switch

- **WHEN** a syscall handler returns `Yield`
- **THEN** the consumer (scheduler) SHALL save the current task's full register context
- **AND** move to the next runnable task without advancing the yielding task's PC past the syscall instruction

#### Scenario: Blocked variant marks task as non-dispatchable

- **WHEN** a syscall handler returns `Blocked(waiter)`
- **THEN** the consumer (scheduler) SHALL save the current task's context
- **AND** associate the waiter with the task
- **AND** skip this task in future rounds until `waiter.can_dispatch()` returns true

### Requirement: Kernel may hold scheduler state

The `kernel` module (or its parent `LinuxRuntime` aggregate) MAY hold a `ThreadScheduler` instance as part of the OS state, enabling syscall handlers to access the scheduler for `add_thread`, `wake_matching`, and other thread-management operations. This SHALL NOT break the kernel's ability to be tested without a full scheduler (the scheduler field MAY be optional or behind a feature gate).

#### Scenario: Syscall clone handler accesses scheduler

- **WHEN** `sys_clone` runs in the syscall layer
- **THEN** it SHALL call the kernel-level scheduler's `add_thread` method to register the child task
- **AND** non-thread-related kernel tests SHALL NOT require a scheduler instance
