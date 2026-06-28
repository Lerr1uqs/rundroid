## Why

当前 `rundroid` 的 file IO 还停留在 bootstrap 级别，设备行为主要通过少量硬编码逻辑模拟。

这条路径虽然能让 `/dev/urandom`、`/dev/null` 之类的最小 case 跑起来，但它有明显问题：

- 新增一个设备往往意味着修改 syscall 或 VFS 核心代码
- 设备路径、fd 行为、`ioctl`、`mmap` 语义容易耦合在一起
- builtin 设备和用户自定义设备缺少统一抽象

## What Changes

这个 change 用来建立 `rundroid` 的第一版 Rust 侧 virtual device / file IO extensibility 基座。

本次变更引入：

- 统一的文件挂载面：`map_file(virtual_path, VirtFile(...))`
- 与文件节点分离的设备挂载面：`map_device(...)`
- 显式 `FileDescriptorTable` 与 `FileDescriptorEntry` 抽象
- 独立的 device registry 和 device factory 模型
- 路径驱动的设备挂载语义
- `VirtFile` 统一抽象 host file、bytes file 和动态 regular file
- builtin 设备与用户自定义设备的统一抽象
- fd 生命周期与 device instance 生命周期分离
- `read` / `write` / `ioctl` / `mmap` / `fstat` 的统一分发路径
- 同名虚拟路径挂载冲突的 fail-fast 规则

本次变更不要求：

- 完整 rootfs
- Python binding
- Python decorator
- PyO3 / FFI 回调
- 完整 `/proc` / `/sys` 仿真
- 完整 `epoll` / `poll`
- 所有 Android 特殊设备都覆盖
- 强制引入单一 `vfs root` 概念

## Capabilities

这个 change 会新增或定义：

- file-descriptor-table
- virtual-devices
- runtime-correctness
- testing-harness

## Impact

实现方完成这个 change 后，新增一个设备不应再需要编辑 syscall matcher 核心分支。

review 的重点应放在：

- `FileDescriptorTable` / `FileDescriptorEntry` 是否成为统一 fd 权威表
- 设备是否通过 registry / mapper 挂载
- fd 是否保存 device handle 而不是反复按路径分派
- 同名虚拟路径冲突是否立即报错，而不是静默覆盖
