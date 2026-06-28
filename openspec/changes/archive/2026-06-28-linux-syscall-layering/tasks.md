## 1. kernel/ — OS 状态与语义按域搬迁

- [x] 1.1 新建 `kernel/mod.rs`，搬入 `LinuxRuntime` struct + 字段（`vfs`/`device_registry`/`fds`/`next_mmap`/`brk`/`stdout`/`exit_code`/`rng_seed`/`telemetry`）+ 构造（`new`/`with_telemetry`/`build`）+ `register_builtins` + `seed_rng`/`mount_file`/`mount_device` + `emit` + `open_path`（路径解析）+ lifecycle（`exit`）
- [x] 1.2 新建 `kernel/fd_io.rs`，`impl LinuxRuntime`：`read(fd,count)` 移动游标 / `read_at(fd,offset,count)` 不移动游标 / `write` / `ioctl` / `fstat` / `dup` / `dup3`（复用 `fd.rs` 底座，返回数据，不接收 `MemoryBridge`），其中 `write` 必须内聚处理 `stdout/stderr` 收集
- [x] 1.3 新建 `kernel/mem.rs`，`impl LinuxRuntime`：`alloc_mmap_addr(length) -> u64`（推进 `next_mmap`）/ `brk` / `munmap`
- [x] 1.4 新建 `kernel/random.rs`，`impl LinuxRuntime`：`getrandom_bytes(count) -> Vec<u8>`（xorshift PRNG）
- [x] 1.5 确认所有 kernel 方法签名只产出数据/推进 OS 状态，无 `MemoryBridge` 参数

## 2. syscall.rs — ABI 边界瘦身

- [x] 2.1 新建 `memory_bridge.rs`，定义 `MemoryBridge` trait（`read` / `write` / `map`）作为 syscall 访问 guest 内存的唯一边界
- [x] 2.2 新建 `errno.rs`，集中放置 errno 常量与 kernel/fd/device 错误到 errno 的统一映射；`syscall.rs` 保留 syscall 号常量 + `SyscallResult`，移除已搬到 `kernel/` 的 `LinuxRuntime` 定义/构造/OS 语义方法
- [x] 2.3 `dispatch` 改为 `impl LinuxRuntime` 块写在 `syscall.rs`（类型定义在 `kernel/mod.rs`，同 crate 跨文件 `impl`），调用点 `rt.linux().dispatch(...)` 保持不变，但参数收敛为单个 `&mut dyn MemoryBridge`，按 syscall 号路由到 `sys_*` handler
- [x] 2.4 `sys_*` handler 重构为"解码 `x0..x5` → 调 `rt` 的 kernel 方法拿数据 → 通过 `MemoryBridge` 回写 → 通过 `errno.rs` 编码 `SyscallResult`（bridge 回写失败固定 `EFAULT`）"，覆盖 openat/read/pread64/write/close/ioctl/fstat/mmap/getrandom/dup/dup3/exit/brk/munmap
- [x] 2.5 `sys_mmap`：匿名映射调 `rt.alloc_mmap_addr` 拿地址 + `MemoryBridge::map` 在 syscall 层落地；fd-backed 走 kernel 取 region，syscall 层必须显式完成内容回写与映射，不依赖隐式 side effect

## 3. 接线与对外兼容

- [x] 3.1 `lib.rs`：`pub mod kernel`、`pub mod errno`、`pub mod memory_bridge`，re-export `LinuxRuntime`/`SyscallResult`/errno/`MemoryBridge` 保持对外路径兼容（`LinuxRuntime` 从 `kernel` re-export，避免破坏 case-runner / jni_hook 等消费者）
- [x] 3.2 case-runner `SyscallDispatcher::on_svc` 改为提供 `MemoryBridge` 适配器，把 `GuestCPU` 的 `mem_read` / `mem_write` / `mem_map` 收敛到单一 bridge；`dispatch` 调用点形态保持 `rt.linux().dispatch(...)`
- [x] 3.3 Python binding 的 syscall 桥也改为提供 `MemoryBridge` 适配器，不再并排维护 `read_guest` / `write_guest` / `map_guest` 三闭包

## 4. 测试与验证

- [x] 4.1 kernel 层新增独立单测：`read_at`（offset 读 + 不动游标）/ `getrandom_bytes`（确定性 + 非零）/ `alloc_mmap_addr`（地址递增 + 不碰目标侧）/ `write(stdout/stderr)`（追加到 `stdout`）—— 脱离 `dispatch`/mock `MemoryBridge`
- [x] 4.2 syscall 层新增失败路径单测，校验 `errno.rs` 映射与 `EFAULT` bridge 失败优先级（至少覆盖 `read`/`ioctl`/`mmap` 各一条）
- [x] 4.3 现有 syscall 单测（含 pread64 四测）全部继续通过（行为不变）
- [x] 4.4 case 01/02/03/04 经 cli 跑通 + `cargo test --workspace` 全绿
- [x] 4.5 `openspec validate --type change linux-syscall-layering --strict` 通过
