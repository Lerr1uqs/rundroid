## ADDED Requirements

### Requirement: Bootstrap explicit virtual path mounts

bootstrap runtime SHALL 提供显式虚拟路径挂载能力。

#### Scenario: Map regular file or device without rootfs

- **WHEN** case 或 runtime 想把宿主资源暴露给目标侧
- **THEN** 它 SHALL 能使用 `map_file` 或 `map_device`
- **AND** 不 SHALL 依赖完整 rootfs 或 `vfs root` 概念才能工作

#### Scenario: Duplicate path registration fails immediately

- **WHEN** bootstrap runtime 试图对同一个虚拟路径重复注册文件节点或设备节点
- **THEN** runtime SHALL 立即返回显式错误
- **AND** 不 SHALL 静默覆盖已有挂载

#### Scenario: map_file accepts unified file source

- **WHEN** case 或 runtime 通过 `map_file` 挂载一个文件节点
- **THEN** 它 SHALL 能接受等价于 `VirtFile.host(...)` 或 `VirtFile.bytes(...)` 的统一文件来源

### Requirement: Bootstrap map_file read correctness

bootstrap runtime SHALL 让 `map_file` 挂载的普通文件节点满足目标侧可观察读语义。

#### Scenario: VirtFile host and bytes write target buffer before success

- **WHEN** 目标程序从 `VirtFile.host(...)` 或 `VirtFile.bytes(...)` 挂载出的文件节点读取数据到目标缓冲区
- **THEN** runtime SHALL 仅在字节真实写入目标缓冲区后返回成功
- **AND** bootstrap regression case SHALL 在调用后回读目标缓冲区进行断言

#### Scenario: Dynamic regular file providers share the same contract

- **WHEN** case 挂载一个 `VirtFile` 子类或等价动态 regular file provider
- **THEN** `read/pread` SHALL 复用与 builtin 文件来源相同的目标侧回写与错误传播模型
- **AND** provider 扩展不 SHALL 需要编辑 syscall 核心分支

### Requirement: Bootstrap builtin device registry

bootstrap runtime SHALL 建立 builtin device registry。

#### Scenario: Urandom is provided by registry instead of hardcoded branch

- **WHEN** 目标程序打开 `/dev/urandom` 或 `/dev/random`
- **THEN** runtime SHALL 通过 device registry 命中 builtin device factory
- **AND** 不 SHALL 依赖 syscall matcher 或 VFS 核心里的路径硬编码分支

### Requirement: Bootstrap fd-based device dispatch

bootstrap runtime SHALL 以 fd handle 为中心分发设备行为。

#### Scenario: Read and ioctl dispatch by fd kind

- **WHEN** 目标程序已经通过 `open` 获得一个 device fd
- **THEN** `read/write/ioctl/mmap/close` SHALL 根据 fd 保存的 device handle 分发
- **AND** 不 SHALL 再反复按虚拟路径重新选择设备行为

### Requirement: Bootstrap file or device mmap correctness

bootstrap runtime SHALL 让 file node 与 device node 的 `mmap` 成功严格对应目标侧可访问映射。

#### Scenario: Node-backed mmap cooperates with backend mapping

- **WHEN** 某个 file node 或 device node 声明支持 `mmap`
- **THEN** runtime SHALL 协调节点语义与 backend 映射，返回真实目标侧可访问区间
- **AND** 如果映射无法建立，runtime SHALL 返回失败而不是返回一个伪成功地址
