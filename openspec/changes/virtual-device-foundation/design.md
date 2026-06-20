## Context

关于 file IO / driver simulation，这里有三个参考系：

1. `unidbg` 当前做法
2. 之前 `agent-output/unidbg-rs-spec` 的理想接口
3. 用户提出的 qiling 风格“路径挂载 + 类行为”

三者对照之后，最合理的方案不是三选一，而是组合：

- 路径驱动挂载，采用 qiling 风格的人机接口
- 生命周期、fd、`ioctl`、`mmap` 分发，采用 `ub-driver` 风格的清晰抽象
- 避免 `unidbg` 当前那种 `if "/dev/urandom".equals(path)` 的硬编码扩散

## 参考实现：unidbg 当前做法的问题

`unidbg` 当前 Android 路径里，`DriverFileIO.create()` 直接按虚拟路径做硬编码分派：

- `/dev/urandom` -> `RandomFileIO`
- `/dev/null` -> `DriverFileIO`
- `/dev/ashmem` -> `Ashmem`
- `/dev/zero` -> `ZeroFileIO`

参考位置：

- [DriverFileIO.java](</F:/reverse-workspace/unidbg/unidbg-android/src/main/java/com/github/unidbg/linux/file/DriverFileIO.java:17>)

这种方式的问题很明显：

- 路径匹配和行为分发都写死在一个工厂里
- 新增设备要改核心分支
- Python 层没有天然扩展点
- builtin 与 custom device 没有统一注册机制

所以它更像“bootstrap 快速可用方案”，不是 `rundroid` 应长期保持的结构。

## 对照 agent-output 设计的结论

之前的 `agent-output/unidbg-rs-spec` 在这块的抽象其实是对的：

- `runtime.fs.map_device(virtual_path, factory)`
- `VirtualDevice` trait / Python base class
- `DeviceRegistry`
- `DeviceFactory`
- fd 生命周期由 Rust 持有
- 新设备不需要改 syscall 核心

对应参考：

- [01-system-spec.md](</F:/reverse-workspace/unidbg/agent-output/unidbg-rs-spec/01-system-spec.md:692>)
- [03-hook-driver-interfaces.md](</F:/reverse-workspace/unidbg/agent-output/unidbg-rs-spec/interfaces/03-hook-driver-interfaces.md:174>)

所以用户这次提出的“类行为 + 显式挂载”方向本身是合理的。

但当前 change 不应一次把 Rust driver 重构和 Python binding 一起打包。

这一轮应先收敛为 Phase 1：

- Rust `runtime/driver`
- VFS 挂载表
- builtin devices
- fd handle 分发
- 路径冲突 fail-fast

Python binding、decorator 和 FFI 回调应作为 follow-up change 单独推进。

## 结论方案

### 1. 总体原则

file IO / device 模拟必须拆成四层：

1. syscall 分发层
2. VFS / path resolution 层
3. device registry / factory 层
4. per-fd device handle 层

这四层的职责必须固定：

- syscall 只看 fd 和操作类型，不看设备路径字符串
- VFS 只负责把路径解析成 node
- device registry 只负责 path -> factory / class metadata
- per-fd handle 负责 `read/write/ioctl/mmap/close` 的真实行为

同时，VFS 的公开挂载面建议固定为：

- `runtime.fs.map_file(virtual_path, VirtFile(...))`
- `runtime.fs.map_device(virtual_path, device_class_or_factory)`

并且需要明确一条规则：

- 同一个虚拟路径不允许重复挂载
- `map_file` 和 `map_device` 若命中同名路径，必须立即报错退出
- 不允许静默覆盖或按“后注册优先”处理

其中：

- `VirtFile.host(...)` 表示宿主文件来源
- `VirtFile.bytes(...)` 表示内存字节来源
- `VirtFile` 子类可表示动态生成的 regular file

也就是说：

- host file 与 bytes file 统一为“文件节点”
- device 仍单独保留，不应混进 `VirtFile`

这里还要补一个之前写得不够硬的边界：

- 普通文件节点不是“只要能拿到 bytes 就行”
- device 节点也不是“回调返回长度就行”

两者都必须以目标侧可观察结果定义成功：

- `read/pread` 成功，意味着目标缓冲区里真的出现了对应字节
- `mmap` 成功，意味着目标侧真的出现了可访问映射
- 如果 backend writeback 或映射失败，source/provider/device 侧即使拿到了数据，也必须整体失败

### 2. Python binding 延后到 follow-up change

Python decorator、Python binding、`PyO3`/FFI 回调这部分是合理方向，但不适合继续留在当前 change 里。

原因：

- 当前仓库没有 Python 实现基础
- 这部分会显著扩大 review 面
- 它和 Rust driver/VFS 主线是可拆分的

所以当前 change 只保留 Rust Phase 1。

后续独立 change 再处理：

- Python `VirtualDevice`
- Python `VirtFile`
- decorator 元数据
- FFI/binding 注册
- `uv` 工程管理

### 3. builtin 设备与 custom 设备的关系

decorator 应该只做“声明类元数据”，不应在 import 时偷偷挂到 runtime 全局表里。

推荐 Python 形态：

```python
from rundroid.drivers import VirtualDevice, device

@file_node("/system/build.prop")
class BuildProp(VirtFile):
    @classmethod
    def bytes(cls) -> bytes:
        return b"ro.product.model=Pixel\n"
```

或者动态 regular file：

```python
@file_node("/proc/self/maps", kind="regular")
class ProcSelfMaps(VirtFile):
    def read(self, ctx, size: int) -> bytes:
        return self.render_maps(ctx.runtime)
```

但这一步只是在类上挂 metadata。

真正让它生效的方式应是显式调用：

```python
runtime.fs.map_file("/system/build.prop", VirtFile.bytes(b"ro.debuggable=1\n"))
runtime.fs.map_file("/data/local/tmp/libfoo.so", VirtFile.host("F:/samples/libfoo.so"))
runtime.fs.map_file("/proc/self/maps", ProcSelfMaps)
```

或者：

```python
def register_devices(runtime):
    runtime.fs.map_file("/proc/self/maps", ProcSelfMaps)
```

这样做的好处：

- case 生命周期明确
- 不会因为 import 一个模块就全局污染
- 更适合回归和并行测试
- 同一个设备类可以挂到多个路径，而不必把路径列表绑死在类定义上

这里要再强调一次：

- decorator 适合 `VirtFile` 子类和 `VirtualDevice` 子类
- `VirtFile.host(...)` / `VirtFile.bytes(...)` 这种值对象本身不需要 decorator
- decorator 可以带默认虚拟路径
- 但默认虚拟路径仍然只是 metadata，不是 import 时自动挂载命令

所以 Python 侧实际会有两种入口并存：

1. 显式路径入口

```python
runtime.fs.map_device("/dev/urandom", MyUrandom)
runtime.fs.map_file("/proc/version", VirtFile.bytes(b"Linux version 5.x\n"))
```

2. decorator 默认路径入口

```python
@device("/dev/urandom")
class MyUrandom(VirtualDevice):
    def read(self, ctx, size: int) -> bytes:
        return b"\x41" * size

MyUrandom.register(runtime)
```

或者：

```python
register_devices(runtime, [MyUrandom, ProcSelfMaps])
```

这两种入口最终都要收敛到 Rust runtime 的同一条注册主线。

最好的方式是混合模型。

#### builtin 设备

这些建议优先用 Rust 实现：

- `/dev/urandom`
- `/dev/random`
- `/dev/null`
- `/dev/zero`
- `/dev/ashmem`

原因：

- 性能更稳定
- 目标内存读写和 `mmap` 行为更容易做强校验
- case 的确定性更好控制

#### custom 设备

当前 change 先不引入 Python custom device。  
后续 change 再把 custom device 扩到 Python 脚本层。

#### override 规则

Rust 侧显式挂载的 custom device 应优先于 builtin。

推荐优先级：

1. case/runtime 显式 map 的 device
2. runtime 预装的 builtin device
3. 普通 file / bytes / host file 映射

说明：

- builtin 的存在由 Rust 运行时启动时注册决定
- 不需要在 Python decorator 上额外声明 `builtin=...`
- `builtin/` 目录中的实现和 runtime 启动时的预注册逻辑就足够表达“这是内置设备”

### 4. 生命周期模型

不要把“device class”和“fd 实例”混为一谈。

推荐模型：

- registry 保存 `DeviceFactory`
- `open(path)` 时调用 factory，生成一个 per-open device instance
- `FileDescriptorTable` 保存 `FileDescriptorEntry`
- `FileDescriptorEntry` 持有 `FdKind` 与 handle 引用
- 后续 `read/write/ioctl/mmap/close` 都按 handle 分发

这点非常关键，因为：

- `/dev/urandom` 是近似无状态设备
- `/dev/ashmem`、某些 fake sensor、binder-like 设备则可能有 per-fd 状态

所以必须明确：

- class / factory 是“设备定义”
- instance / handle 是“这次 open 出来的会话状态”
- `FileDescriptorEntry` 是“当前 fd 槽位的引用与元数据”

#### 4.1 `FileDescriptorTable` / `FileDescriptorEntry` 的职责

`FileDescriptorTable` 应该属于 `OS` 层，而不是 `VirtualDevice` 本身。

它的职责是：

- 管理 `fd -> FileDescriptorEntry` 的映射
- 统一承接 `open`、`socket`、`pipe`、`eventfd`
- 为 `read/write/ioctl/mmap/fstat/close` 提供统一入口
- 在 `close` 后移除条目，并控制 fd 何时可被重用

`FileDescriptorEntry` 则是单个 descriptor slot。

它至少应表达：

- `fd`
- `kind`
- `handle_ref`
- `descriptor_flags`
- `virtual_path: Option<String>`，仅用于路径来源对象的可观测与诊断

需要特别强调：

- `FileDescriptorEntry` 不是 file/device/socket 行为对象
- `FileDescriptorEntry` 不负责路径解析
- `FileDescriptorEntry` 只负责把一个 fd 槽位稳定地指向某个已打开 handle

推荐行为语义：

- `open` 为 path-derived node 创建 handle，并插入一个新的 `FileDescriptorEntry`
- `socket/pipe/eventfd` 也走同一张 `FileDescriptorTable`
- `dup/dup2/dup3` 创建新的 `FileDescriptorEntry`
- 新旧 `FileDescriptorEntry` 默认应指向同一 opened handle，descriptor 级 flags 保持独立
- 只有当最后一个引用该 handle 的 `FileDescriptorEntry` 被关闭时，底层 handle 才真正执行 `close`

### 5. Rust 侧推荐接口

建议新增 crate：`runtime/driver`

建议文件布局：

```text
runtime/driver/
  src/
    lib.rs
    device.rs
    registry.rs
    mapper.rs
    context.rs
    builtin/
      mod.rs
      urandom.rs
      zero.rs
      null.rs
      ashmem.rs
runtime/os/linux/
  src/
    file_descriptor_table.rs
    file_descriptor_entry.rs
    syscall.rs
```

核心 trait 建议：

```rust
pub trait VirtualDevice: Send {
    fn open(&mut self, ctx: &mut DeviceOpenContext) -> Result<(), DeviceError>;
    fn read(&mut self, ctx: &mut DeviceIoContext, len: usize) -> Result<Vec<u8>, DeviceError>;
    fn write(&mut self, ctx: &mut DeviceIoContext, data: &[u8]) -> Result<usize, DeviceError>;
    fn ioctl(&mut self, ctx: &mut DeviceIoctlContext, request: u64, argp: u64) -> Result<i64, DeviceError>;
    fn mmap(&mut self, ctx: &mut DeviceMmapContext, req: &DeviceMmapRequest) -> Result<Option<DeviceMappedRegion>, DeviceError>;
    fn fstat(&self, ctx: &DeviceStatContext) -> Result<SyntheticStat, DeviceError>;
    fn close(&mut self, ctx: &mut DeviceCloseContext) -> Result<(), DeviceError>;
}
```

同时建议增加：

```rust
pub type DeviceFactory = Arc<dyn Fn() -> Box<dyn VirtualDevice> + Send + Sync>;
```

`FileDescriptorTable` 和 `FileDescriptorEntry` 建议放在 `OS` crate，而不是 `runtime/driver` crate。

原因是：

- 它们服务于所有 fd 来源，而不只服务设备
- regular file、device、socket、pipe、eventfd 都需要共享这层语义
- 这能避免把 OS 级句柄表错误地下沉到 driver 层

### 6. VFS 与 driver 的边界

VFS 层不应该知道设备行为细节。

它只需要把路径解析为：

```rust
pub enum VfsNode {
    HostFile(...),
    Bytes(...),
    Device(DeviceMountId),
}
```

推荐另行保留显式挂载来源：

```rust
pub enum MountSource {
    File(VirtFileSource),
    Device { mount_id: DeviceMountId },
}
```

其中：

```rust
pub enum VirtFileSource {
    HostPath(PathBuf),
    Bytes(Vec<u8>),
    Dynamic(FileProviderId),
}
```

然后：

- `openat` 看到 `Device(...)` -> 向 `DeviceRegistry` 请求 instance
- 拿到 device handle -> 插入 `FileDescriptorTable`
- `read/write/ioctl/mmap/close` 一律按 fd kind 分发

这样新增设备时：

- 不需要改 syscall matcher
- 不需要在 `openat` 之外反复判断路径

普通文件路径则走：

- `VirtFileSource::HostPath` -> 打开宿主文件并生成 file handle
- `VirtFileSource::Bytes` -> 打开内存文件视图并生成 file handle
- `VirtFileSource::Dynamic` -> 打开动态 regular file provider 并生成 file handle

这条普通文件主线除了“来源统一”，还必须保证“结果统一”：

- `VirtFileSource::HostPath` 的 `read/pread` 先从宿主文件读出字节，再由 runtime 执行目标侧回写
- `VirtFileSource::Bytes` 的 `read/pread` 先从内存源切片，再由 runtime 执行目标侧回写
- `VirtFileSource::Dynamic` 的 `read/pread` 先由 provider 生成字节，再由 runtime 执行目标侧回写

也就是说，provider/factory/file source 本身都不直接拥有“syscall 已成功”的最终解释权。

只有当 runtime 确认：

- 目标缓冲区可写
- 实际写入长度与返回长度一致
- 对应 telemetry/event 已按统一协议输出

这次 `read/pread` 才算成功。

### 6.1 关于 `vfs root`

当前阶段不建议强制引入单一 `vfs root` 或完整 rootfs 概念。

原因：

- bootstrap / early runtime 更需要的是“显式挂载表”
- `map_file("/virtual/path", VirtFile.host("host/path"))`、`map_file(..., VirtFile.bytes(...))`、`map_device(...)` 已足够覆盖多数 smoke 与 case 需求
- 一旦引入 rootfs，就会同时带来路径归一化、overlay、mount precedence、workdir、symlink 等额外复杂度

建议策略：

- phase 1：只做显式挂载表
- phase 2：如果 `/system`、`/vendor`、`/proc`、APK 展开目录等场景明显增多，再补 root mount / rootfs 概念

### 8. builtin 注册方式

推荐 runtime 在启动时预注册 builtin factory：

```rust
registry.map_device("/dev/urandom", builtin::urandom_factory())?;
registry.map_device("/dev/random", builtin::urandom_factory())?;
registry.map_device("/dev/null", builtin::null_factory())?;
registry.map_device("/dev/zero", builtin::zero_factory())?;
```

### 9. `mmap` 的处理原则

这是 file IO/driver 设计里最容易做坏的点。

建议区分两类：

#### 设备不支持 `mmap`

- 返回 `Ok(None)` 或显式 `ENODEV`
- 由上层 syscall 路径决定目标侧返回值

#### 设备支持 `mmap`

- 设备返回一个 `DeviceMappedRegion` 或等价描述
- 实际目标侧映射仍由 runtime/backend 完成
- 设备不应自己直接拿 backend 句柄偷偷映射

也就是说：

- device 决定“想映射什么”
- runtime 决定“怎么映射到目标侧”

这能保持 driver 层不反向依赖 backend。

普通文件节点也应遵守相同原则：

- `VirtFile` / file provider 只描述文件内容与偏移语义
- runtime/VFS/backend 负责把这段内容映射成目标侧可访问区域
- file-backed `mmap` 的验收必须回到目标侧读写层验证，而不是只检查返回地址

### 10. telemetry 要求

driver 路径必须统一出事件，不允许只在 Python 里 print。

建议至少输出：

- `device_open`
- `device_read`
- `device_write`
- `device_ioctl`
- `device_mmap`
- `device_close`
- `device_error`

错误信息至少包含：

- 虚拟路径
- fd
- request code（若是 ioctl）
- device class 名

## 推荐实施顺序

### Phase 1

- 建 `runtime/driver`
- 把当前 `/dev/urandom`、`/dev/null`、`/dev/zero` 从硬编码 VFS 中迁出去
- 打通 fd -> device handle 分发

### Phase 2

- follow-up change：Python `VirtualDevice`
- follow-up change：decorator 元数据模型
- follow-up change：Rust runtime -> Python binding / FFI 注册面
- follow-up change：Python case 注册主线

### Phase 3

- 增加 `/dev/ashmem`
- 增加一个 `ioctl` 型 fake device regression case
- 增加 `mmap` 型 fake device regression case

## 结论

用户提的“类行为 + 路径挂载”方案是合理的，而且比当前硬编码 `/dev/urandom` 分支更对路。

但最佳实现不是“decorator 直接替代 registry”，而是：

- decorator = 元数据声明
- `VirtFile` = 普通文件节点统一抽象
- registry = 真正挂载点
- factory = 设备定义到实例的桥
- fd handle = 行为分发主体

这套结构同时满足：

- qiling 风格的人机接口
- `agent-output` 里要求的高内聚低耦合
- 后续 builtin + custom device 共存
- 不改 syscall 核心即可新增设备
- 普通文件、内存文件、设备节点三种路径并列存在
