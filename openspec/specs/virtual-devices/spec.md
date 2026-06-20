## Purpose

定义 `rundroid` 中目标侧可观察虚拟设备、builtin device 和 Python 自定义设备的稳定接口边界，确保新增设备不需要修改 syscall 核心分支，并保持路径挂载、fd 生命周期和设备行为的清晰分层。

## Requirements

### Requirement: Explicit VFS mount surface

runtime SHALL 为普通文件节点与设备节点提供显式挂载面。

#### Scenario: Mount regular file or device explicitly

- **WHEN** runtime 或 case 脚本注册一个虚拟路径
- **THEN** 它 SHALL 能显式选择 `map_file` 或 `map_device`
- **AND** 这些挂载类型 SHALL 通过统一的 VFS 入口管理
- **AND** 当前阶段不 SHALL 强制要求单一 rootfs 或 `vfs root`

#### Scenario: Path conflicts fail fast

- **WHEN** runtime 试图对同一个虚拟路径重复注册 `map_file`、`map_device` 或等价节点来源
- **THEN** runtime SHALL 立即返回显式错误
- **AND** 不 SHALL 静默覆盖原有挂载

#### Scenario: File mounts unify host-path and bytes sources

- **WHEN** runtime 通过 `map_file` 挂载一个普通文件节点
- **THEN** 该文件节点 SHALL 能统一表示宿主文件来源或内存字节来源
- **AND** 这两种来源 SHALL 共享相同的 open/read/pread/fstat 分发主线

### Requirement: File-node operations are target-observable

regular file node SHALL 遵守与 device node 一致的目标侧可观察正确性边界。

#### Scenario: Unified file sources share one regular-file contract

- **WHEN** 某个虚拟路径由 `VirtFile.host(...)`、`VirtFile.bytes(...)` 或动态 regular file provider 挂载
- **THEN** `open/read/pread/fstat` SHALL 走同一 regular-file 合同
- **AND** 成功的 `read/pread` SHALL 让目标缓冲区可观察到对应字节

#### Scenario: Dynamic file providers do not bypass writeback rules

- **WHEN** 动态 regular file provider 在运行时生成读取结果
- **THEN** runtime SHALL 对其应用与宿主文件、内存字节相同的目标侧回写与错误传播规则
- **AND** provider 自身返回成功不 SHALL 自动等价为目标侧可观察成功

### Requirement: Path-driven device mounting

runtime SHALL 通过虚拟路径挂载虚拟设备。

#### Scenario: Mount a device at a virtual path

- **WHEN** runtime 或 case 脚本为某个虚拟路径注册设备
- **THEN** VFS SHALL 把该路径解析为 device node
- **AND** syscall core SHALL 不需要硬编码该路径的特殊分支

### Requirement: Per-open device instances

runtime SHALL 区分设备定义与每次 open 的设备实例。

#### Scenario: Open returns a per-fd device handle

- **WHEN** 目标程序对某个 device path 执行 `open`
- **THEN** runtime SHALL 通过 device factory 创建或获取适当的 device instance
- **AND** fd table SHALL 保存 device handle
- **AND** 后续 `read/write/ioctl/mmap/close` SHALL 按 fd 分发，而不是重新按路径分发

### Requirement: Builtin and custom devices share one abstraction

builtin device 与 custom device SHALL 通过统一抽象工作。

#### Scenario: Custom device can override builtin mapping

- **WHEN** runtime 预装 builtin device，且 case 显式挂载同路径 custom device
- **THEN** runtime SHALL 按明确定义的优先级选择映射
- **AND** builtin 与 custom device SHALL 共享同一分发接口

### Requirement: Device operations are observable

driver 路径 SHALL 输出统一 telemetry 事件。

#### Scenario: Device IO emits structured events

- **WHEN** 虚拟设备处理 `open/read/write/ioctl/mmap/close`
- **THEN** runtime SHALL 输出结构化事件
- **AND** 失败事件 SHALL 包含虚拟路径、fd 和相关请求信息

### Requirement: File and device mmap is realized by runtime

file node 与 device node 的 `mmap` SHALL 通过 runtime/backend 协同落地到目标侧。

#### Scenario: Node-backed mmap creates a target-accessible region

- **WHEN** 某个 file node 或 device node 支持 `mmap`
- **THEN** node 侧 SHALL 描述内容或区域语义，而 runtime/backend SHALL 负责建立真实目标侧映射
- **AND** runtime 不 SHALL 在映射未建立时报告 `mmap` 成功
