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

### Requirement: Bootstrap target write correctness

bootstrap runtime SHALL 把目标侧可观察写入作为 syscall 成功的前提。

#### Scenario: getrandom writes into mapped target buffer

- **WHEN** 目标代码调用 `getrandom` 或同类写缓冲区 syscall
- **THEN** runtime SHALL 只在数据真实写入目标缓冲区后返回成功长度
- **AND** 如果目标地址未映射或写入失败，runtime SHALL 返回失败

### Requirement: Bootstrap file and device read correctness

bootstrap runtime SHALL 对文件节点与设备节点执行相同的目标侧回写正确性约束。

#### Scenario: Builtin device or mapped file writes target buffer before success

- **WHEN** 目标程序从 `/dev/urandom`、`VirtFile.bytes(...)` 或 `VirtFile.host(...)` 读取数据到已映射目标缓冲区
- **THEN** runtime SHALL 仅在字节真实写入目标缓冲区后返回成功
- **AND** 如果源字节已准备好但目标侧回写失败，runtime SHALL 返回失败

#### Scenario: Read regression verifies target-visible bytes

- **WHEN** bootstrap regression case 覆盖 `read` 或 `pread64`
- **THEN** case SHALL 在调用后回读目标内存并断言长度或内容
- **AND** 不 SHALL 仅依据 syscall 返回值判定通过

### Requirement: Bootstrap manifest parameters are enforced

bootstrap runtime SHALL 让 case manifest 的关键参数真正生效。

#### Scenario: seed and backend are applied

- **WHEN** case manifest 指定 `seed`、`arch` 或 `backend`
- **THEN** runtime SHALL 应用该配置
- **AND** 如果该配置超出 bootstrap 支持矩阵，runtime SHALL 显式报错

### Requirement: Bootstrap scratch memory stays test-scoped

bootstrap runtime SHALL 把 scratch memory 限定为 harness/stub/debug 辅助能力。

#### Scenario: Scratch API is not treated as target heap

- **WHEN** case runner 或 Python stub 使用 scratch buffer 准备目标侧参数或回读输出
- **THEN** runtime SHALL 允许这类辅助用法
- **AND** 不 SHALL 把 scratch API 当成正常目标堆、`malloc` 或通用 userspace allocator 语义的一部分

### Requirement: Bootstrap page protection tightening

bootstrap runtime SHALL 支持最小页权限收紧路径。

#### Scenario: RELRO and segment permissions are applied

- **WHEN** ELF 模块完成 relocation 写回
- **THEN** runtime SHALL 至少在 Unicorn backend 上应用分段权限或 RELRO 收紧
- **AND** 不 SHALL 仅通过事件记录假装完成该步骤

### Requirement: Bootstrap mmap must create target-visible mappings

bootstrap runtime SHALL 让 `mmap` 成功与目标侧可访问映射严格对应。

#### Scenario: Supported mmap returns a target-accessible region

- **WHEN** 目标程序执行 bootstrap 已支持的匿名映射、文件映射或设备映射
- **THEN** runtime SHALL 在返回成功前建立真实目标侧映射，并在需要时完成初始字节落地
- **AND** 如果映射无法建立，runtime SHALL 返回失败而不是返回占位地址

