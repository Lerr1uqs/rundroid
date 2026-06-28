## Purpose

定义 `rundroid` 中 `FileDescriptorTable` 与 `FileDescriptorEntry` 的稳定语义边界，确保 fd 分配、生命周期与行为分发不依赖路径硬编码，并能统一覆盖 regular file、device、socket、pipe、eventfd 等对象。
## Requirements
### Requirement: Unified descriptor table

runtime SHALL 使用显式 `FileDescriptorTable` 作为 fd 的唯一权威表。

#### Scenario: All fd-producing objects enter one table

- **WHEN** `open`、`socket`、`pipe`、`eventfd` 或等价路径产生一个新 fd
- **THEN** runtime SHALL 在 `FileDescriptorTable` 中创建一个对应的 `FileDescriptorEntry`
- **AND** 后续 syscall SHALL 先通过该表解析 fd
- **AND** syscall 核心不 SHALL 直接依赖虚拟路径字符串决定行为

### Requirement: FileDescriptorEntry separates descriptor slot from opened handle

`FileDescriptorEntry` SHALL 表达描述符槽位，而不是底层 file/device/socket 实现本体。

#### Scenario: FileDescriptorEntry references an opened handle

- **WHEN** runtime 为一个新 fd 建立 `FileDescriptorEntry`
- **THEN** 该条目 SHALL 保存句柄引用与描述符级元数据
- **AND** per-open 状态 SHALL 由被引用的 opened handle 持有
- **AND** `FileDescriptorEntry` 自身不 SHALL 充当 file/device/socket 行为实现对象

### Requirement: FD-based dispatch is uniform

runtime SHALL 对路径来源对象与非路径来源对象使用统一的 fd 分发主线。

#### Scenario: Syscall dispatches by resolved entry

- **WHEN** 目标程序对一个有效 fd 执行 `read`、`write`、`ioctl`、`mmap`、`fstat` 或 `close`
- **THEN** OS SHALL 先解析对应的 `FileDescriptorEntry`
- **AND** 后续行为 SHALL 基于条目中的 handle 引用或种类信息分发
- **AND** runtime 不 SHALL 为每次操作重新按路径猜测对象类型

### Requirement: Dup creates a new descriptor entry

runtime SHALL 把 `dup`、`dup2`、`dup3` 建模为新的描述符槽位，而不是新的路径解析。

#### Scenario: Duplicate fd produces a distinct entry

- **WHEN** 目标程序复制一个已有 fd
- **THEN** runtime SHALL 为目标 fd 创建或替换一个新的 `FileDescriptorEntry`
- **AND** 新旧条目 SHALL 是两个独立的 descriptor slot
- **AND** 它们对底层 opened handle 的共享或克隆策略 SHALL 被显式定义并稳定保持

### Requirement: Close removes the descriptor entry explicitly

runtime SHALL 显式维护 `FileDescriptorEntry` 的移除与 fd 复用时机。

#### Scenario: Closing a descriptor removes its entry

- **WHEN** 一个 fd 被成功关闭
- **THEN** runtime SHALL 从 `FileDescriptorTable` 中移除其 `FileDescriptorEntry`
- **AND** 后续对该 fd 的访问 SHALL 视为无效，直到 runtime 为它重新分配新条目

### Requirement: Bootstrap explicit FileDescriptorTable

bootstrap runtime SHALL 引入显式 `FileDescriptorTable`。

#### Scenario: Path and non-path objects share one descriptor table

- **WHEN** `open`、`socket`、`pipe`、`eventfd` 或等价路径在 bootstrap runtime 中产出 fd
- **THEN** runtime SHALL 在 `FileDescriptorTable` 中创建 `FileDescriptorEntry`
- **AND** non-path 对象不 SHALL 需要伪造虚拟路径才能进入该表

### Requirement: Bootstrap FileDescriptorEntry stores handle reference

bootstrap runtime SHALL 让 `FileDescriptorEntry` 承担 descriptor slot 语义，而不是行为实现语义。

#### Scenario: Per-open state lives behind the entry

- **WHEN** runtime 为 regular file、device、socket 或 pipe 建立一个 `FileDescriptorEntry`
- **THEN** 该条目 SHALL 保存 handle 引用与 descriptor 级元数据
- **AND** per-open 状态 SHALL 由被引用 handle 持有
- **AND** syscall 分发 SHALL 通过解析后的 `FileDescriptorEntry` 进入后端对象

### Requirement: Bootstrap dup semantics are explicit

bootstrap runtime SHALL 明确 `dup`、`dup2`、`dup3` 的 `FileDescriptorEntry` 语义。

#### Scenario: Dup creates a new entry without rerunning path resolution

- **WHEN** 目标程序复制一个已有 fd
- **THEN** runtime SHALL 创建一个新的 `FileDescriptorEntry`
- **AND** 它不 SHALL 重新按路径匹配 file/device 类型
- **AND** 底层 handle 的共享策略 SHALL 由 runtime 明确定义并受回归保护

### Requirement: Bootstrap close removes descriptor slots

bootstrap runtime SHALL 显式移除被关闭的 fd 条目。

#### Scenario: Close invalidates the old entry

- **WHEN** 一个 fd 被成功关闭
- **THEN** 对应 `FileDescriptorEntry` SHALL 从 `FileDescriptorTable` 中移除
- **AND** 后续对旧 fd 的访问 SHALL 返回无效描述符错误，而不是命中陈旧条目

