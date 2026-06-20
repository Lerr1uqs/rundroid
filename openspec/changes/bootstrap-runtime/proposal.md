## Why

`rundroid` 现在还是一个空仓库，首先需要建立一条最小但方向正确的实现主线。

如果一开始就追求“完整替代旧版 unidbg”，范围会迅速失控，特别是会同时被 JNI、hook、driver、backend 矩阵拖住。

当前最合理的启动方式是优先建立：

- Rust workspace 骨架
- `runtime/core`
- `runtime/backends/api`
- `runtime/backends/unicorn`
- `runtime/memory`
- `runtime/elf/parse`
- `runtime/elf/loader`
- `runtime/elf/linker`
- `runtime/os/linux`
- `runtime/telemetry`
- `runtime/cli`
- 基础 case runner

这条主线跑通之后，再逐层扩展 JNI、hook、driver、多 backend。

## What Changes

这个 change 用来定义 `rundroid` 的启动期运行时。

本次变更引入：

- Rust-first 的 workspace 布局
- 以 Unicorn 为首个实现的 backend 抽象
- 通过配置和 flag 控制的统一 telemetry 子系统
- ARM64 最小 ELF loader 和 Linux runtime 执行路径
- 面向资源系统的测试 harness 和 case 格式
- ELF parser 与 loader/linker 明确分层

本次变更不要求：

- 完整 JNI 兼容
- ARM32/Thumb 支持
- 完整 driver 模拟
- 完整 GDB/LLDB 支持
- 完整 xhook/inline hook 支持

## Capabilities

这个 change 会新增或定义：

- runtime-core
- elf-runtime-interfaces
- telemetry
- testing-harness

## Impact

实现方应当把这个 change 作为第一阶段的交付目标。

在 bootstrap 主线稳定之前，review 阶段应拒绝把范围过早扩展到 JNI、大规模 backend 支持或大型 rootfs 资源。

补充约束：

- 目录层级不使用 `ub-*` 这类临时前缀
- Cargo package 可以使用 `rundroid-*` 命名，但磁盘目录应保持短名、语义化
- ELF parser 优先复用现成 Rust 库，不在 bootstrap 阶段手写完整 parser
- ELF loader / linker 以 Android guest 语义为准自行实现，不直接把 host-oriented 通用 loader 当成运行时核心
