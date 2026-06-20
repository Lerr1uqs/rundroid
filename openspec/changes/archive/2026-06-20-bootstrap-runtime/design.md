## Context

bootstrap 阶段必须优先保证边界正确，而不是功能面铺得很大。

新的 runtime 目标是把旧的 Java-heavy 控制流替换成：

- Rust 执行层 / runtime 层
- Python 脚本层以及后续的 `javashim`
- 通过配置控制的 telemetry 和调试能力

第一版实现必须保持收敛：

- ARM64
- Unicorn backend
- ELF 导出函数调用
- 基础 Linux 用户态行为

## Architecture

bootstrap workspace 包含：

- `runtime/core`
- `runtime/backends/api`
- `runtime/backends/unicorn`
- `runtime/memory`
- `runtime/elf/parser`
- `runtime/elf/loader`
- `runtime/elf/linker`
- `runtime/os/linux`
- `runtime/telemetry`
- `runtime/cli`

建议的物理目录树：

```text
rundroid/
  runtime/
    core/
    backends/
      api/
      unicorn/
    memory/
    elf/
      parse/
      loader/
      linker/
    os/
      linux/
    telemetry/
    cli/
  bindings/
    python/
  cases/
  resources/
  tools/
  openspec/
```

建议的 Cargo package 命名：

- `rundroid-core`
- `rundroid-backend`
- `rundroid-backend-unicorn`
- `rundroid-memory`
- `rundroid-elf-parser`
- `rundroid-elf-loader`
- `rundroid-elf-linker`
- `rundroid-linux`
- `rundroid-telemetry`
- `rundroid-cli`

telemetry 子系统是统一的。日志、结构化事件、trace、debugger transcript 都属于同一个通过配置控制的表面。

初始 runtime 路径是：

1. 创建 `RuntimeConfig`
2. 选择 backend
3. 初始化 telemetry router
4. 建立内存、栈、TLS
5. 加载一个简单 ARM64 ELF `.so`
6. 调用一个导出符号
7. 通过 test harness 输出 artifacts

## Key Decisions

### 1. Telemetry 是一个子系统

观测和调试不再拆成两个顶层架构支柱。

它们统一合并到 `runtime/telemetry` crate 中，由配置文件或 CLI flag 控制。

### 2. Rust-first 执行路径

bootstrap 主线不能依赖 Java 组件。

Java 以后可以作为 differential testing 的可选 oracle，但不能成为 required runtime path。

### 3. ELF parser / loader / linker 分层

`unidbg` 现状已经证明 parser 和 loader/linker 应分层：

- `jelf` 负责 ELF 结构读取
- `AndroidElfLoader` 负责 map、依赖加载、重定位、init 调用
- `LinuxModule` / `ModuleSymbol` 负责模块与符号解析

`rundroid` 应保持这个边界，但用 Rust 重写：

- `runtime/elf/parser` 负责 ELF 头、program headers、dynamic table、dynsym、dynstr、hash、version、rel/relA 原始读取
- `runtime/elf/loader` 负责 PT_LOAD 映射、段权限、load bias、TLS 基础布局、模块对象构建
- `runtime/elf/linker` 负责 `DT_NEEDED` 依赖图、符号查找、重定位写回、`init_array`/`fini_array`

bootstrap 的 parser 选型要求：

- 默认使用现成 Rust parser crate，而不是手写完整 ELF parser
- 首选 `elf` crate 作为底层 parser 适配层
- `goblin` 可作为调试或回归对照，不作为主实现约束
- `LIEF` 不作为 runtime 必选依赖，只作为离线分析、样本修补、fixture 生成的可选工具

bootstrap 的 loader/linker 选型要求：

- 不直接复用 host-oriented 通用 loader 作为运行时核心
- 可以参考 `elf_loader` 一类库的接口与 relocation 组织方式
- 但 Android guest 在 Unicorn 中运行时，内存映射、调用入口、TLS、符号桥接、syscall 与 driver 行为仍需自实现

trait 级别建议如下。

`runtime/elf/parser` 负责“把字节变成稳定的、不可变的解析结果”，不能依赖 backend、memory mapper 或 syscall 层。

```rust
pub trait ElfParser: Send + Sync {
    fn parse(&self, input: ParseInput<'_>) -> Result<ParsedElf, ElfParseError>;
}

pub struct ParseInput<'a> {
    pub module_name: &'a str,
    pub bytes: &'a [u8],
    pub policy: ParsePolicy,
}

pub struct ParsedElf {
    pub file: ElfIdentity,
    pub segments: Vec<LoadSegment>,
    pub dynamic: DynamicInfo,
    pub symbols: Vec<DynSymbol>,
    pub relocations: Vec<RelocationRecord>,
    pub init: InitMetadata,
    pub notes: Vec<ParseNote>,
}
```

parser 的关键约束：

- `ParsedElf` 必须是只读快照，后续 loader/linker 不得回写 parser 内部状态
- parser 必须把 `REL`、`RELA`、Android packed relocation 统一归一化为 `RelocationRecord`
- parser 必须显式暴露 `DT_NEEDED`、`DT_SONAME`、`DT_INIT`、`DT_INIT_ARRAY`、`DT_FINI_ARRAY`、TLS 元数据
- parser 的错误只描述格式问题、截断、架构不支持、动态表不合法，不掺杂 map/link 失败

`runtime/elf/loader` 负责“把解析结果映射到 guest address space 并生成模块对象”，但不负责跨模块符号解析。

```rust
pub trait ElfLoader: Send + Sync {
    fn load(
        &self,
        ctx: &mut dyn LoadContext,
        image: &ParsedElf,
        request: LoadRequest<'_>,
    ) -> Result<LoadedModule, ElfLoadError>;
}

pub trait LoadContext {
    fn reserve_image_space(&mut self, size: u64, align: u64) -> Result<u64, MemoryError>;
    fn map_segment(&mut self, spec: SegmentMapSpec<'_>) -> Result<MappedSegment, MemoryError>;
    fn write_bytes(&mut self, guest_addr: u64, bytes: &[u8]) -> Result<(), MemoryError>;
    fn zero_fill(&mut self, guest_addr: u64, len: u64) -> Result<(), MemoryError>;
    fn emit(&mut self, event: TelemetryEvent<'_>);
}

pub struct LoadedModule {
    pub module_id: ModuleId,
    pub name: String,
    pub load_bias: u64,
    pub base: u64,
    pub size: u64,
    pub entry: Option<u64>,
    pub tls: Option<TlsTemplate>,
    pub exports: ExportTable,
    pub unresolved: Vec<PendingRelocation>,
    pub init_plan: InitPlan,
}
```

loader 的关键约束：

- loader 只做单模块装载，不在 `load()` 内部递归 `DT_NEEDED`
- loader 只负责 guest memory 布局、段权限、RELRO/TLS 基础布局、导出表建立
- loader 必须把待解析 relocation 产物以 `PendingRelocation` 形式交给 linker
- loader 的错误只描述空间分配、段重叠、权限切换、镜像布局非法等装载问题

`runtime/elf/linker` 负责“连接模块图、解析符号、写回 relocation、生成 init 调用顺序”。

```rust
pub trait ElfLinker: Send + Sync {
    fn link_root(
        &self,
        ctx: &mut dyn LinkContext,
        graph: &mut ModuleGraph,
        root: ModuleId,
    ) -> Result<LinkReport, ElfLinkError>;
}

pub trait LinkContext {
    fn resolve(&self, query: SymbolQuery<'_>) -> Result<Option<ResolvedSymbol>, ResolveError>;
    fn write_relocation(&mut self, patch: RelocationPatch) -> Result<(), MemoryError>;
    fn protect_relro(&mut self, module: ModuleId) -> Result<(), MemoryError>;
    fn emit(&mut self, event: TelemetryEvent<'_>);
}

pub struct LinkReport {
    pub linked: Vec<ModuleId>,
    pub unresolved: Vec<UnresolvedSymbol>,
    pub init_order: Vec<ModuleId>,
}
```

linker 的关键约束：

- linker 消费 `LoadedModule.unresolved`，不重新解析 ELF 原始字节
- linker 必须先建立依赖图，再做符号解析和 relocation 写回
- linker 必须支持 bootstrap AArch64 所需的最小 relocation 集
- linker 必须把 weak、undefined、host bridge、driver bridge 作为不同 resolution source 记录到 telemetry
- linker 必须输出稳定的 `init_order`，供后续 `JNI_OnLoad` 和普通 init 调用复用

建议的文件布局：

```text
runtime/elf/parser/
  src/
    lib.rs
    api.rs
    model.rs
    error.rs
    parser_elf.rs

runtime/elf/loader/
  src/
    lib.rs
    api.rs
    model.rs
    error.rs
    loader.rs
    tls.rs
    relro.rs

runtime/elf/linker/
  src/
    lib.rs
    api.rs
    model.rs
    error.rs
    linker.rs
    resolver.rs
    reloc_aarch64.rs
    init.rs
```

最小 bootstrap relocation 范围建议锁定为：

- `R_AARCH64_RELATIVE`
- `R_AARCH64_GLOB_DAT`
- `R_AARCH64_JUMP_SLOT`
- `R_AARCH64_ABS64`

Android 变体要求：

- parser 层必须识别 Android packed relocation 并归一化
- linker 层不直接感知 packed encoding，只处理归一化后的 `RelocationRecord`

### 4. 初期范围必须收敛

bootstrap 阶段明确不做：

- 完整 JNI
- ARM32/Thumb
- 多 backend 广度铺开
- 完整 driver 模拟
- 完整 hook 子系统

### 5. Test harness 从第一天就存在

测试不是后补项。

bootstrap 必须包含：

- `case.toml`
- `script.py`
- Rust runner
- resource URI 解析
- artifact 输出

## Risks

### 范围失控

如果过早把 JNI、hook、multi-backend 拉进主线，bootstrap 交付速度会明显下降。

### Telemetry 被绕过

如果子系统直接打印日志，而不是统一走 telemetry，后面的 debug 和 replay 模型会碎掉。

### 硬编码路径

如果初始 case 直接写绝对路径而不是 `resource:` URI，回归体系从第一天就会失去可移植性。

## Mitigations

- 严格按 milestone 顺序推进
- review 中强制检查 telemetry mode
- case manifest 强制使用 `resource:<pack>/...`
- 第一个 smoke case 保持最小和确定性
