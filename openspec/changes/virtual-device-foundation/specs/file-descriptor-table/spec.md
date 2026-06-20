## ADDED Requirements

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
