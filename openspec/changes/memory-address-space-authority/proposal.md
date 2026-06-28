## Why

当前 `rundroid` 的 guest 地址空间没有单一权威：ELF 装载依赖 `case-runner` / Python binding 各自维护的 `reserve_cursor`，Linux `mmap` 依赖 `LinuxRuntime.next_mmap`，而 `RegionTracker` 只做事后记账，不参与选址与冲突判定。结果是地址分配语义分裂、重叠检测滞后到 backend `mem_map`、不同入口对同一 guest VMA 的理解不一致，已经暴露出多模块装载冲突这类真实 bug。现在项目仍处于 ARM64 + Android + Unicorn 的 bootstrap 阶段，引入统一地址空间 authority 的成本最低。

## What Changes

- 新增 `MemoryAddressSpace` 作为 guest VMA 的第一权威，统一管理 ELF、匿名 `mmap`、fd/device `mmap`、JNI ABI 表、trampoline、scratch、stack 等所有 guest 地址区间。
- 把地址分配模式统一为两类：`Reserved`（固定地址/固定布局）与 `Dynamic`（由地址空间管理器找洞分配）。
- 明确任何成功分配出的 VMA 都必须立即 materialize 到 backend；bootstrap 阶段不支持 lazy paging、按 page fault 补页、file pager、未 materialize 的 VMA。
- overlap 必须在 `MemoryAddressSpace` 内于 backend `mem_map` 之前即时报错；不再依赖 backend 作为主冲突检测路径。
- `munmap` / `mprotect` / 已映射区间查询必须回写到同一份 VMA 账本，确保后续分配、调试输出与 `/proc/self/maps` 风格能力建立在一致事实之上。
- **BREAKING**：`LinuxRuntime` 不再维护独立的 `next_mmap` 地址真相；它只能向统一的 `MemoryAddressSpace` 请求 `Dynamic` 分配。
- **BREAKING**：ELF loader `LoadContext` 的 guest 地址空间保留契约将从“局部 reserve 游标”升级为“经统一地址空间 authority 进行预检查 + materialize”。

## Capabilities

### New Capabilities

- `guest-address-space`: 定义 `MemoryAddressSpace` 作为 guest VMA 的唯一权威，覆盖 `Reserved`/`Dynamic` 分配、overlap 即时报错、eager materialize、`munmap`/`mprotect` 回写与统一 region 视图。

### Modified Capabilities

- `elf-runtime-interfaces`: loader 的地址空间保留与段映射 requirement 将改为依赖共享的 guest 地址空间 authority，而不是各调用方自有 reserve cursor。
- `linux-layering`: kernel 的 `mmap` 地址分配 requirement 将改为通过统一 `MemoryAddressSpace` 选址；`LinuxRuntime` 不再拥有独立的 guest VMA 真相。

## Impact

- **`emulator/memory`**：现有 `RegionTracker` 需要演进为或被替换为 `MemoryAddressSpace`，新增 gap search、fixed/dynamic 分配、`protect`/`unmap` 账本更新能力。
- **`emulator/case-runner/src/runtime.rs`**：删掉 `reserve_cursor` 主逻辑，ELF loader、JNI ABI、scratch/stack/trampoline 都改经共享地址空间 authority 分配与 materialize。
- **`emulator/bindings/python/src/lib.rs`**：删掉私有 `reserve_cursor` 语义，Python binding 与 case-runner 复用同一套 guest 地址空间行为。
- **`emulator/os/linux/src/kernel/mem.rs` / `syscall.rs`**：匿名/文件/设备 `mmap` 从“推进 `next_mmap`”改为向共享地址空间请求 `Dynamic` 分配；`munmap` / `mprotect` 路径要与 VMA 账本同步。
- **`emulator/elf/loader`**：`LoadContext` 契约与测试桩会更新，以适应统一地址空间 authority。
- **测试**：需要新增重叠即时报错、dynamic gap search、多模块加载不重叠、`munmap` 拆分区间、`mprotect` 权限视图、Linux `mmap` 与 loader 共用同一 VMA 真相等回归用例。
