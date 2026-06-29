# Memory Space

## 定义

`MemoryAddressSpace` 是 guest VMA 的第一权威。

它夹在 `Backend` 的 `mem_map` / `mem_protect` / `mem_unmap` 能力之上，  
夹在 ELF loader、Linux `mmap`、runtime 固定布局分配之下。

## 负责什么

- 统一管理 guest 区间账本
- 统一做 `Reserved` / `Dynamic` 地址分配
- 在落到 `Backend` 前做 overlap / gap / 对齐校验
- 在 `protect` / `unmap` 后回写同一份 VMA 事实

## 哪些上层必须经过它

- ELF loader 的 image footprint 预留与权限收紧
- Linux `mmap` / `munmap` / `mprotect`
- case-runner 的 stack / scratch / trampoline / JNI ABI 固定布局
- Python binding 侧的装载与运行时地址分配

## 不允许绕过

- 不允许私有 `reserve_cursor`
- 不允许私有 `next_mmap`
- 不允许上层直接把 guest 地址真相写死到 `Backend`
