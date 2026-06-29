## ADDED Requirements

### Requirement: Scheduler loop integrated at assembly layer

The runtime assembly layer (case-runner or equivalent emulator entry point) SHALL integrate the scheduler loop as the outer execution driver instead of a single `emulate()` call. The scheduler loop SHALL wrap the syscall dispatch cycle to support context save/restore across multiple threads.

#### Scenario: Scheduler loop drives execution

- **WHEN** the runtime starts executing guest code
- **THEN** it SHALL enter the scheduler loop rather than calling `emulate()` directly
- **AND** the loop SHALL iterate over all registered threads, dispatching runnable ones
- **AND** the loop SHALL terminate only when the main thread has exited

#### Scenario: Single-threaded workloads run correctly under scheduler

- **WHEN** a single-threaded .so is loaded and executed
- **THEN** the scheduler SHALL operate correctly with only the main task
- **AND** the behavior SHALL be identical to the non-scheduler execution path for all observable outcomes (return values, guest memory state, stdout)

### Requirement: Emulator entry point can run with or without scheduler

The emulator entry point SHALL support both a scheduler-driven path (for multi-threaded workloads) and a direct `emulate()` path (for simple single-threaded scenarios) without requiring the caller to restructure their code. The scheduler SHALL be opt-in.

#### Scenario: Scheduler is opt-in

- **WHEN** a caller does not configure a scheduler
- **THEN** the runtime SHALL use the existing direct `emulate()` path
- **AND** no scheduler overhead SHALL be introduced

#### Scenario: Scheduler is configured for multi-thread workloads

- **WHEN** a caller enables the scheduler
- **THEN** the runtime SHALL use the scheduler loop
- **AND** syscall handlers for clone/futex/sched_yield SHALL interact with the scheduler
