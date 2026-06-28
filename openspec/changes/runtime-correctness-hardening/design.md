## Context

这一阶段的目标不是继续铺能力面，而是修正当前 runtime 的正确性边界。

`unidbg` 在 Android 路径上虽然实现复杂、Java 依赖重，但它在几个关键点上的边界是成熟的：

- ELF 装载与链接以目标侧语义为中心，而不是以 host 便利性为中心
- `DT_NEEDED` 会驱动依赖装载，`soName` / `neededLibraries` 参与符号解析
- relocation 写回直接作用到目标内存，失败不会被默默吞掉
- 文件 IO / `mmap` / `/dev/urandom` 这些行为最终都要落实到目标侧可观察状态

这几个点正是 `rundroid` 当前需要补齐的主线。

## 参考实现：unidbg 当前做法

### 1. 依赖装载与链接

`unidbg` 的 `AndroidElfLoader.loadInternal()` 在装载一个模块时会：

1. 解析 ELF 与 program headers
2. 计算镜像 footprint、映射 PT_LOAD 段
3. 读取 `PT_DYNAMIC`
4. 通过 `dynamicStructure.getSOName()` 获取模块 soname
5. 通过 `dynamicStructure.getNeededLibraries()` 枚举 `DT_NEEDED`
6. 对每个依赖：
   - 先查已经装载的模块
   - 未命中则继续递归装载
7. 构建 `neededLibraries`
8. 先尝试解析历史未决符号，再处理当前模块 relocation
9. 构建 `init` / `init_array`
10. 最终把模块放入全局模块表

结论：

- `unidbg` 不是“把所有模块扫一遍看看谁有符号”，而是先建立依赖上下文，再做符号解析。
- `rundroid` 应复用这个原则，但用 Rust 的 `ModuleGraph` 与 trait 做得更清晰。

### 2. 段映射与页权限

`unidbg` 并不是简单粗暴把整块镜像永久映射成 RWX。它会按段计算页对齐区间、对共享页做权限合并或 `mem_protect`、写入真实段数据、维护 `MemRegion`。

结论：“先整块 reserve，再按段收紧权限”是合理路线。

### 3. 文件 IO / `mmap` / `/dev/urandom`

`unidbg` 的文件 IO 路径最终会把读出的字节写入目标缓冲区（`RandomFileIO.read` / `ByteArrayFileIO.read` / `AbstractFileIO.mmap2`）。共同点：**目标侧可观察状态是真实结果的一部分；返回值正确但目标缓冲区没变，不算成功**。

实现主线必须固定成：source 产出字节 → runtime 把结果写回目标缓冲区或建立目标侧映射 → 只有步骤 2 成功 syscall 才返回成功。

## 现状对齐（2026-06-28 评估）

> 本 change 创建于 `bootstrap-runtime` 完成时。后续 8 个 change（android-vm-state-model、jni-shim-foundation、native-jni-lifecycle、python-*、android-framework-stubs、jni-abi-surfaces 等）已把下列建议的绝大部分落地。本节逐条对齐当前代码形态，标注剩余真实 gap。证据为 `file:line`。

### 1. 目标侧状态访问 fail-fast ✅

- `Engine` trait（会话句柄）的 `mem_read`/`mem_write`/`reg_read`/`reg_write`/`mem_protect` 已全部返回 `Result<(), BackendError>`（`emulator/backends/api/src/engine.rs:33-69`）。
- `GuestCPU` trait（hook 内受限视图）的 `mem_read`/`mem_write` **故意**返回 `bool`，用于把"未映射缓冲"上报成 EFAULT（`engine.rs:140-158`）。
- syscall 路径已 fail-fast：`write_guest`/`map_guest` 返回 false 即返回 EFAULT（`emulator/os/linux/src/syscall.rs:397-402, 578-609, 650-652`）。
- **原"统一收敛为 `Backend` 命名"的建议不适用**：`Backend` 现已是工厂 trait（`Backend::open -> Box<dyn Engine>`，`engine.rs:16-22`），`Engine` 是会话 trait。改名将破坏 8 个 change 的代码并与现有 `Backend` 冲突。spec 亦无此命名 SHALL。**跳过。**

### 2. pc_read / pc_write ✅（功能等价）

- PC 读写通过 `reg_read(Arm64Reg::Pc)` / `reg_write(Arm64Reg::Pc, ..)` 实现（`backends/api/src/reg.rs:17`，用法见 `case-runner/src/runtime.rs:519`）。
- 独立方法名非 spec 要求。**跳过。**

### 3. mem_protect + 页权限收紧 ✅

- `Engine::mem_protect` 已存在（`engine.rs:69`）；unicorn 已实现，调 `uc.mem_protect` + page 对齐（`backends/unicorn/src/engine.rs:174-192`）。
- loader 装载后按 PT_LOAD p_flags 收紧段权限（`case-runner/src/runtime.rs:439-450`）。
- linker RELRO 收紧**真调** `engine.mem_protect`（`runtime.rs:711-731`），不是只 emit event。
- **精度 gap（可选，非 spec 强制）**：parser `DynamicInfo.relro` 仅 `bool`（`elf/parser/src/model.rs:83`），loader 用"第一个 RW PT_LOAD 段"近似 RELRO 范围（`elf/loader/src/loader.rs:96-111`）而非精确 PT_GNU_RELRO vaddr/memsz。spec 验收只要求"应用 RELRO 收紧"，已满足。

### 4. parser soname/needed 字符串化 ✅

- `DynamicInfo.soname: Option<String>` + `needed: Vec<String>` 已有，从 DT_SONAME/DT_NEEDED + strtab 解析（`elf/parser/src/parser_elf.rs:51-67`）。init/fini/init_array/fini_array 全有。

### 5. parser 错误分类 ✅

- `ElfParseError` 有 `BadMagic`/`Truncated`/`MalformedDynamic`/`Unsupported`/`Policy`（`elf/parser/src/error.rs:10-31`）。三层错误（parser/loader/linker）严格分离。

### 6. DT_NEEDED 装载队列 + 依赖图 resolve ✅

- `load_and_link` 递归装载 DT_NEEDED + `ModuleGraph.add_dep` + soname 去重（`case-runner/src/runtime.rs:332-391`）。
- linker `resolve` 按 self → direct deps → 依赖闭包 BFS，`SymbolQuery.requester` 限定范围，**非全表扫描**（`elf/linker/src/resolver.rs:37-80`）。
- init_order 用 Kahn 拓扑排序 + 稳定输出 + 环检测（`elf/linker/src/init.rs`）。

### 7. scratch memory ✅

- assemble 时固定映射 1 MiB @ `0x800_000` 为 RW scratch（`runtime.rs:216-229`），`RegionTracker` 标 `RuntimeScratch`，注释明确"仅 case manifest 公开契约，非 malloc/heap"。
- spec "Bootstrap scratch memory stays test-scoped" 已满足。原 `alloc_scratch` 动态 API 建议非 spec 要求。**跳过。**

### 8. file/device/mmap 目标侧回写 ✅（pread64 未实现）

- getrandom/read 写目标失败即 EFAULT（`syscall.rs:397-402, 650-652`）。
- `VirtFile.host`/`VirtFile.bytes` + builtin `/dev/urandom` 统一经 `read_from_fd` → `write_guest`（`emulator/driver/src/fd.rs`，`emulator/driver/src/mapper.rs`，`emulator/driver/src/builtin/urandom.rs`）。
- SYS_MMAP 必须调 `map_guest` 成功才返回地址（`syscall.rs:578-609`）。
- **pread64 未实现**（task 11 字面提及）。spec scenario 用"read **或** pread64"的"或"，read 已覆盖，pread64 非强制。

### 9. case manifest 参数生效 ✅

- arch/backend 不支持即 fail-fast，seed 应用到 LinuxRuntime，telemetry 切换 mode（`case-runner/src/case.rs:42-68`）。`manifest_validation.rs` 有专项测试。

### 10. smoke/regression case ✅（mmap case 缺）

- `03-dev-urandom` 用 XOR 校验和间接断言目标缓冲区非零；`syscall.rs` 有完整 regression suite（`regression_urandom_buffer_visible` / `regression_virtfile_bytes_read_back` / `regression_virtfile_host_read_back` / `regression_dynamic_provider_writeback_failure` 等）。
- **mmap 缺真实回归 case**（见剩余 gap）。

## 剩余真实 gap

经现状对齐，spec 验收角度仅剩 1 个硬缺口：

- **task 16：mmap 真实可读写回归 case**。spec "Bootstrap mmap must create target-visible mappings" 要求 case 证明返回地址真实可读/可写。现状 mmap unit test 用 mock `map_guest`（恒 true，`syscall.rs:886-899, 1206-1286`），未通过真实 backend 验证地址可读写。需要新增 `case.toml` + guest fixture：guest 调 `mmap` → 写该地址 → 读回断言。

可选增强（design 建议、spec 非强制，按需取舍）：

- pread64 实现（task 11 字面）
- RELRO 精确 PT_GNU_RELRO vaddr/memsz（task 6 精度）
- `alloc_scratch` 动态 API（task 9）
- `pc_read`/`pc_write` 独立方法名（task 2）

## Architecture Changes（对齐现状命名）

- `runtime/backends/api`：`Backend`=工厂（`open`），`Engine`=会话句柄（mem_map/mem_protect/mem_read/mem_write/reg_*/emu_*/install_*_hook），`GuestCPU`=hook 内受限视图（bool 语义用于 EFAULT 上报），`SyscallHook`/`CodeHook`=回调 trait。mem_read/mem_write/reg_* 在 `Engine` 上返回 `Result`。
- `runtime/backends/unicorn`：`mem_protect` 已实现，syscall hook 不吞目标侧读写失败。
- `runtime/elf/parser`：`DynamicInfo` 已输出 `soname: Option<String>` 与 `needed: Vec<String>`；错误分 BadMagic/Truncated/MalformedDynamic。
- `runtime/elf/loader`：输出 `LoadedModule.relro: Option<RelroRange>`（当前为近似范围）。
- `runtime/elf/linker`：`resolve()` 基于 requester 依赖图顺序；`init_order` 拓扑稳定。
- `runtime/os/linux`：`mmap`/`read`/`getrandom` 已建立目标侧映射/回写；回写失败上抛 EFAULT。
- `runtime/case-runner`：固定 scratch buffer；应用 seed；manifest 参数校验。

## Risks

### 1. 过早把 correctness change 扩成完整 syscall/VFS 重写

不是本次目标。本次只补 task 16（mmap 真实回归 case），可选增强按需。

### 2. design 过时导致破坏性改动

本 change 原design 的若干"命名收敛"建议（Engine→Backend、SyscallCpu 批评等）描述的是 7 天前的代码形态，与现状冲突。已在本 design 中改为"现状对齐"。**禁止**按过时建议做破坏性改名。

### 3. 引入过多 unsafe

`LinkCtxAdapter` 已有裸指针绕借用。本次补 case 不应扩大 unsafe 边界。

## Acceptance Direction

- task 16：新增 mmap 回归 case，通过真实 backend 证明返回地址可读/可写（而非 mock map_guest）。
- 其余 spec Requirement 已由后续 change 满足，对应 task 在 tasks.md 标注并勾选。
- 可选增强（pread64 / RELRO 精度 / alloc_scratch / pc_read）按需决定，不阻塞本 change 收尾。
