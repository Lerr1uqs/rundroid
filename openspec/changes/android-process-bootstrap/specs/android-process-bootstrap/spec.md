## ADDED Requirements

### Requirement: Zygote struct for process initialization

`Zygote` SHALL 是承载 guest 进程初始化的唯一入口与集中式组件。所有进程初始状态构造（auxv、TLS、stack、constructor 执行）SHALL 通过 Zygote 完成，不分散到 loader、kernel、syscall 或 `call_export` 中。

#### Scenario: Zygote is constructed at assembly time

- **WHEN** case-runner 完成模块加载、relocation 与链接
- **THEN** 它 SHALL 构造或激活 Zygote 来完成 guest 进程初始化
- **AND** Zygote SHALL 在 Kernel（原 LinuxRuntime）可用之后、`JNI_OnLoad` 之前完成初始化

#### Scenario: Zygote provides a single bootstrap entry point

- **WHEN** assembly 层调用 Zygote 的 bootstrap 入口
- **THEN** Zygote SHALL 依次完成：stack 布局 → auxv 写入 → TLS 块分配 → TPIDR_EL0 设置 → constructor 执行
- **AND** 完成后 SHALL 使 guest 处于"JNI_OnLoad 可安全调用"的状态

### Requirement: Auxiliary vector construction

Zygote SHALL 在 guest 栈上构造最小辅助向量（auxv），包含 AT_RANDOM 与 AT_PAGESZ 两项，以 AT_NULL 终止。

#### Scenario: AT_RANDOM is present

- **WHEN** Zygote 写入 auxv
- **THEN** 它 SHALL 在 auxv 中写入 `{AT_RANDOM(25), addr}` 项
- **AND** addr SHALL 指向栈上 16 字节随机数（由 `getrandom_bytes` 等来源提供）
- **AND** 该 16B 随机数 SHALL 在每次 bootstrap 时不同（若 engine 支持 PRNG）

#### Scenario: AT_PAGESZ is present

- **WHEN** Zygote 写入 auxv
- **THEN** 它 SHALL 在 auxv 中写入 `{AT_PAGESZ(6), 0x1000}` 项

#### Scenario: auxv is null-terminated

- **WHEN** Zygote 完成 auxv 条目写入
- **THEN** 它 SHALL 以 `{AT_NULL(0), 0}` 终止 auxv 序列

#### Scenario: Other AT_* are not required

- **WHEN** Zygote 构造 auxv
- **THEN** 它 SHALL 不 要求提供 AT_PHDR、AT_ENTRY、AT_HWCAP、AT_UID、AT_SECURE 等非最小条目
- **AND** guest libc SHALL 能在仅有 AT_RANDOM + AT_PAGESZ 的条件下正常运行（libc 对缺失值有 safe default）

### Requirement: Main thread TLS construction

Zygote SHALL 在 guest 栈上分配主线程 TLS 块并设置 TPIDR_EL0。

#### Scenario: TLS block is allocated on stack

- **WHEN** Zygote 执行 TLS 初始化
- **THEN** 它 SHALL 在 guest 栈上（低地址方向）分配至少 512 字节空间
- **AND** 该空间 SHALL 以 8 字节对齐

#### Scenario: Minimal pthread_internal_t is populated

- **WHEN** Zygote 填充 TLS 块
- **THEN** 它 SHALL 在预期偏移处填入以下值：
  - errno 指针或值 SHALL 设置为 0（初始 errno 为 0）
  - tids SHALL 设置为 1（主线程）
- **AND** 其它 pthread_internal_t 字段 SHALL 初始化为 0

#### Scenario: TPIDR_EL0 points to TLS block

- **WHEN** Zygote 完成 TLS 块填充
- **THEN** 它 SHALL 通过 `Engine::reg_write(Arm64Reg::TpidrEl0, tls_addr)` 或等价机制将 TPIDR_EL0 设置为 TLS 块地址
- **AND** 该设置 SHALL 对 guest 代码可见（guest 读 TPIDR_EL0 得到正确地址）

### Requirement: Stack layout

Zygote SHALL 在 guest 栈顶布置 argc/argv/envp/auxv 指针序列，符合 Linux ARM64 初始栈布局惯例。

#### Scenario: Stack top has standard layout

- **WHEN** Zygote 布置初始栈
- **THEN** 栈顶区域 SHALL 按以下顺序排列：
  1. `argc`（8B，值固定为 0，即无命令行参数）
  2. `argv` 指针序列以 `NULL` 终止（空）
  3. `envp` 指针序列以 `NULL` 终止（空）
  4. auxv 条目序列（type/value pair）以 `AT_NULL` 终止
- **AND** SP SHALL 在布局完成后指向栈顶（`argc` 地址）

#### Scenario: Stack base is configurable

- **WHEN** Zygote 确定 STACK_BASE
- **THEN** 它 SHALL 使用一个确定的基址（当前为 `0x7F_E000_0000`）
- **AND** 栈增长方向为向低地址（初始 SP 在基址 + 大小处）

### Requirement: Constructor execution

Zygote SHALL 在 relocation 完成后、JNI_OnLoad 之前，按 init_order（拓扑序）执行各模块的 constructor（`DT_INIT` + `DT_INIT_ARRAY`）。

#### Scenario: Constructor runs after relocation

- **WHEN** 链接完成且 `LinkReport.init_order` 可用
- **THEN** Zygote SHALL 遍历 `init_order`，对每个 module 执行其 `InitPlan`
- **AND** 执行 SHALL 在 JNI_OnLoad 调用之前完成

#### Scenario: Legacy init (DT_INIT) is called

- **WHEN** 某个 module 的 `InitPlan.legacy_init` 为 `Some(addr)`
- **THEN** Zygote SHALL 以零参数调用该地址（不设 x0-x7 或设零值）
- **AND** 调用 SHALL 使用 `Engine::emu_start` 或 `call_export` 等价机制，以 sentinel/ret 结束

#### Scenario: Init array entries are called in order

- **WHEN** 某个 module 的 `InitPlan.init_array` 非空
- **THEN** Zygote SHALL 按数组顺序依次调用每个函数指针
- **AND** 每个调用 SHALL 先通过 `mem_read` 从 slot 地址读取函数指针值，再以该值作为入口执行

#### Scenario: Execution order follows topological sort

- **WHEN** 多个 module 都有 constructor
- **THEN** 执行序 SHALL 使用 `init_order`（即依赖先于被依赖初始化）
- **AND** 如果 `init_order` 为空或出错，执行序回退为加载序

#### Scenario: Constructor failure propagates

- **WHEN** 某个 constructor 执行中 guest 触发未处理异常（如 `__stack_chk_fail`）
- **THEN** Zygote SHALL 将错误传播给调用者
- **AND** 不 SHALL 静默跳过失败的 constructor

### Requirement: Zygote is self-contained

Zygote SHALL 保持自包含：它完成自己的 invariants（auxv/TLS/stack/constructor），不将内部状态泄漏给 syscall handler 或 linker。

#### Scenario: No leak into syscall layer

- **WHEN** syscall handler 接收 guest syscall
- **THEN** 它 SHALL 不 需要访问 Zygote 的内部状态来完成语义
- **AND** 不 SHALL 感知 auxv/TLS/constructor 细节

#### Scenario: Linker does not depend on Zygote

- **WHEN** linker 执行符号解析和 relocation
- **THEN** 它 SHALL 不 需要 Zygote 存在
- **AND** Zygote 可以 linker 之后出现而不改变 linker 行为
