## 1. MemoryAddressSpace 核心模型

- [ ] 1.1 在 `emulator/memory` 新增 `MemoryAddressSpace` 聚合根与 VMA 数据模型，支持 `Reserved` / `Dynamic` 两类分配模式、usage 元数据、权限视图与按地址排序的统一账本
- [ ] 1.2 为 `MemoryAddressSpace` 增加对齐、overflow、overlap、gap search、按地址查找等核心逻辑，并补单测覆盖固定地址冲突、dynamic 找洞、边界对齐与地址溢出
- [ ] 1.3 在 `emulator/memory` 设计窄 materialize executor/trait，把 `map` / `protect` / `unmap` 与 VMA 预检查流程串起来，保证“backend 成功后才记账”
- [ ] 1.4 为 `MemoryAddressSpace` 增加 `protect` / `unmap` 账本更新能力，补单测覆盖完整移除、部分拆分、权限更新与已释放 gap 重新分配

## 2. ELF loader 与装配层接入统一 authority

- [ ] 2.1 修改 `emulator/elf/loader/src/api.rs` 与相关实现，把 `reserve_image_space` 契约改为通过共享 guest 地址空间 authority 完成镜像 footprint 的选址与 materialize
- [ ] 2.2 改造 `emulator/case-runner/src/runtime.rs`，移除主镜像装载路径对私有 `reserve_cursor` 的依赖，让 ELF loader、scratch、JNI ABI/trampoline 等路径共享一个 `MemoryAddressSpace`
- [ ] 2.3 改造 `emulator/bindings/python/src/lib.rs`，移除 binding 侧私有 `reserve_cursor`，与 case-runner 复用同样的 guest 地址空间逻辑
- [ ] 2.4 为 ELF 装载路径补回归：至少覆盖多模块加载不重叠、`Reserved` 固定布局冲突即时报错、不同 consumer 共享同一 VMA 真相

## 3. Linux mmap / munmap / mprotect 追平

- [ ] 3.1 修改 `emulator/os/linux/src/kernel/mem.rs`，去掉 `next_mmap` 作为 guest VMA 真相的职责，把匿名/文件/设备 `mmap` 地址选择改为通过共享 `MemoryAddressSpace` 完成
- [ ] 3.2 修改 `emulator/os/linux/src/syscall.rs` 与接线层，让 `sys_mmap` 在统一 authority 中做 overlap 预检查、backend materialize 与内容落地，而不是先推进私有游标
- [ ] 3.3 为 `munmap` / `mprotect` 建立共享地址空间回写路径，确保 syscall、loader 与后续分配使用同一份 VMA 账本
- [ ] 3.4 补 Linux 路径测试：至少覆盖 anonymous/file/device `mmap` 与 ELF 区间共存、不重叠、`munmap` 后 gap 可复用、`mprotect` 后权限视图更新

## 4. 文档与验证

- [ ] 4.1 更新 `ROADMAP.md` 中关于 `RegionTracker`、ELF reserve、Linux `mmap` 地址分配的描述，明确 `MemoryAddressSpace` 是第一权威
- [ ] 4.2 运行 `cargo test --workspace`，并补跑 Python 绑定相关回归（至少包含多模块加载场景）
- [ ] 4.3 运行 `openspec validate --type change memory-address-space-authority --strict`，确认 proposal / design / specs / tasks 全部通过校验
