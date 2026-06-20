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

不允许使用 get_xxx的方式 直接使用 xxx获取field即可

不要写大量的兜底策略 let-it-failed 方便调试 对于没覆盖没要求到的case直接丢出异常即可

尽量不要这么写： `fn build_exports(image: &ParsedElf, load_bias: u64) -> ExportTable` 不是很面向对象
最好是 `ExportTable::build(...)` 或者 `ExportTableBuilder(...).set(...).add(...).build(...)`

CPU ARM 这种首字母缩写的 就尽量全部大写 不要出现 `Cpu` 这种形式

所有 ut/harness api都必须注释标注清楚 

# 项目管理
rust 提供 ffi给到python 如果需要运行python 务必使用uv 管理 不要用全局python

# 项目目的

rust提供运行层核心框架 能够打包为python ffi 让python 通过给定的接口去写 hook/breakpoint/tracing/补环境能力 实现执行层和脚本层的解耦 