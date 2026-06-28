## Why

`emulator/os/linux` crate 的 `syscall.rs` 把三层职责混在一起：syscall ABI 边界（syscall 号/errno/`dispatch`/`sys_*` 参数解码）、OS 状态（`LinuxRuntime` 持有 vfs/fds/mmap 区/brk/stdout/rng_seed）、OS 语义实现（设备注册、路径解析、mmap 地址分配、getrandom PRNG、mount API）。随着 syscall 集持续扩张（socket/procfs/信号/...），`syscall.rs` 会膨胀成几千行巨石，且 OS 语义无法脱离 syscall 号/寄存器/目标侧内存独立测试。现在 crate 还小（4 文件），拆分成本最低。

## What Changes

把 `os/linux/src/syscall.rs` 按职责边界拆成 `kernel/` 子模块目录 + `syscall.rs`（不拆 crate，bootstrap 阶段避免过度分割）：

- **新增 `kernel/` 目录**，按 OS 子系统域组织（方法按域分文件 `impl LinuxRuntime`）：
  - `kernel/mod.rs`：`LinuxRuntime` 聚合根（struct + 字段）+ 构造（new/with_telemetry/build）+ 设备注册 `register_builtins` + 配置 API `seed_rng`/`mount_file`/`mount_device` + `emit` + 路径解析 `open_path` + lifecycle（exit）
  - `kernel/fd_io.rs`：fd IO 语义方法 `read`/`read_at`/`write`/`ioctl`/`fstat`/`dup`/`dup3`
  - `kernel/mem.rs`：内存管理 `alloc_mmap_addr`/`brk`/`munmap`
  - `kernel/random.rs`：PRNG `getrandom_bytes`
  - 所有 kernel 方法**只产出数据/纯状态，不接收 `write_guest`/`map_guest` 闭包**，不碰目标侧内存。
- **瘦身 `syscall.rs`**：只做 ABI 边界——syscall 号常量 + errno + `SyscallResult` + `dispatch` + 各 `sys_*` handler。handler 职责限定为：解码 `x0..x5` → 调 `LinuxRuntime`(kernel) 的 OS 方法拿数据 → `write_guest`/`map_guest` 把结果落地到目标侧 → 编码 `SyscallResult`（回写失败即 EFAULT）。
- **`fd.rs` / `vfs.rs` 不变**。
- **pure refactor**：所有现有 syscall 单测（含 pread64 四个）+ case（01/02/03/04）必须继续通过，不改变任何对外行为。`runtime-correctness-hardening` 已确立的 "source → 目标侧回写 → 返回值" 主线落到 syscall 这一层。

## Capabilities

### New Capabilities

- `linux-layering`: linux crate 内 kernel（OS 状态 + 语义，按子系统域组织）与 syscall（ABI 边界 + 目标侧回写）的职责分离约束——OS 层只产出数据不碰目标侧内存，syscall 层负责回写与返回值编码，kernel 内部按 OS 子系统域（fd IO / mem / random / ...）分文件。

### Modified Capabilities

（无。本 change 是内部结构 refactor，不改任何对外行为；`runtime-correctness` / `dependency-linking` 的 Requirement 在拆分后仍满足，spec 不变。）

## Impact

- **`emulator/os/linux/src/syscall.rs`**：拆分，瘦身到 ABI 边界。
- **`emulator/os/linux/src/kernel/`**：新增子模块目录（`mod.rs`/`fd_io.rs`/`mem.rs`/`random.rs`），承接 `LinuxRuntime` + OS 语义方法。
- **`emulator/os/linux/src/lib.rs`**：模块声明 + re-export 调整。
- **`emulator/case-runner/src/runtime.rs`**：`SyscallDispatcher` 调用 `dispatch` 的签名保持不变（行为不变）。
- **测试**：OS 语义方法（`read_at` / `getrandom_bytes` / `alloc_mmap_addr`）可在 kernel 层独立单测（脱离 syscall 号/寄存器/mock 闭包）；现有 syscall 单测与 case 保持绿。
