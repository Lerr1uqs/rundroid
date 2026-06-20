# rundroid 验收标准

## 文档目的

本文档定义 `rundroid` 当前阶段的验收标准。  
这里的重点不是“代码写了多少”，而是“系统是否达到了可接受的行为结果”。

## 总体验收原则

验收遵循四个原则：

1. 结果优先于形式
2. 目标侧结果优先于返回值
3. 显式行为优先于隐式副作用
4. 稳定性优先于偶然跑通

也就是说：

- 不能只看 syscall 返回值
- 不能只看日志有无输出
- 不能只看某个 case 在某次运行里侥幸通过

## 一、执行层验收

### 1. Backend 抽象

满足以下条件视为通过：

- runtime 不直接把核心执行逻辑写死到 Unicorn API
- `Backend` 能统一提供寄存器、目标内存、PC、stop、页权限能力
- 关键内存访问失败会返回错误，而不是静默吞掉

### 2. Unicorn backend

满足以下条件视为通过：

- ARM64 最小 workload 能执行
- `mem_protect` 真正生效
- 目标内存写入失败时 syscall 路径能上抛失败

## 二、ELF 装载与链接验收

### 1. Parser

满足以下条件视为通过：

- 能输出 `soname`
- 能输出 `needed`
- 能区分基础错误类型
- relocation 数据模型被归一化

### 2. Loader

满足以下条件视为通过：

- 能完成单模块地址空间保留
- 能写入段数据与零填充
- 能提供后续 link 所需模块对象
- 不在 loader 阶段偷做跨模块符号解析

### 3. Linker

满足以下条件视为通过：

- 能依据依赖图解析符号
- relocation 写回真实落到目标内存
- init 顺序稳定
- 不依赖全局模块扫描碰运气

### 4. DT_NEEDED 装载队列

满足以下条件视为通过：

- root module 能通过 `module_uri` 装入
- `DT_NEEDED` 能驱动 direct dependency 装入
- 依赖 closure 能形成模块图
- `resolve()` 顺序稳定
- 结果不依赖 `HashMap` 遍历顺序

## 三、运行时正确性验收

### 1. 目标缓冲区写入正确性

满足以下条件视为通过：

- `read`
- `pread64`
- `getrandom`

这类路径在返回成功时，目标缓冲区内容必须真实变化。

以下情况视为不通过：

- 返回长度正确但目标缓冲区未写入
- 数据源成功但回写失败仍返回成功

### 2. mmap 正确性

满足以下条件视为通过：

- `mmap` 成功时返回区间在目标侧真实可访问
- file-backed / device-backed `mmap` 需要初始内容时已正确落地

以下情况视为不通过：

- 仅返回一个地址
- 地址不可读写
- 仅 host 侧记录了映射但目标侧没有真实映射

### 3. 页权限与 RELRO

满足以下条件视为通过：

- 装载后权限会按段语义收紧
- relocation 完成后 `RELRO` 至少在已支持 backend 上生效

以下情况视为不通过：

- 长期保留全局 RWX
- 只记日志不真正调用 backend

### 4. manifest 生效

满足以下条件视为通过：

- `arch`
- `backend`
- `seed`
- `telemetry`

这些字段确实影响运行结果，或在不支持时显式报错。

以下情况视为不通过：

- manifest 字段解析了但没用
- 配置非法但静默忽略

## 四、文件系统与设备验收

### 1. 挂载面

满足以下条件视为通过：

- 同时支持 `map_file`
- 同时支持 `map_device`
- 当前阶段不要求 rootfs 也能工作

### 2. VirtFile

满足以下条件视为通过：

- `VirtFile.host(...)`
- `VirtFile.bytes(...)`
- 动态 provider

三类来源共享统一读语义和统一目标侧回写语义。

### 3. builtin device

满足以下条件视为通过：

- `/dev/urandom`
- `/dev/random`
- `/dev/srandom`
- `/dev/null`
- `/dev/zero`

至少具备稳定挂载与基本行为。

### 4. 设备实例生命周期

满足以下条件视为通过：

- `open` 返回 per-fd 实例
- fd table 保存实例句柄
- 后续 `read/write/ioctl/mmap/close` 按 fd 分发

以下情况视为不通过：

- 每次操作都重新按路径猜设备
- 设备状态无法随 fd 保存

## 五、Python stub 验收

### 1. decorator 语义

满足以下条件视为通过：

- `@device(...)`
- `@file_node(...)`

能够声明元数据与默认虚拟路径。

以下情况视为不通过：

- import 模块时自动污染 runtime 全局状态

### 2. 注册主线

满足以下条件视为通过：

- Python 能通过 FFI/binding 显式注册文件与设备
- `register(runtime, ...)` 可以作为语法糖存在
- 最终挂载仍由 Rust runtime 完成

以下情况视为不通过：

- Python 私自持有核心 mount table
- Python 私自接管 fd 生命周期
- Python 直接提交最终目标内存状态

### 3. Python 工程管理

满足以下条件视为通过：

- Python 目录具备 `pyproject.toml`
- 能通过 `uv` 安装依赖
- 能通过 `uv run` 执行 case 或 pytest

## 六、测试与回归验收

### 1. smoke cases

当前至少应有：

- 纯导出调用 case
- `/dev/urandom` case
- `VirtFile.bytes(...)` case
- `VirtFile.host(...)` case
- `mmap` case

### 2. 回归断言强度

满足以下条件视为通过：

- case 会回读目标缓冲区
- case 会检查映射可访问性
- case 会验证文件/设备返回结果真实落地

以下情况视为不通过：

- 只断言返回值
- 只断言日志
- 只断言“程序没崩”

### 3. scratch memory

满足以下条件视为通过：

- scratch API 明确标注为 harness/stub/debug 用途
- 不被当作正式目标堆使用

以下情况视为不通过：

- 正常运行时主路径偷偷依赖 scratch allocator

## 七、可观测性验收

### 1. 结构化事件

满足以下条件视为通过：

- load/link 有结构化事件
- file/device 路径有结构化事件
- 失败事件可归因

### 2. 错误可归因

满足以下条件视为通过：

- parse error、load error、link error 分层明确
- 设备错误能关联到虚拟路径和 fd
- syscall 失败能定位到目标缓冲区/目标地址问题

## 八、当前阶段不作为通过前提的内容

以下内容不是当前阶段的强制通过条件：

- 完整 JNI
- 完整 Java VM
- 完整 rootfs
- 全量 Android 特殊设备
- 全功能 GDB/LLDB
- 全量 backend 矩阵

## 最终通过标准

当前阶段可视为“通过”的最低条件是：

1. ARM64 最小执行主线稳定
2. `DT_NEEDED` 依赖图真实生效
3. 目标缓冲区写入语义可信
4. `mmap` 语义可信
5. 文件与设备挂载主线清晰
6. Python stub 注册主线成立
7. smoke/regression case 能稳定复现并断言目标侧结果

如果以上七项有明显短板，就不能视为 `rundroid` 当前阶段已经达到可交付实现基线。
