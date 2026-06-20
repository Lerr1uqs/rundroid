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

对应参考位置：

- [AndroidElfLoader.java](</F:/reverse-workspace/unidbg/unidbg-android/src/main/java/com/github/unidbg/linux/AndroidElfLoader.java:317>)
- [LinuxModule.java](</F:/reverse-workspace/unidbg/unidbg-android/src/main/java/com/github/unidbg/linux/LinuxModule.java:87>)
- [ModuleSymbol.java](</F:/reverse-workspace/unidbg/unidbg-android/src/main/java/com/github/unidbg/linux/ModuleSymbol.java:15>)

结论：

- `unidbg` 不是“把所有模块扫一遍看看谁有符号”，而是先建立依赖上下文，再做符号解析。
- `rundroid` 应复用这个原则，但用 Rust 的 `ModuleGraph` 与 trait 做得更清晰。

### 2. 段映射与页权限

`unidbg` 并不是简单粗暴把整块镜像永久映射成 RWX。

它会：

- 按段计算页对齐后的区间
- 对共享页做权限合并或 `mem_protect`
- 把真实段数据写入目标内存
- 维护 `MemRegion`

这说明：

- “先整块 reserve，再按段收紧权限”是合理路线
- “整块 RWX 然后不再收紧”只是临时跑通手段，不能作为长期语义

### 3. 文件 IO / `mmap` / `/dev/urandom`

`unidbg` 的文件 IO 路径最终会把读出的字节写入目标缓冲区，例如：

- `RandomFileIO.read(...)`
- `ByteArrayFileIO.read(...)`
- `AbstractFileIO.mmap2(...)`

这些路径的共同点是：

- 目标侧可观察状态是真实结果的一部分
- 返回值正确但目标缓冲区没变，不算成功

这也是 `rundroid` 当前最需要修正的点之一。

在 `rundroid` 里，这一段不能只停留在“数据源能返回 bytes”。

实现上必须固定成下面这条主线：

1. file/device source 先产出源字节或映射描述
2. runtime 尝试把结果写回目标缓冲区，或建立目标侧映射
3. 只有步骤 2 成功，syscall 才能返回成功

具体约束：

- `VirtFile.host(...)`、`VirtFile.bytes(...)`、builtin `/dev/urandom`、custom device 的 `read/pread64` 都必须走同一条目标侧回写主线
- 回写失败时，不允许保留“返回长度但目标缓冲区没变化”的假成功
- `mmap` 不允许只分配一个 host side 句柄或只计算返回地址，必须让 backend 中的目标页真实可访问
- file-backed / device-backed `mmap` 若需要初始内容，必须在返回前完成初始字节落地或显式失败

验收上也必须对应加强：

- `/dev/urandom` case 不只检查返回值，还要回读目标缓冲区，确认字节真实可见
- `VirtFile.bytes(...)` / `VirtFile.host(...)` case 必须回读目标缓冲区，断言长度和内容
- `mmap` case 必须证明返回地址可读或可写，而不是只证明 syscall 没报错

## 最佳实现建议

### 1. 执行环境的目标侧状态访问必须 fail-fast

当前把这层接口命名成 `SyscallCpu` 语义不对：

- 它不是“只给 syscall 用”的接口
- 它也不只是 CPU，而是同时覆盖 register、memory 和 stop control

更合理的收敛方式是：

- 直接使用 `Backend` 作为统一执行环境接口命名
- 它同时封装 backend/unicorn 侧执行能力与 OS/syscall 侧所需的寄存器、内存、PC、stop 控制能力
- syscall handler 直接依赖 `&mut dyn Backend`，不再额外引入误导性的 `SyscallCpu` 命名

当前这类 `mem_read()` / `mem_write()` 的 best-effort 语义不适合 correctness 阶段。

建议改成：

```rust
pub trait Backend {
    fn reg_read(&self, reg: Arm64Reg) -> Result<u64, BackendError>;
    fn reg_write(&mut self, reg: Arm64Reg, value: u64) -> Result<(), BackendError>;
    fn mem_read(&self, addr: u64, buf: &mut [u8]) -> Result<(), BackendError>;
    fn mem_write(&mut self, addr: u64, bytes: &[u8]) -> Result<(), BackendError>;
    fn pc_read(&self) -> Result<u64, BackendError>;
    fn pc_write(&mut self, value: u64) -> Result<(), BackendError>;
    fn stop(&mut self) -> Result<(), BackendError>;
}
```

然后：

- Linux runtime 的 `dispatch()` 返回 `Result<SyscallResult, SyscallError>`
- `Backend` 统一承载 register / memory / pc / execution control，以及 syscall 所需的访问主线
- 任何目标缓冲区读写失败都直接进入失败路径
- case runner 不能把“返回值正确但目标内存没写进去”判成 pass

### 2. 增加 scratch memory / call buffer 管理

当前 case 直接传 `0x800000` 这种裸地址，不够可靠。

建议在 `OS` 增加一个最小 scratch allocator：

- `alloc_scratch(size, perms) -> target_addr`
- `write_scratch(addr, bytes)`
- `read_scratch(addr, len)`

这样 case runner 可以：

- 先申请目标缓冲区
- 把指针传进导出函数
- 调用结束后回读目标内容并断言

这比让 case 手工写裸地址稳定得多。

这里必须明确它的边界：

- scratch allocator 是 testing-harness / case runner / Python stub 的辅助能力
- 它不是目标程序正式堆的一部分
- 它也不是 `malloc` / `free` / `mmap` / `brk` 的替代实现

也就是说：

- 正常目标程序的动态内存语义，仍应走目标程序自己的分配路径
- scratch 只解决“测试脚本要给 native 调用准备一块可控目标缓冲区”这个问题

推荐命名上也体现这一点，例如：

- `os.testing.alloc_scratch(...)`
- `os.harness.alloc_scratch(...)`

而不是让它看起来像正式 userspace allocator。

### 3. 建立真实的 `DT_NEEDED` 装载队列

建议把当前的 `load_and_link(module_name, bytes)` 演进为：

```rust
pub fn load_root_module(&mut self, module_uri: &str) -> Result<ModuleId, RuntimeAssemblyError>;
```

这里的 `module_uri` 不是“某个神秘的新协议”，它只是一个比裸文件名更稳定的资源定位符。

它的作用是：

- 告诉 loader “根模块从哪里来”
- 让 case/harness/resource resolver 能用统一入口加载模块
- 避免把 loader API 绑死在“只能传本地路径”或“只能直接给 bytes”上

例如都可以接受：

- `resource:cases/so/libfoo.so`
- `file:F:/samples/libfoo.so`
- `apk:/lib/arm64-v8a/libfoo.so`

bootstrap 阶段并不要求一次支持所有 scheme，但接口层最好先按 URI/resolver 模型设计。

它背后的含义其实很简单：

- `module_name` 更像显示名或调用方随便起的名字
- `module_uri` 才是“这个模块实际从哪里解析和读取”

之所以要这样改，是因为真实 `DT_NEEDED` 装载不是“当前模块自己跑一遍 link 就完了”，而是：

- 先明确 root module 的来源
- 再根据它的 `DT_NEEDED` 继续查 resolver
- 逐步把依赖图装完整
- 最后在一个稳定依赖上下文里做 link

如果还停留在 `load_and_link(module_name, bytes)` 这种接口，很容易让实现方偷懒成：

- 只 link 当前给进来的一个模块
- 依赖模块靠“之后谁先 load 到内存里”碰运气
- `resolve()` 直接扫全局模块表

这就是我们想避免的错误语义。

推荐流程：

1. 解析 root module
2. 取出 `DT_SONAME`
3. 取出 `DT_NEEDED`
4. 用 `resource:` / resolver 找到依赖模块
5. 对未装载依赖递归执行 parser + loader
6. 建立 `ModuleGraph.deps`
7. 等图完整后再统一调用 linker

关键约束：

- 不允许靠“扫描所有已装载模块”替代依赖图
- `resolve()` 必须先 self，再 direct deps，再按依赖闭包的稳定顺序继续
- 不允许用 `HashMap` 的偶然遍历顺序决定结果

### 4. parser 必须给出字符串化的 soname / needed

当前只保留 strtab offset 不够。

建议 parser 的 `DynamicInfo` 至少演进为：

```rust
pub struct DynamicInfo {
    pub soname: Option<String>,
    pub needed: Vec<String>,
    pub init: Option<u64>,
    pub fini: Option<u64>,
    pub init_array: Option<u64>,
    pub init_array_size: u64,
    pub fini_array: Option<u64>,
    pub fini_array_size: u64,
    pub relro: Option<RelroTemplate>,
}
```

这一步不是“功能扩张”，而是让 loader/linker 拿到足够正确的输入。

### 5. 页权限与 RELRO 的正确落地方式

推荐采用两阶段模型：

1. `reserve_image_space()` 把 footprint 整块映射成临时可写
2. loader 写完段内容、linker 写完 relocation 后：
   - 依据段权限做 `mem_protect`
   - 对 RELRO 区域做最终只读收紧

建议新增 backend trait：

```rust
fn mem_protect(&mut self, addr: u64, size: usize, perms: MemPerms) -> Result<(), BackendError>;
```

然后：

- `LoadContext` 只负责初始映射和写入
- `LinkContext::protect_relro()` 必须真正调用 backend
- 失败时必须返回 error，不能只记一条 event

### 6. case manifest 必须变成“强生效输入”

`arch` / `backend` / `seed` / `telemetry` 都必须真正作用于 OS。

建议：

- 不支持的 `arch` / `backend` 直接在 manifest 校验阶段报错
- `seed` 必须在 OS 装配后立刻应用到 Linux OS
- `telemetry = full` 如果当前未实现完整能力，也应保持结构一致并至少输出更多字段，而不是仅与 `events_only` 等价

### 7. smoke case 需要从“只看返回值”升级为“检查目标侧状态”

建议把现有 case 升级为：

- `01-pure-export-call`
  - 保持现状，验证导出调用和 ABI
- `02-open-and-read-urandom`
  - open `/dev/urandom`
  - read 到目标缓冲区
  - 回读目标缓冲区，断言数据非零且长度正确
- `03-mmap-write-read`
  - 目标代码调用 `mmap`
  - runtime 确保该地址真的可写
  - 函数返回后从目标侧回读，断言页已可见

## Architecture Changes

本次 change 建议新增或修改的 crate 内职责如下。

### `runtime/backends/api`

- 新增 `mem_protect`
- `Backend` 的 register / memory / pc 访问接口改为 `Result`

### `runtime/backends/unicorn`

- 实现 `mem_protect`
- syscall hook 里不再吞目标侧读写失败

### `runtime/elf/parser`

- `DynamicInfo` 输出 `soname: Option<String>` 与 `needed: Vec<String>`
- bad magic / truncated / malformed dynamic 区分更准确

### `runtime/elf/loader`

- 输出真实的 RELRO 模板信息
- 不再把 `module_name` 当成 soname 替代品

### `runtime/elf/linker`

- `resolve()` 必须基于依赖图顺序
- `init_order` 继续保持稳定拓扑序

### `runtime/os/linux`

- `mmap` 不能只返回地址，至少要能与 backend 映射协同
- `/dev/urandom` / `getrandom` 写目标缓冲区失败要上抛

### `runtime/case-runner`

- 管理 scratch memory
- 应用 `seed`
- manifest 参数校验
- 增加目标缓冲区回读断言能力

## Risks

### 1. 过早把 correctness change 扩成完整 syscall/VFS 重写

这不是本次 change 的目标。

本次只要求：

- 目标内存可见性正确
- `DT_NEEDED` 最小依赖图正确
- 页权限最小正确

### 2. 引入过多 unsafe

当前 `LinkCtxAdapter` 里已经有裸指针绕借用检查的做法。

本次 change 应尽量减少 unsafe 边界，而不是继续扩大。

优先策略：

- 重排 API 所有权
- 抽出 snapshot 数据
- 最后才考虑增加新的裸指针桥接

## Acceptance Direction

这个 change 完成时，至少应满足：

- `getrandom` / `read` 类路径在目标缓冲区未成功写入时不会返回 pass
- `DT_NEEDED` 的 direct dependency 能被装载并参与符号解析
- `resolve()` 不再扫描所有模块决定结果
- `mem_protect` 与 RELRO 至少在 Unicorn backend 生效
- case manifest 的 `seed` / `arch` / `backend` 参数真实生效
- 新的 smoke case 会断言目标侧内存状态
