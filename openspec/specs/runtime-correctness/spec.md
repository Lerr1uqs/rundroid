## Purpose

定义 `rundroid` 在运行时正确性上的长期要求，确保目标侧可观察状态、manifest 生效行为和页权限语义不会在后续扩展中被重新做松。

## Requirements

### Requirement: Target-observable syscall effects

runtime SHALL 以目标侧可观察状态定义 syscall 成功。

#### Scenario: Target buffer write must be real

- **WHEN** 某个 syscall 声称已向目标缓冲区写入数据
- **THEN** 该写入 SHALL 真实作用到目标可读内存
- **AND** 如果写入失败，runtime SHALL 返回失败而不是仅返回长度

### Requirement: Target-observable file IO effects

file IO 与 device IO SHALL 以目标缓冲区的真实变化作为成功判据。

#### Scenario: Read-like operations require target writeback

- **WHEN** runtime 处理 `read`、`pread64`、`getrandom` 或等价的文件/设备读操作，并声称已填充目标缓冲区
- **THEN** runtime SHALL 先把结果字节真实写入目标缓冲区，再返回成功长度
- **AND** 如果数据源可用但目标侧回写失败，runtime SHALL 返回失败

#### Scenario: File source kinds share one correctness rule

- **WHEN** 某个虚拟路径由宿主文件、内存字节或动态文件 provider 提供内容
- **THEN** 这些来源 SHALL 共享一致的目标侧回写与错误传播语义
- **AND** runtime 不 SHALL 把 `VirtFile.host(...)` 或 `VirtFile.bytes(...)` 视为“只要拿到源字节就算成功”

### Requirement: Manifest fields must be effective

case manifest 的关键字段 SHALL 真正影响 runtime 行为。

#### Scenario: Manifest parameters are applied

- **WHEN** case manifest 指定 `arch`、`backend`、`seed` 或 `telemetry`
- **THEN** runtime SHALL 应用这些参数
- **AND** 对于当前不支持的参数组合，runtime SHALL 显式报错

### Requirement: Memory permissions are meaningful

目标内存权限 SHALL 反映真实的装载与链接阶段。

#### Scenario: Loader and linker tighten permissions

- **WHEN** 模块完成装载与 relocation 写回
- **THEN** runtime SHALL 根据段权限与 RELRO 规则收紧目标页权限
- **AND** 不 SHALL 永久保留全局 RWX 映射作为稳定语义

### Requirement: Target-observable mmap effects

runtime SHALL 仅在映射真实建立后报告 `mmap` 成功。

#### Scenario: Successful mmap returns target-accessible memory

- **WHEN** runtime 对匿名映射、文件映射或设备映射返回成功
- **THEN** 返回区间 SHALL 按当前支持的权限/标志子集在目标侧真实可访问
- **AND** 如果 runtime 无法建立映射或完成初始数据落地，runtime SHALL 返回失败而不是仅返回一个地址
