## 1. kernel/ — OS 状态与语义按域搬迁

- [ ] 1.1 新建 `kernel/mod.rs`，搬入 `LinuxRuntime` struct + 字段（`vfs`/`device_registry`/`fds`/`next_mmap`/`brk`/`stdout`/`exit_code`/`rng_seed`/`telemetry`）+ 构造（`new`/`with_telemetry`/`build`）+ `register_builtins` + `seed_rng`/`mount_file`/`mount_device` + `emit` + `open_path`（路径解析）+ lifecycle（`exit`）
- [ ] 1.2 新建 `kernel/fd_io.rs`，`impl LinuxRuntime`：`read(fd,count)` 移动游标 / `read_at(fd,offset,count)` 不移动游标 / `write` / `ioctl` / `fstat` / `dup` / `dup3`（复用 `fd.rs` 底座，返回数据，不接收回写闭包）
- [ ] 1.3 新建 `kernel/mem.rs`，`impl LinuxRuntime`：`alloc_mmap_addr(length) -> u64`（推进 `next_mmap`）/ `brk` / `munmap`
- [ ] 1.4 新建 `kernel/random.rs`，`impl LinuxRuntime`：`getrandom_bytes(count) -> Vec<u8>`（xorshift PRNG）
- [ ] 1.5 确认所有 kernel 方法签名只产出数据/推进纯状态，无 `write_guest`/`map_guest` 参数

## 2. syscall.rs — ABI 边界瘦身

- [ ] 2.1 保留 syscall 号常量 + errno + `SyscallResult`，移除已搬到 `kernel/` 的 `LinuxRuntime` 定义/构造/OS 语义方法
- [ ] 2.2 `dispatch` 改为 `impl LinuxRuntime` 块写在 `syscall.rs`（类型定义在 `kernel/mod.rs`，同 crate 跨文件 `impl`），签名 `&mut self` 不变（调用点零改动），按 syscall 号路由到 `sys_*` handler
- [ ] 2.3 `sys_*` handler 重构为"解码 `x0..x5` → 调 `rt` 的 kernel 方法拿数据 → `write_guest`/`map_guest` 回写 → 编码 `SyscallResult`（回写失败 `EFAULT`）"，覆盖 openat/read/pread64/write/close/ioctl/fstat/mmap/getrandom/dup/dup3/exit/brk/munmap
- [ ] 2.4 `sys_mmap`：匿名映射调 `rt.alloc_mmap_addr` 拿地址 + `map_guest` 在 syscall 层落地；fd-backed 走 kernel 取 region，内容回写 + `map_guest` 在 syscall 层

## 3. 接线与对外兼容

- [ ] 3.1 `lib.rs`：`pub mod kernel`，re-export `LinuxRuntime`/`SyscallResult`/errno 保持对外路径兼容（`LinuxRuntime` 从 `kernel` re-export，避免破坏 case-runner / jni_hook 等消费者）
- [ ] 3.2 确认 case-runner `SyscallDispatcher::on_svc` 调 `rt.linux().dispatch(...)` 调用点无需改动（`dispatch` 签名不变）

## 4. 测试与验证

- [ ] 4.1 kernel 层新增独立单测：`read_at`（offset 读 + 不动游标）/ `getrandom_bytes`（确定性 + 非零）/ `alloc_mmap_addr`（地址递增 + 不碰目标侧）—— 脱离 `dispatch`/mock 闭包
- [ ] 4.2 现有 syscall 单测（含 pread64 四测）全部继续通过（行为不变）
- [ ] 4.3 case 01/02/03/04 经 cli 跑通 + `cargo test --workspace` 全绿
- [ ] 4.4 `openspec validate --type change linux-syscall-layering --strict` 通过
