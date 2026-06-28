## Context

`emulator/os/linux/src/syscall.rs` 当前是三层职责混合的大杂烩：

| 层次 | 当前在 syscall.rs 里的内容 |
|---|---|
| syscall ABI 边界 | syscall 号常量、errno、`SyscallResult`、`dispatch(nr, x0..x5, ...)`、各 `sys_*` 参数解码 |
| OS 状态 | `LinuxRuntime{vfs, device_registry, fds, next_mmap, brk, stdout, exit_code, rng_seed, telemetry}` |
| OS 语义实现 | `register_builtins`（设备注册）、`sys_openat` 内联 VFS 路径解析、`sys_mmap` 内联地址分配、`sys_getrandom` 内联 xorshift PRNG、`seed_rng`/`mount_file`/`mount_device` |

后果：随 syscall 集扩张，`syscall.rs` 膨胀成巨石；OS 语义无法脱离 syscall 号/寄存器/mock 目标侧闭包独立测试；回写职责（`write_guest`/`map_guest`）与 OS 语义交织，`runtime-correctness-hardening` 确立的 "source → 目标侧回写 → 返回值" 主线没有清晰落点。

crate 当前还小（`syscall.rs`/`fd.rs`/`vfs.rs`/`lib.rs` 共 ~1700 行），现在拆分成本最低。本 change 一步到位把 kernel 按子系统域拆成目录，避免先建扁平 `kernel.rs` 再返工。

## Goals / Non-Goals

**Goals:**

- 把 `syscall.rs` 拆成 `kernel/` 子模块目录（按 OS 子系统域）+ `syscall.rs`（ABI 边界 + 目标侧回写）。
- kernel 的 OS 语义方法只产出数据（`Vec<u8>`/`usize`/`u64`）/推进纯状态，不接收 `write_guest`/`map_guest`。
- syscall handler 职责限定为：解码参数 → 调 kernel OS 方法 → 目标侧回写 → 编码 `SyscallResult`。
- kernel 层 OS 语义可脱离 dispatch 单测。
- pure refactor：对外行为、`fd.rs`/`vfs.rs`、现有测试/case 全部不变。

**Non-Goals:**

- 不拆 crate（bootstrap 阶段避免过度分割）。
- 不改 `fd.rs` / `vfs.rs` / `rundroid-driver`。
- 不新增 syscall（不加 socket/procfs/...，那是后续 change）。
- 不改对外公共 API 的可观察行为（`LinuxRuntime::dispatch` 调用点签名保持兼容）。
- 不重构 `fd.rs` 内部的 `read_from_fd`/`pread_from_fd` 等底座函数（kernel 复用它们）。

## Decisions

### 1. kernel 按子系统域组织为目录

```
emulator/os/linux/src/
├── syscall.rs      # ABI 边界（syscall 号/errno/SyscallResult/dispatch/sys_* handler）
├── fd.rs           # fd 表底座（不变）
├── vfs.rs          # VFS 挂载表（不变）
└── kernel/
    ├── mod.rs      # LinuxRuntime 聚合根 + 构造 + register_builtins + seed_rng/mount_*/emit + open_path + lifecycle(exit)
    ├── fd_io.rs    # impl LinuxRuntime: read/read_at/write/ioctl/fstat/dup/dup3
    ├── mem.rs      # impl LinuxRuntime: alloc_mmap_addr/brk/munmap
    └── random.rs   # impl LinuxRuntime: getrandom_bytes
```

**按 OS 子系统域分文件，不按 syscall 号分**——syscall 号是 ABI 层的事；kernel 层按语义域组织（类比 Linux 内核的 mm/fs/drivers 子系统）。每个域文件承载该域的 OS 语义方法。

### 2. 拆分粒度：fd_io / mem / random / mod

选这个粒度的理由：

- **fd_io.rs**：fd 上的 IO 语义方法最多（read/read_at/write/ioctl/fstat/dup/dup3），内聚于"操作 fd 表 + IO"，独立成文件最自然。
- **mem.rs**：内存管理（mmap 地址分配/brk/munmap）是独立 OS 子系统，且 mmap 逻辑较厚。
- **random.rs**：PRNG 是独立状态机（`rng_seed`），未来可换策略（如真实 `/dev/urandom` 回源），独立文件留演进位。
- **mod.rs**：聚合根 + 构造期配置（register_builtins/seed/mount）+ 路径解析（open_path，桥接 vfs+fd）+ lifecycle（exit）。这些是 OS 核心入口/配置，归聚合根文件。

不再细拆（如每个 syscall 一个文件）= 避免 over-fragmentation；不再粗（单 kernel.rs）= 失去分层意义。4 文件是 bootstrap 阶段的平衡点，未来加 socket/signal/procfs 时新增对应域文件即可。

### 3. OS 方法返回数据，不接收回写闭包

kernel 的 `read`/`read_at`/`getrandom_bytes` 返回 `Vec<u8>`，`alloc_mmap_addr` 返回 `u64`——**不接收** `write_guest`/`map_guest`。理由：

- OS 层变纯语义/纯数据源，可脱离 mock 闭包独立测试。
- 目标侧落地（回写）统一收敛到 syscall 层一个地方，呼应 runtime-correctness 的主线。
- 备选方案"OS 方法直接拿回写闭包"会让两层再次耦合，被否决。

### 4. mmap 拆"地址分配"与"目标侧映射"

`kernel::alloc_mmap_addr(length) -> u64`（在 `mem.rs`）只推进 `next_mmap` 状态、返回地址；目标侧 `mem_map`（通过 `map_guest`）由 syscall 层在拿到地址后调用。fd-backed/device-backed mmap 的 region 内容获取走 kernel（读 device.mmap），内容回写走 syscall 层。

### 5. `dispatch` 实现落 syscall 层，调用点签名不变

`dispatch` 是 ABI 入口，逻辑属于 syscall 层。但为了**调用点零改动**（case-runner `SyscallDispatcher::on_svc` 调 `linux.dispatch(...)`），保留 `LinuxRuntime::dispatch(&mut self, ...)` 方法签名——方法体以 `impl LinuxRuntime` 块写在 `syscall.rs`（Rust 允许同 crate 跨文件 impl），内部路由到 syscall 层 handler 函数。`LinuxRuntime` 类型定义本身在 `kernel/mod.rs`。

### 6. 扁平字段 + 分文件 `impl`，不引入子状态 struct

`LinuxRuntime` 字段保持扁平（`next_mmap`/`brk`/`rng_seed` 等直接是 struct 字段），各域方法以分文件 `impl LinuxRuntime` 实现。不引入 `MemState`/`RandomState` 等子 struct——bootstrap 阶段字段少，扁平更直读；子 struct 是未来某域状态真正复杂（如 mm 子系统有 VMA 树）时再引入。

### 7. 不拆 crate

bootstrap 阶段 `rundroid-linux` 内模块边界足够；拆 crate（如 `rundroid-linux-syscall` + `rundroid-linux-kernel`）是过度分割，增加依赖图复杂度而无收益。留作未来 syscall 集真正庞大时再评估。

## Risks / Trade-offs

- **[行为漂移]** 搬运 OS 语义时回写/errno/游标逻辑出错 → 缓解：现有 syscall 单测（含 pread64 四测）+ case 01/02/03/04 作为不变性守门；新增 kernel 层单测与原 dispatch 测试对照。
- **[回写职责遗漏]** 某 handler 重构后忘了回写或回写位置错 → 缓解：spec SHALL 约束 + review 检查每个 handler 的回写路径。
- **[拆分粒度争议]** fd_io/mem/random 的划分可能被认为过细或过粗 → 缓解：按 OS 域天然边界，每域语义内聚；random 单独留 PRNG 策略演进位。粒度可后续按实际增量调整。
- **[跨文件 impl 可读性]** `LinuxRuntime` 的 impl 散在 4+ 文件 → 缓解：聚合方法（syscall 层调用的入口）集中在 `mod.rs` 或域文件头部，域内部细节在域文件；`dispatch` 单一入口在 syscall.rs。
- **[过度设计风险]** 拆分可能被延伸成"顺手改 fd.rs/vfs.rs/加新 syscall" → 缓解：Non-Goals 明确边界，本 change 只做结构搬运。
