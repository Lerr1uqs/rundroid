## Why

当前 `rundroid` 的 file IO 还停留在 bootstrap 级别，设备行为主要通过少量硬编码逻辑模拟。

这条路径虽然能让 `/dev/urandom`、`/dev/null` 之类的最小 case 跑起来，但它有明显问题：

- 新增一个设备往往意味着修改 syscall 或 VFS 核心代码
- 设备路径、fd 行为、`ioctl`、`mmap` 语义容易耦合在一起
- Python 脚本层无法优雅地定义和复用设备模拟
- builtin 设备和用户自定义设备缺少统一抽象

用户提出的方向是“像 qiling 那样，通过虚拟路径挂设备类，例如 `/dev/urandom -> class Urandom + decorator`”。这个方向总体合理，而且和之前 `agent-output/unidbg-rs-spec` 中的 `VirtualDevice` / `map_device` / driver registry 设计是一致的。

但要注意一个边界：

- decorator 适合做声明和元数据收集
- 真正的挂载和生效应该由 runtime 或 case 脚本显式完成

否则 import-time 自动注册很容易造成：

- 全局状态污染
- case 之间互相影响
- 回归不可重复

## What Changes

这个 change 用来建立 `rundroid` 的第一版 virtual device / file IO extensibility 基座。

本次变更引入：

- 统一的文件挂载面：`map_file(virtual_path, VirtFile(...))`
- 与文件节点分离的设备挂载面：`map_device(...)`
- 独立的 device registry 和 device factory 模型
- 路径驱动的设备挂载语义
- `VirtFile` 统一抽象 host file、bytes file 和动态 regular file
- builtin 设备与用户自定义设备的统一抽象
- Python `VirtualDevice` / `VirtFile` 基类与 decorator 元数据模型
- Python stub 通过 FFI/binding 向 Rust runtime 注册文件节点与设备节点
- fd 生命周期与 device instance 生命周期分离
- `read` / `write` / `ioctl` / `mmap` / `fstat` 的统一分发路径

本次变更不要求：

- 完整 rootfs
- 完整 `/proc` / `/sys` 仿真
- 完整 `epoll` / `poll`
- 所有 Android 特殊设备都覆盖
- 强制引入单一 `vfs root` 概念

## Capabilities

这个 change 会新增或定义：

- virtual-devices
- runtime-correctness
- testing-harness

## Impact

实现方完成这个 change 后，新增一个设备不应再需要编辑 syscall matcher 核心分支。

review 的重点应放在：

- 设备是否通过 registry / mapper 挂载
- fd 是否保存 device handle 而不是反复按路径分派
- Python decorator 是否只负责声明而不是偷偷修改全局状态
- Python stub 是否通过受控 FFI 注册，而不是在 Python 侧私自接管 runtime 核心状态
