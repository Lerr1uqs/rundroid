# rundroid 功能设计总览

## 文档目的

本文档从“项目要实现哪些功能”角度描述 `rundroid`。  
它不展开 trait 或代码接口，而是给实现方、评审方和后续拆分 agent 一个统一的功能地图。

## 总体目标

`rundroid` 的目标不是复刻 Java 版 `unidbg` 的工程形态，而是建立：

- Rust 核心执行层
- Rust Android/Linux 运行语义层
- Python 脚本扩展层
- 面向回归和测试的可观测平台

当前阶段只考虑 Android Native 运行主线，不展开完整 Java VM。

## 功能域

当前项目功能分成八个域：

1. 执行与内存控制
2. ELF 装载与链接
3. Linux/Android 运行语义
4. 文件系统与虚拟设备
5. Python stub 扩展
6. 测试与回归
7. 遥测与调试
8. 资源与模块来源管理

## 1. 执行与内存控制

### 1.1 Backend 抽象

系统需要一个独立于 Unicorn 的 `Backend` 抽象层，统一封装：

- 寄存器读写
- 目标内存读写
- PC 控制
- 执行停止
- 页权限调整

### 1.2 Unicorn backend

bootstrap 阶段优先支持 Unicorn backend。

要求：

- 不是把 Unicorn API 直接泄漏到全局
- 正常承载最小 ARM64 执行路径
- 支持页权限收紧
- 目标内存写失败不得静默吞错

## 2. ELF 装载与链接

### 2.1 Parser

Parser 负责：

- ELF 基本结构读取
- Dynamic 信息读取
- `DT_SONAME`
- `DT_NEEDED`
- relocation 归一化

### 2.2 Loader

Loader 负责：

- 目标内存布局
- PT_LOAD 映射
- 段数据写入
- 零填充
- TLS/RELRO 基础输入准备

### 2.3 Linker

Linker 负责：

- 依赖图上的符号解析
- relocation 写回
- init 顺序生成
- 与页权限收紧协作

### 2.4 依赖图装载

系统必须支持：

- root module 装载
- `DT_NEEDED` 递归依赖装载
- direct dependency / dependency closure
- 稳定的解析顺序

系统不允许：

- 仅靠全局模块扫描解析符号
- 让 `HashMap` 遍历顺序决定结果

## 3. Linux/Android 运行语义

### 3.1 OS 语义层

系统需要 `OS` 作为统一运行语义层。

它负责：

- syscall 分发
- `FileDescriptorTable`
- `FileDescriptorEntry`
- 地址空间协作
- `mmap`
- `read/pread`
- `getrandom`
- fd 生命周期

### 3.2 运行时正确性

系统成功语义必须以目标侧结果为准：

- 目标缓冲区真实变化
- 目标内存真实可访问
- `mmap` 真正建立
- 页权限真正收紧

### 3.3 manifest 生效

case manifest 至少应支持：

- `arch`
- `backend`
- `seed`
- `telemetry`

并要求：

- 生效或显式报错
- 不能“字段存在但运行时没用”

## 4. 文件系统与虚拟设备

### 4.1 显式挂载表

当前阶段采用显式挂载表，不强制 rootfs。

核心挂载面：

- `map_file(virtual_path, VirtFile(...))`
- `map_device(virtual_path, device)`

### 4.2 VirtFile

普通文件节点统一通过 `VirtFile` 表达：

- 宿主文件来源
- 内存字节来源
- 动态 regular file provider

### 4.3 VirtualDevice

设备节点统一通过 `VirtualDevice` 表达。

至少覆盖：

- `open`
- `read`
- `write`
- `ioctl`
- `mmap`
- `fstat`
- `close`

### 4.4 builtin devices

bootstrap 阶段至少内建：

- `/dev/urandom`
- `/dev/random`
- `/dev/null`
- `/dev/zero`
- `/dev/ashmem`

### 4.5 FileDescriptorTable 与 FileDescriptorEntry

系统需要一个显式的 `FileDescriptorTable` 来管理所有 fd。

其中 `FileDescriptorEntry` 代表单个描述符槽位，至少保存：

- fd 编号
- kind
- handle 引用
- descriptor flags
- 可选的 `virtual_path` 诊断信息

这意味着：

- `open`、`socket`、`pipe`、`eventfd` 都进入同一张表
- syscall 先解析 `FileDescriptorEntry`，再分发到对应 handle
- `dup/dup2/dup3` 产生新的 `FileDescriptorEntry`，而不是重跑路径匹配

### 4.6 设备实例模型

设备系统必须区分：

- device definition
- per-open device instance

`FileDescriptorTable` 保存的是实例句柄引用，而不是仅路径信息。

### 4.7 挂载冲突规则

同一个虚拟路径不允许同时注册多个节点来源。

要求：

- `map_file` 与 `map_file` 同名冲突时立即报错
- `map_device` 与 `map_device` 同名冲突时立即报错
- `map_file` 与 `map_device` 同名冲突时立即报错
- 不允许静默覆盖

## 5. Python stub 扩展

这一部分建议作为 follow-up change 独立推进，不与 Rust driver/VFS 主线混在同一个实现变更中。

### 5.1 Python 角色

Python 层负责：

- 声明 mock/stub/device/file provider
- 编写 case 逻辑
- 快速扩展复杂行为

Python 层不负责：

- 持有最终 OS 核心状态
- 直接提交目标内存写入
- 接管 fd / mount table 真正生命周期

### 5.2 decorator 模式

系统需支持：

- `@device("/dev/urandom")`
- `@file_node("/proc/version")`

decorator 可提供：

- 类型元数据
- 能力元数据
- 默认虚拟路径

但不允许 import 时自动挂载。

### 5.3 FFI/binding 注册

Python stub 需要通过 Rust 暴露的 binding/FFI 注册：

- `map_file`
- `map_device`
- 或 `register(runtime, ...)`

### 5.4 Python 工程管理

Python 侧默认使用 `uv` 管理：

- 依赖
- 运行入口
- pytest
- 类型工具

## 6. 测试与回归

### 6.1 case 系统

系统应采用 case 驱动测试。

case 至少覆盖：

- 纯导出调用
- `/dev/urandom` 读取
- `VirtFile.bytes(...)`
- `VirtFile.host(...)`
- `mmap`
- 路径冲突报错

### 6.2 scratch memory

系统可提供 scratch memory 给：

- case runner
- Python stub
- 调试/验证逻辑

但它只能是 harness API，不能替代真实目标堆。

### 6.3 回归断言重点

回归不能只断言返回值，还要断言：

- 目标缓冲区内容
- `mmap` 可访问性
- 页权限收紧结果
- 依赖图是否真实参与 link

## 7. 遥测与调试

### 7.1 结构化 telemetry

系统需要统一事件流，而不是散落 print。

至少覆盖：

- load
- link
- syscall
- file IO
- device IO
- error

### 7.2 调试友好性

当前阶段“调试支持”的核心不是完整 IDE 集成，而是：

- 事件可观察
- 行为可回放
- 错误可归因

后续可继续扩展：

- tracing 协议
- gdb/lldb 对接
- 更细粒度 hook 观测

## 8. 资源与模块来源管理

系统需要一个资源解析模型，用于：

- 根模块定位
- `DT_NEEDED` 依赖递归定位
- case 资源文件解析

统一入口通过：

- `module_uri`
- `resolver`
- resource packs

## 当前优先级

当前优先级不是把能力面无限铺大，而是先收敛正确性主线：

1. 目标内存可观察正确
2. `DT_NEEDED` 依赖图正确
3. 页权限与 `RELRO` 正确
4. 文件/设备路径正确
5. 路径冲突规则正确
6. smoke case 与 regression case 可信

## 不在当前阶段强制要求的内容

当前阶段不强制要求：

- 完整 Java VM
- 完整 JNI 全量覆盖
- 完整 rootfs
- 所有 Android 特殊设备
- 全功能 gdb/lldb
- 所有 backend 一次到位

## 一句话总结

`rundroid` 当前应优先做成一个：

- Rust-first
- Android Native-only
- 目标侧结果可信
- Python stub 易扩展
- 文件/设备模型清晰
- 回归测试可重复

的运行时与验证平台。
