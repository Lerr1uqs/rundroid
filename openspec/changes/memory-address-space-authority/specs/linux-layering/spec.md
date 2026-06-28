## MODIFIED Requirements

### Requirement: Kernel owns OS state and semantics without touching target memory

linux crate SHALL 把 Linux OS 状态（VFS 挂载表、fd 表、stdout、PRNG 种子、设备注册表、`mmap`/`brk`/`munmap` 的 OS 语义）与纯语义方法收敛到 `kernel` 模块，且这些方法 SHALL 只产出数据（如 `Vec<u8>` / `usize` / `u64`）或推进 OS 状态，不接收 `MemoryBridge`。`MemoryBridge` 是 syscall 边界注入的 guest 内存访问抽象，底层通常落到 engine / `GuestCPU::mem_read` / `mem_write` / `mem_map`，不是 Python 专属概念。guest 地址空间的最终 VMA 真相 SHALL 由共享的 `MemoryAddressSpace` authority 持有，而不是由 `LinuxRuntime` 维护第二份独立地址真相。

#### Scenario: Read semantics produce bytes without target-side write

- **WHEN** kernel 层执行 `read` / `read_at`（pread）语义
- **THEN** 它 SHALL 返回读到的字节（`Vec<u8>`）与读取长度
- **AND** 不 SHALL 接收或调用 `MemoryBridge`

#### Scenario: mmap address selection uses shared address space authority

- **WHEN** kernel 层参与匿名 `mmap` 或 fd/device `mmap` 语义
- **THEN** 它 SHALL 把长度、权限、flags、backing 内容等 OS 语义结果交给共享的 guest 地址空间 authority 做地址选择与区间冲突判定
- **AND** 不 SHALL 通过独立的 `next_mmap` 或等价私有游标维持第二份 guest VMA 真相

#### Scenario: getrandom produces bytes without target-side write

- **WHEN** kernel 层执行 getrandom 语义
- **THEN** 它 SHALL 返回随机字节（`Vec<u8>`）
- **AND** 不 SHALL 把字节直接写入目标侧缓冲
