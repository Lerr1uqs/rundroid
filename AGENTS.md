# unidbg改造计划

- rust 重写底层core层 unicorn-rs + android 执行层rs + unix syscall 层也rs
- python实现 stub 也就是加入断点 增加mock类 进行overload

实现执行层和core层 rust运行速度 但是脚本层有python的编写效率 舍弃java 笨重的虚拟机和源码依赖

```
  整体依赖关系（一句话抽象）

  [用户测试代码]
        │ 调用
        ▼
  [0 Emulator 装配] ── 依赖 unidbg-api 的 CPU 仿真内核
        │
        ├──► [1 ELF Loader]   加载 .so 到内存
        ├──► [5 libdl/linker] 处理 dlopen/dlsym
        ├──► [2 Syscall]      拦截 svc 0x0
        │        └─► [3 File/IO]   open/read/socket…
        │        └─► [signal / thread / struct]
        └──► [4 Dalvik VM]    JNI 桥，最大子系统
                  ├─ [对象模型 / 数组 / 包装类]
                  ├─ [调用约定 VaList/VarArg]
                  ├─ [api: Bitmap/Bundle/Signature…]
                  └─ [ProxyJni: 反射代理到真实 Java]
        + [6 Hook: xHook / system_property / logcat]
        + [7 VirtualModule: jnigraphics/medandk stub]

  最核心、最庞大的是 第 4 层（DVM/JNI 模拟），其次是 第 2/3 层（syscall + 文件
  IO），其余都是支撑和扩展。如果你想深入某一块，告诉我即可。
```
目前只做 arm + android版本 


# 你的职责
根据 target.md 编写代码和实现 有别的agent会来验收 你需要负责跑通（不允许写完就不管了）

unidbg源码路径： F:\reverse-workspace\unidbg 可供参考
unidbg resources里面有一些可以用于验证的prebuild库

也可以模拟一些qiling之类的实现 通过搜索


# 代码风格
注意要有中文注释 函数注释+函数体内部复杂算法注释 如果遇到特殊case需要说明什么情况下有这个case 高内聚低耦合 面向对象编程

不允许使用 get_xxx的方式 直接使用 xxx获取field即可 (除非是Java 签名专属要求 那没办法 此外尽量别用get_xxx)

不要写大量的兜底策略 let-it-failed 方便调试 对于没覆盖没要求到的case直接丢出异常即可

尽量不要这么写： `fn build_exports(image: &ParsedElf, load_bias: u64) -> ExportTable` 不是很面向对象
最好是 `ExportTable::build(...)` 或者 `ExportTableBuilder(...).set(...).add(...).build(...)` 链式风格

CPU ARM 这种首字母缩写的**类名** 就尽量全部大写 不要出现 `Cpu` 这种形式 使用 `CPU` `JNI` `ARM` `VMP` 之类的大写缩写更舒服

所有 ut/harness api都必须注释标注清楚 

编写python的时候不要使用 Any作为typing 尽量都要使用typing作为注释

# 项目管理
rust 提供 ffi给到python 如果需要运行python 务必使用uv 管理 不要用全局python

# 测试要求

单元测试 要求从点到面 有丰富的模块内测试复杂度深度和交叉度

# 项目目的

rust提供运行层核心框架 能够打包为python ffi 让python 通过给定的接口去写 hook/breakpoint/tracing/补环境能力 实现执行层和脚本层的解耦 

# 最终想实现的效果

其实就是rust写core 提供api和stub给到python 写python脚本去运行unidbg模拟+补齐环境
当然也可以添加rs代码作为链接时加入来保证速度（这个后面继续优化 现在先不做）

> 注意下面的API不一定完全需要完全对照 只是当做意思符合 后续随时会变 只参考语义不要参考命名即可
```python
@java_class("android/content/pm/Signature")
class Signature(JavaObject): # 注册后 native层调用会走到这里 

	def __init__(self):
		# 正常进行初始化 复制成员函数 这个Signature会被实例化的
		self._msig = bytes([])

	@java_method("Signature([B)V") # 构造函数
	def signature_init(self, sig) -> JVoid:
		self._msig = sig

	@java_method("hashCode()I")
    def hash_code(self) -> JInt:
    	pass

    @java_field(name="mSignature", sig="[B") # 应该是这么描述field？ or @java_field("mSignature[B")
    def member_signature(self) -> JArray[JByte]:
    	return self._msig


emulator = (
	AndroidEmulatorBuilder()
        .arch(ARM64)
        .backend_factory(Unicorn2Factory(true))
        .library_resolver(23) # 自带的系统库
        .avm_verbose(true)
        .build()
)

emulator.avm.register_java_classes([Signature])

SharedLibrary so = emulator.load_library("path/to/file")
emulator.jni_onload(so)

class Breakpoint(BreakpointInterface):

	def on_hit(emulator: Emulator, addr: int):

		x0 = emulator.backend().reg_read(Arm64Const.UC_ARM64_REG_X0) # or .as_pointer()
		p0 = Pointer.pointer(emulator, x0)

		Inspector.inspect(p1.read(0x20), tag="p0");

emulator.breakpoint.add(so.base + 0x1234, Breakpoint())

jclass = emulator.avm.find_class("android/content/pm/Signature")
sig = emulator.avm.new_object(jclass, bytes([0x11, 0x22, 0x33])) # call the constructor: "android/content/pm/Signature->signature([B)V"
```

# 项目文档

project_design/ 项目设计目录 如果用户没有要求严禁更改 如果有事实性冲突 请及时提出
lessons/ 犯错清单 历史上的错误 经验教训