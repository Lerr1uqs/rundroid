## MODIFIED Requirements

### Requirement: Loader trait boundary

`runtime/elf/loader` SHALL 只负责单模块装载和目标内存布局，并通过共享的 guest 地址空间 authority 完成镜像 footprint 的地址分配与 materialize。

#### Scenario: Load one parsed image into target memory

- **WHEN** loader 收到一个 `ParsedElf`
- **THEN** 它 SHALL 通过共享的 `MemoryAddressSpace` 或等价 guest 地址空间 authority 完成地址空间保留、段映射、字节写入、零填充、段权限与 TLS 基础布局
- **AND** 它 SHALL 返回包含导出表、待解析 relocation、init 计划的模块对象
- **AND** 它 SHALL 不在该步骤内完成跨模块符号解析

#### Scenario: Loader does not own a private reserve cursor

- **WHEN** loader 为某个 ELF image 申请 guest 地址空间
- **THEN** 它 SHALL 复用运行时共享的 guest 地址空间 authority
- **AND** 不 SHALL 通过调用方私有的 reserve cursor 或独立地址真相决定镜像基址
