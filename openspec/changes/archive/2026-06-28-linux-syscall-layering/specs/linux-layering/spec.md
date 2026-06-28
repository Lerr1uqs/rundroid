## ADDED Requirements

### Requirement: Kernel owns OS state and semantics without touching target memory

linux crate SHALL 把 Linux OS 状态（VFS 挂载表、fd 表、mmap 区游标、brk、stdout、PRNG 种子、设备注册表）与纯语义方法收敛到 `kernel` 模块，且这些方法 SHALL 只产出数据（如 `Vec<u8>` / `usize` / `u64`）或推进 OS 状态，不接收 `MemoryBridge`。`MemoryBridge` 是 syscall 边界注入的 guest 内存访问抽象，底层通常落到 engine / `GuestCPU::mem_read` / `mem_write` / `mem_map`，不是 Python 专属概念。

#### Scenario: Read semantics produce bytes without target-side write

- **WHEN** kernel 层执行 `read` / `read_at`（pread）语义
- **THEN** 它 SHALL 返回读到的字节（`Vec<u8>`）与读取长度
- **AND** 不 SHALL 接收或调用 `MemoryBridge`

#### Scenario: mmap address allocation is pure state

- **WHEN** kernel 层为匿名 mmap 分配地址
- **THEN** 它 SHALL 仅推进 `next_mmap` 状态并返回地址
- **AND** 不 SHALL 调用目标侧 `mem_map`（目标侧映射由 syscall 层经 `MemoryBridge` 落地）

#### Scenario: getrandom produces bytes without target-side write

- **WHEN** kernel 层执行 getrandom 语义
- **THEN** 它 SHALL 返回随机字节（`Vec<u8>`）
- **AND** 不 SHALL 把字节直接写入目标侧缓冲

### Requirement: Kernel organized by OS subsystem domain

linux crate 的 kernel 模块 SHALL 按 OS 子系统域（fd IO / 内存管理 / 随机数 / 路径解析 / 设备注册）分文件组织 OS 语义方法，而不是单文件堆叠。`LinuxRuntime` 作为聚合根，各域方法以分文件 `impl LinuxRuntime`（同 crate 跨文件 impl）实现，对调用方只暴露聚合方法。`stdout/stderr` 收集 SHALL 归属 kernel 的 `write` 语义，而不是 syscall 层单独分支。

#### Scenario: fd IO and memory management live in separate files

- **WHEN** kernel 内承载 fd IO 语义（read/read_at/write/ioctl/fstat/dup）或内存管理语义（mmap 地址分配/brk/munmap）
- **THEN** 它们 SHALL 落在按域划分的独立文件（如 `fd_io.rs` / `mem.rs`）
- **AND** 不 SHALL 全部堆在单一 kernel 文件里

#### Scenario: LinuxRuntime aggregates subsystem domains

- **WHEN** syscall 层或测试访问 OS 语义
- **THEN** 它 SHALL 通过 `LinuxRuntime` 聚合根调用（如 `rt.read_at` / `rt.alloc_mmap_addr` / `rt.getrandom_bytes`）
- **AND** 各域实现细节分布在子系统文件中，聚合方法对外统一

#### Scenario: stdout collection is part of kernel write semantics

- **WHEN** OS 层执行 `write(fd, data)` 且 `fd` 为 `1` 或 `2`
- **THEN** 它 SHALL 把字节追加到 `LinuxRuntime.stdout`
- **AND** syscall 层不 SHALL 再保留独立的 stdout/stderr 写分支

### Requirement: Syscall layer carries ABI boundary and target-side writeback

linux crate 的 `syscall` 模块 SHALL 限定为 ABI 边界：解码寄存器参数、调用 `kernel` 的 OS 方法、通过 `MemoryBridge` 把结果回写到目标侧内存、编码 `SyscallResult`。回写失败 SHALL 上抛 `EFAULT`。

#### Scenario: Handler writes back through target-side closures

- **WHEN** `sys_*` handler 从 kernel OS 方法拿到数据
- **THEN** 它 SHALL 通过 `MemoryBridge` 把数据落地到目标侧
- **AND** 回写失败时 SHALL 返回 `EFAULT`（不允许"返回长度但目标缓冲没变"的假成功）

#### Scenario: Dispatch is the ABI entry point

- **WHEN** backend 在 `svc` 时进入 syscall 分派
- **THEN** `dispatch` SHALL 按 syscall 号解码参数并路由到对应 handler
- **AND** handler SHALL 经 kernel OS 方法获取数据，再完成目标侧回写

#### Scenario: mmap content writeback does not rely on implicit map side effects

- **WHEN** `sys_mmap` 处理 fd-backed/device-backed 映射且 kernel 返回初始 `region.content`
- **THEN** syscall 层 SHALL 先建立目标侧映射，再通过 `MemoryBridge` 把内容字节显式落地到返回地址
- **AND** 不 SHALL 依赖 `map_guest` 自带写内容的隐式副作用

### Requirement: Syscall boundary uses a single MemoryBridge abstraction

linux crate SHALL 定义单一 `MemoryBridge` trait 作为 syscall 层访问 guest 内存的唯一抽象，至少包含 `read` / `write` / `map` 三个方法；`dispatch` 与各 `sys_*` handler SHALL 接收该 trait，而不是三个独立闭包或完整 `GuestCPU`。

#### Scenario: Dispatch receives one bridge instead of three closures

- **WHEN** syscall 层实现 `dispatch`
- **THEN** 它 SHALL 接收 `&mut dyn MemoryBridge` 或等价单一 bridge 参数
- **AND** 不 SHALL 继续暴露 `read_guest` / `write_guest` / `map_guest` 三闭包签名

#### Scenario: MemoryBridge stays minimal

- **WHEN** 为 syscall 层设计 guest 内存访问边界
- **THEN** `MemoryBridge` SHALL 只暴露 guest 内存读/写/映射三类能力
- **AND** 不 SHALL 扩张为包含寄存器访问、`stop()` 或其他 `GuestCPU` hook 能力的胖接口

### Requirement: Errno mapping is centralized

linux crate SHALL 通过 `errno.rs` 集中定义 errno 常量与 kernel/底座错误到 errno 的映射，避免在 `syscall` 各 handler 内分散硬编码错误码选择。

#### Scenario: Handler uses centralized errno mapping

- **WHEN** `sys_*` handler 需要把 kernel 或 fd/device 底座错误编码成返回值
- **THEN** 它 SHALL 经由 `errno.rs` 的常量或映射函数完成编码
- **AND** 不 SHALL 在多个 handler 中重复散落 `Err(_) => EINVAL/EBADF/...` 规则

### Requirement: Refactor preserves observable behavior

拆分 SHALL 是 pure refactor，不改变任何对外可观察行为（返回值、errno、目标侧可见状态、fd 生命周期、VFS 解析）。

#### Scenario: Existing syscall tests and cases stay green

- **WHEN** 现有 syscall 单测（含 pread64 四测）与 case `01/02/03/04` 在拆分后运行
- **THEN** 它们 SHALL 全部继续通过
- **AND** 返回值 / errno / 目标侧可见状态 SHALL 与拆分前一致

### Requirement: OS semantics testable at kernel layer

kernel 层的 OS 语义方法 SHALL 可脱离 syscall 号、寄存器参数、mock `MemoryBridge` 独立单测。

#### Scenario: read_at tested without dispatch scaffolding

- **WHEN** 为 pread64 的 `read_at` 语义编写单测
- **THEN** 它 SHALL 能直接调用 kernel 方法（如 `rt.read_at(fd, offset, count)`）
- **AND** 不 SHALL 需要构造 `dispatch(SYS_PREAD64, ...)` + mock `MemoryBridge`
