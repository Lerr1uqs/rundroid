## Context

当前 `rundroid` 的 guest 地址空间被三套局部状态割裂：

| 入口 | 当前地址分配真相 |
|---|---|
| ELF 装载（case-runner） | `GuestRuntime.reserve_cursor = 0x4000_0000` |
| ELF 装载（Python binding） | `PyEmulatorBridge.reserve_cursor = 0x4000_0000` |
| Linux `mmap` | `LinuxRuntime.next_mmap = 0x7F_0000_0000` |
| 记账层 | `RegionTracker` 只在 backend 成功后 `register`，不参与选址 |

这意味着系统里没有单一的 guest VMA authority。loader、Python binding 与 Linux syscall 各自推进独立游标，谁都无法在分配前完整看到其它子系统已经占用的区间；冲突通常只能等 backend `mem_map` 拒绝时才暴露。该问题已经在 Python 多模块加载路径上以真实 bug 的形式出现过。

同时，现有 `linux-layering` spec 把 `alloc_mmap_addr()` 定义为“只推进 `next_mmap` 的纯状态方法”，这与“统一 VMA authority”直接冲突；`elf-runtime-interfaces` 也默认 loader 的地址空间 reserve 可以由调用方私有实现。在当前 bootstrap 阶段（ARM64 + Android + Unicorn），统一 guest 地址空间事实的改造面较大，但仍可控制在 `memory` / `case-runner` / `bindings/python` / `os/linux` / `elf/loader` 五个接缝内。

## Goals / Non-Goals

**Goals:**

- 建立 `MemoryAddressSpace` 作为 guest VMA 的第一权威，统一管理 ELF、匿名 `mmap`、fd/device `mmap`、JNI ABI、trampoline、stack、scratch 等所有 guest 地址区间。
- 把地址分配模式收敛为 `Reserved` 与 `Dynamic` 两类，并保证它们共享同一份 VMA 账本。
- overlap 必须在 backend `mem_map` 之前于地址空间层即时发现并上抛结构化错误，不再把 backend 当成主冲突检测器。
- 当前阶段采用 eager materialize：任何成功创建的 VMA 都必须立即落到 backend，账本与 backend 成功状态保持一致。
- `munmap` / `mprotect` / 查找区间 / gap search 都回写到同一份地址空间事实，为后续 `/proc/self/maps`、更准调试输出与更复杂 VMA 逻辑打基础。
- case-runner 与 Python binding 复用同一套 guest 地址空间行为，不再各自维护私有 reserve cursor。

**Non-Goals:**

- 不在本 change 引入 lazy paging、基于 page fault 的按需建图、file pager、未 materialize 的 VMA。
- 不尝试复刻 Linux kernel 内部数据结构（rb-tree / maple tree）；只对齐 VMA 语义，不追求实现形态一致。
- 不一次性实现完整 `mprotect` / `munmap` 的所有 Linux flag/edge case 语义；bootstrap 阶段只覆盖当前 rundroid 已支持的子集。
- 不改变 backend trait 的整体定位（`Backend` 仍是工厂，`Engine` 仍是会话句柄）。
- 不在本 change 内顺手扩展新的 syscall、JNI 能力或虚拟设备语义。

## Decisions

### 1. `MemoryAddressSpace` 取代 `RegionTracker` 成为第一权威

`RegionTracker` 当前只会：

- 保存已映射区间
- 检测重叠
- 根据地址查找所属区间

它不支持：

- `Dynamic` 找洞分配
- `Reserved` 固定地址预检查
- `munmap` 拆分区间
- `mprotect` 回写权限视图
- 区分不同用途的 VMA 元数据

因此本 change 不在外层继续“拿 `RegionTracker` + 多个 cursor 打补丁”，而是在 `emulator/memory` 内引入新的 `MemoryAddressSpace` 聚合根。`RegionTracker` 可以：

- 被直接替换；或
- 退化为 `MemoryAddressSpace` 的内部存储细节

但对外真相必须收敛到单一对象。

**为什么不是继续保留多个 cursor，再让 `RegionTracker` 做兜底检查？**

因为那样冲突检测仍然是事后、局部和脆弱的。只要系统里还存在多个不共享 gap search 视图的游标，overlap 与“谁该占这段地址”的问题就无法在语义层被解决。

### 2. 分配模式只保留 `Reserved` 与 `Dynamic`

地址分配模式统一为：

- `Reserved`：固定地址/固定布局策略。适用于 JNI 表、JavaVM/JNIEnv、trampoline、scratch、stack、某些固定 ABI 布局。
- `Dynamic`：由地址空间 authority 在允许范围内找洞分配。适用于 ELF image、匿名 `mmap`、无 hint 的 fd/device `mmap`。

语义上：

- `Reserved` 请求必须带目标地址；发生冲突则立即报 `Overlap`。
- `Dynamic` 请求不再允许调用方私自推进 cursor；统一由 `MemoryAddressSpace` 按 gap search 选择地址。

这里故意不使用 `reserve_any` 之类接口名，而是把“固定地址”和“动态找洞”显式建模为分配模式。这样更符合项目心智模型，也更贴近 Linux VMA 语义。

### 3. 当前阶段采用 eager materialize，不保留未落地的 VMA 状态

本 change 明确规定：

- bootstrap 阶段不支持 lazy paging / page-fault 驱动的补页 / file pager
- `MemoryAddressSpace` 成功创建的任何 VMA 都必须立即落到 backend
- backend `mem_map` 失败时，不得写入账本

因此第一版模型不引入“账本里有、backend 里没有”的持久状态。流程固定为：

```text
请求分配
  ↓
MemoryAddressSpace 做对齐 / overflow / overlap / gap search
  ↓
调用 materialize executor 执行 backend.mem_map
  ↓
成功后写入 VMA 账本
```

这样可以避免“模式”和“状态”混淆，也最符合当前 rundroid 的 fail-fast 目标。

### 4. `MemoryAddressSpace` 只依赖窄 executor，不直接依赖 backend crate

`emulator/memory` 当前明确保持“不依赖 backend，只做布局规划与记账”。这个边界值得保留，否则 `memory` 会反向耦合到底层引擎实现。

因此设计上新增一个很窄的 materialize 边界，例如：

```rust
trait MemoryMapper {
    fn map(&mut self, addr: u64, size: u64, perms: MemoryPerms) -> Result<(), MemoryError>;
    fn protect(&mut self, addr: u64, size: u64, perms: MemoryPerms) -> Result<(), MemoryError>;
    fn unmap(&mut self, addr: u64, size: u64) -> Result<(), MemoryError>;
}
```

`MemoryAddressSpace` 持有或临时接收该 executor，负责把“预检查 → materialize → 成功后记账”封装为原子流程。

这样做的好处：

- `memory` 仍不依赖 `backend` crate
- 纯单测可以用 mock executor 验证选址/分裂/错误传播
- 上层（case-runner、Python binding、syscall 层）不再需要手写“先算地址、再调 backend、再记账”的重复模板

### 5. VMA 账本必须携带足够元数据，而不是只有 `addr + size`

第一版 `MemoryRegion` 只记录：

- `addr`
- `size`
- `origin`

这对统一 authority 不够。新 VMA 记录至少需要：

- `start`
- `size` 或 `end`
- `perms`
- `allocation_mode`（`Reserved` / `Dynamic`）
- `usage`（如 `ELFImage` / `AnonymousMmap` / `FileMmap` / `JNIEnv` / `JavaVM` / `Trampoline` / `Stack` / `Scratch`）

其中 `usage` 很关键。它不仅服务调试输出，还直接影响错误报告与未来 `/proc/self/maps` 风格可视化。

### 6. `Dynamic` 使用统一 gap search，替换各处 bump cursor

当前实现里的三个地址推进策略：

- `case-runner.reserve_cursor`
- Python binding `reserve_cursor`
- `LinuxRuntime.next_mmap`

都要被收掉，改由 `MemoryAddressSpace::allocate(Dynamic, ...)` 统一选址。

算法上，bootstrap 阶段不需要追求 Linux kernel 等级的数据结构。使用有序 `Vec` 或 `BTreeMap` 做：

- 对齐
- 顺序 gap search（first-fit 或 next-fit）
- 查找/插入/分裂

已经足够。

我倾向第一版采用 **first-fit + per-usage 默认起始 hint/range**：

- ELF image：默认低地址镜像区起点（保留当前 `0x4000_0000` 作为 hint，而不是硬真相）
- anonymous/file/device `mmap`：默认高地址 mmap arena 起点（保留当前 `0x7F_0000_0000` 作为 hint）
- 但最终地址选择仍必须经过统一账本检查

这保留了当前布局习惯，同时把“固定常量”降级为 hint。

### 7. Linux kernel 层不再持有 guest VMA 真相

现有 `linux-layering` spec 中，`alloc_mmap_addr()` 被定义为“只推进 `next_mmap` 的纯状态方法”。在统一 VMA authority 之后，这个 requirement 必须修改。

新的职责拆分应为：

- kernel 仍决定 mmap 语义、长度、prot、flags、fd/device region 内容
- 但 guest 地址选择不再由 `LinuxRuntime.next_mmap` 自己推进
- syscall 层或 runtime 装配层通过共享的 `MemoryAddressSpace` 请求 `Dynamic` 分配，再调用 backend materialize

也就是说，Linux kernel 保留“语义 authority”，但不再保留“guest 地址真相”。

### 8. ELF loader 契约改为“共享地址空间 authority”，不是“私有 reserve cursor”

现有 `LoadContext::reserve_image_space()` 注释写的是：

> 实现负责保证返回的区间与已映射区不重叠。

但实际各实现都只是拿自己的 cursor 去 bump，再让 backend 去兜底。

新契约要更强：

- loader 只能通过共享 `MemoryAddressSpace` 请求镜像 footprint 的 `Dynamic` 分配
- 这一步必须已经完成 overlap 预检查与 backend materialize
- `map_segment()` 不再自己做独立 address authority，只处理 footprint 内的数据写入与零填充

这能保证 case-runner 与 Python binding 的 loader 行为一致，也能把多模块装载冲突提前暴露。

### 9. `munmap` / `mprotect` 进入统一地址空间账本主线

如果只统一 `mem_map`，不统一 `unmap/protect`，很快又会出现地址事实漂移。

因此第一版 `MemoryAddressSpace` 必须至少支持：

- `protect(addr, size, perms)`：更新 VMA 权限视图并调用 backend
- `unmap(addr, size)`：支持完整移除与中间拆分，并调用 backend

这里不要求第一版完整复刻 Linux 所有细节，但必须让账本与 backend 共同演进。否则后续 `Dynamic` gap search 会错误复用已经被 `munmap` 释放的洞，或者忽略权限变化。

### 10. 实施顺序按“先 authority、再消费方接线、最后 syscall 追平”推进

为了降低改造风险，建议分三层推进：

1. 在 `emulator/memory` 里实现 `MemoryAddressSpace` 与测试
2. 让 `case-runner` / Python binding / ELF loader 先接入，消除多模块装载与双 cursor 问题
3. 再把 `LinuxRuntime.next_mmap` 收掉，让 `sys_mmap/munmap/mprotect` 接入统一 authority

这样可以先解决当前最直接的装载冲突问题，再逐步把 Linux 内存路径纳入同一条主线。

## Risks / Trade-offs

- **[跨模块改造面大]** → 缓解：先在 `memory` crate 建立稳定接口，再分层替换 case-runner / Python binding / linux consumers，避免一次性重写所有路径。
- **[spec 与现有 `linux-layering` 冲突]** → 缓解：本 change 同步修改 `linux-layering` requirement，明确 kernel 不再持有独立 VMA 真相。
- **[实现中把 `memory` 反向耦合到 backend]** → 缓解：通过窄 executor trait 适配 materialize，不直接依赖 `Engine`。
- **[first-fit 造成地址布局与 unidbg/现状不完全一致]** → 缓解：保留 arena hint，使布局大体稳定；spec 只约束“不重叠、同一 authority、即时错误”，不约束具体 gap search 算法。
- **[`munmap`/`mprotect` 第一版行为不全]** → 缓解：spec 明确只要求当前支持子集的账本一致性，不承诺完整 Linux 全语义。
- **[Python binding 与 case-runner 行为再次分叉]** → 缓解：把地址空间 authority 放在共享 crate，不允许 binding 侧继续复制一份局部 cursor 逻辑。
- **[名称迁移噪声]** → 缓解：可在过渡期保留 `RegionTracker` 别名或兼容 re-export，但所有新调用都必须指向 `MemoryAddressSpace`。
