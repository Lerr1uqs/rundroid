## ADDED Requirements

### Requirement: Bootstrap parser trait contract

bootstrap runtime SHALL 为 ELF parser 提供清晰的 trait 合同。

#### Scenario: Parser returns normalized immutable image

- **WHEN** bootstrap parser 解析一个 ARM64 Android `.so`
- **THEN** 它 SHALL 返回只读的 `ParsedElf` 风格结果
- **AND** 结果 SHALL 包含段、动态表、导出符号、统一的 relocation 记录和 init 元数据
- **AND** parser SHALL 不接触 backend 或 guest memory

### Requirement: Bootstrap loader trait contract

bootstrap runtime SHALL 为 ELF loader 提供单模块装载合同。

#### Scenario: Loader maps one module without linking dependencies

- **WHEN** bootstrap loader 装载一个已解析镜像
- **THEN** 它 SHALL 完成 guest 内存保留、段映射、写入、零填充和 TLS 基础布局
- **AND** 它 SHALL 产出待 linker 消费的未决 relocation 集合
- **AND** 它 SHALL 不在装载阶段递归完成依赖符号解析

### Requirement: Bootstrap linker trait contract

bootstrap runtime SHALL 为 ELF linker 提供依赖解析和 relocation 合同。

#### Scenario: Link root module through normalized relocation stream

- **WHEN** bootstrap linker 处理一个 root module 及其依赖
- **THEN** 它 SHALL 基于 loader 产出的未决 relocation 进行解析
- **AND** 它 SHALL 不重新读取 packed relocation 原始编码
- **AND** 它 SHALL 生成确定性的 init 调用顺序

### Requirement: Bootstrap AArch64 relocation minimum

bootstrap runtime SHALL 明确 AArch64 最小重定位范围。

#### Scenario: Support minimum bootstrap relocation set

- **WHEN** bootstrap runtime 声称 ELF 导出调用路径可用
- **THEN** 它 SHALL 至少支持 `R_AARCH64_RELATIVE`、`R_AARCH64_GLOB_DAT`、`R_AARCH64_JUMP_SLOT`、`R_AARCH64_ABS64`

### Requirement: Bootstrap interface file layout

bootstrap runtime SHALL 为 parser、loader、linker trait 预留稳定文件布局。

#### Scenario: Interface source layout exists

- **WHEN** 实现方创建 ELF 相关 crate
- **THEN** 每个 crate SHALL 至少区分 `api.rs`、`model.rs`、`error.rs`
- **AND** loader crate SHOULD 额外拆分 `tls.rs` 与 `relro.rs`
- **AND** linker crate SHOULD 额外拆分 `resolver.rs`、`reloc_aarch64.rs` 与 `init.rs`
