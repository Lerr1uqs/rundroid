## Context

`rundroid` 的目标不是做一个新的 Java 虚拟机，而是做一个 Rust-first 的 Android Native runtime，并把脚本扩展层交给 Python。

这意味着 JNI 层的职责必须被严格切开：

1. Rust 持有目标侧 JNI 状态、对象表、引用表和分派权威
2. Python 提供更适合写 case/stub/mock 的 class-based 声明方式
3. `JNIEnv` / `JavaVM` 是 guest 可见 ABI surface，不是 Python 业务对象
4. 完整 Java VM、classloader、framework 行为留到后续 change

这个 change 的目的不是“一次实现所有 JNI”，而是建立未来能持续扩展的稳定基座。

同时，对外 API 的命名与装配层也要开始收敛。

当前仓库里很多地方还把目录和用户主入口叫 `runtime` / `Runtime`，但目标形态应更接近 unidbg 的使用心智：

- 顶层目录改成 `emulator/*`
- 对外提供 `Emulator`
- JNI 作为 emulator 组装出的一个子系统，而不是直接让用户面向 `Runtime` 类型编程

## Scope Boundary

这一轮的 JNI 范围应固定为 foundation，而不是 full Java runtime。

必须覆盖：

- `emulator/jni` crate
- emulator 装配层对 `runtime/jni` 的接入点
- 顶层 `runtime/` -> `emulator/` 重命名
- canonical `JType` / `JValue`
- method / field descriptor parser
- shim registry
- method / field dispatch
- local/global/weak reference table
- 最小 `JNIEnv` / `JavaVM` surface
- `JNI_OnLoad` 所需的 attach / env 获取主线
- Python shim decorator 和显式注册桥

明确不覆盖：

- 完整 `FindClass` / `ClassLoader` 体系
- 完整 `java.lang.*`、`android.*` 对象模型
- 完整异常传播语义
- `ProxyJni` 风格的“反射到真实 Java”
- ART/Dalvik method table 细节
- 自动补齐所有签名的 `JNIEnv` 函数表实现

## Architecture

建议新增 crate：

```text
emulator/jni/
  src/
    lib.rs
    args.rs
    class.rs
    descriptor.rs
    dispatch.rs
    field.rs
    jnienv.rs
    javavm.rs
    object.rs
    refs.rs
    registry.rs
    types.rs
    verify.rs
```

同时扩展：

```text
emulator/core/
  src/
    emulator.rs
    runtime.rs

emulator/bindings/python/
  src/
    lib.rs
    javashim.rs

python/rundroid/
  emulator.py
  javashim/
    __init__.py
    base.py
    decorators.py
    registry.py
    types.py
```

整体依赖关系建议固定为：

```text
User script / case
        │
        ▼
Emulator facade
        │ 装配 backend / memory / loader / os / jni / telemetry
        ▼
Python shim decorators
        │ 只声明 metadata
        ▼
bindings/python registration bridge
        │ 解析 descriptor + 注解校验
        ▼
emulator/jni registry
        │
        ├── object / refs
        ├── method / field sig
        ├── dispatch
        └── JNIEnv / JavaVM surface
```

关键原则：

- 对外主入口是 emulator，不是 runtime
- Python 不直接持有 JNI registry 权威状态
- registry collision 必须 fail-fast
- 类型不匹配必须在注册时尽早失败
- 调用阶段只走 typed dispatch，不走中心字符串 switch

## Emulator Layer Boundary

这一层是本次需要补强的架构约束。

建议区分：

- `emulator/*`：顶层 crate 目录与内部子系统容器
- `Emulator`：对外装配 facade

也就是说：

- `emulator/jni` 不是给最终用户直接 new 出来使用的对象
- 用户面向 `Emulator` 配置 backend、装载模块、补 VFS、注册 JNI shim
- `JNIEnv` / `JavaVM`、driver、loader、linker 都是 emulator 持有的子系统

推荐 Rust 侧形态：

```rust
pub struct Emulator {
    // backend
    // memory
    // loader/linker
    // linux/android os
    // jni registry / refs / vm
    // telemetry
}
```

推荐 Python 侧形态：

```python
from rundroid import Emulator

emu = Emulator("arm64", "unicorn", seed=42)
```

这样更接近 `unidbg` 的外部心智模型，也能避免把 `runtime` 这个旧命名继续固化到目录和用户接口里。

## Core Type Model

JNI foundation 最重要的是先定义稳定的 canonical type model。

建议保持：

```rust
pub enum JType {
    Void,
    Boolean,
    Byte,
    Char,
    Short,
    Int,
    Long,
    Float,
    Double,
    Object(String),
    Array(Box<JType>),
}

pub enum JValue {
    Void,
    Boolean(bool),
    Byte(i8),
    Char(u16),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Object(ObjectId),
    Null,
}
```

这里要先把规则写硬：

- `Null` 只允许出现在 object / array 兼容位置
- primitive 返回值不允许以 `Null` 代替
- runtime 不做 silent widening / narrowing

`MethodSig` / `FieldSig` 必须是 registry 的 canonical key。

原因是：

- 仅靠 `class + name` 无法区分 overload
- descriptor string 如果一直以未解析文本流动，后续校验会越来越散

所以应在注册入口就完成解析，后续统一用 typed signature。

## Descriptor Parsing

descriptor parser 是整个 JNI foundation 的入口闸门。

建议规则：

- 内部统一使用 slash-separated class name
- 非法 descriptor 在注册阶段直接失败
- parser 输出 canonical `MethodSig` / `FieldSig`
- 后续 dispatch / verify 不重新解析原始 descriptor 字符串

这样 Python bridge 的注册流程就能稳定为：

1. 读取 decorator metadata
2. 解析 Java descriptor
3. 提取 Python 注解
4. 转成 `PythonCallableAnnotations`
5. 做 exact match verify
6. 注册到 emulator 持有的 Rust registry

## Registry and Dispatch

JNI 注册面要避免回到传统 `switch(signature)`。

建议 registry 最少区分三类对象：

- `JClassDef`
- `JMethodDef`
- `JFieldDef`

其中 method / field 的实现来源可以是：

- Rust native handler
- Python shim handler

也就是说，dispatch 的核心不是“按来源分两套 API”，而是“同一个 registry，不同的 implementation backend”。

推荐模型：

```rust
pub enum MethodImpl {
    RustNative(Arc<dyn RustMethodHandler>),
    PythonShim(ShimMethodId),
}

pub enum FieldAccess {
    RustNative(Arc<dyn RustFieldHandler>),
    PythonShim(ShimFieldId),
}
```

这样做的价值有两点：

1. 后续 builtin Java stub 和 Python 自定义 shim 能共享一套查找和分派链
2. review 时能直接检查 registry/dispatch 是否稳定，而不是被 FFI 细节淹没

## Object and Reference Model

JNI foundation 不能一上来就实现完整对象模型，但必须先把“对象身份”和“引用语义”稳定下来。

建议最小模型：

- `JavaObject` 只表达 object id 和 class name
- `RefKind` 区分 local / global / weak global
- `RefTable` 统一持有句柄生命周期

这里要特别避免一个常见错误：

- 把 Python 对象引用当成 guest 可见 JNI 引用本身

正确边界应是：

- guest 只看到 `jobject` / `jclass` 之类的 handle
- Rust `RefTable` 维护 handle -> `ObjectId`
- Python bridge 只在 dispatch 时被借用，不持有最终权威

当前阶段不强求完整 GC，但必须至少保证：

- local refs 可在 call frame 结束后统一清理
- global refs 不因局部返回而失效
- weak global 至少有显式 kind 区分，即便后续真正弱引用回收策略延后

## JNIEnv and JavaVM Boundary

这部分最容易范围失控，所以必须定清楚。

当前阶段 `JNIEnvSurface` / `JavaVMSurface` 只要求最小可运行主线：

- call method
- call static method
- get/set field
- attach current thread
- detach current thread

为什么只做到这里：

- crackme 最先撞到的是 `JNI_OnLoad`、少量 method/field 调用和返回值桥接
- 如果一上来要求完整 `JNIEnv` 函数表，工作量会远超当前 runtime 成熟度

这一轮应把“可扩展的 surface 形式”定下来，而不是把所有 entry 都一次补全。

后续如果扩展 `FindClass`、`NewStringUTF`、`GetByteArrayElements` 等能力，也应继续挂在 `emulator/jni` crate 下，不应把逻辑散进 Python 层。

## Python shim API

Python 侧要追求两个目标：

1. 编写体验接近 unidbg 里常见 Java stub 的脚本效率
2. 绝不牺牲注册阶段的严格性

推荐 decorator：

```python
@java_class("android/content/pm/Signature")
class Signature(JavaObject):
    @java_method("hashCode()I")
    def hashCode(self) -> JInt:
        return 0x12345678
```

核心规则：

- decorator 只挂 metadata，不做即时 runtime 注册
- import 模块不会污染全局 registry
- 真正生效必须显式 `register(emulator, Signature)` 或等价入口

注解规则默认 strict：

- 必须使用 Java-aware typing marker
- 不接受裸 `int`、`str` 作为稳定 ABI 注解
- descriptor 与 annotation exact match

这和当前项目整体风格一致：

- fail fast
- let-it-failed
- 不搞大量宽松兜底策略

## JNI_OnLoad Strategy

foundation 阶段必须明确 `JNI_OnLoad` 的位置，因为很多实际样本第一步就是它。

建议最小语义：

1. linker 输出稳定 init 顺序
2. emulator 能在模块装载后定位 `JNI_OnLoad`
3. 若模块导出该符号，则使用当前 `JavaVM` surface 调用它
4. `JNI_OnLoad` 内部若请求 `JNIEnv`，emulator 提供当前线程绑定的 env

当前阶段不要求：

- 完整 `JNIEnv` 函数表实现
- 真正 Android framework class 就绪

但必须要求：

- `JNI_OnLoad` 的入口调用链是明确的
- 失败 telemetry 能定位到模块、符号、线程和 descriptor 上下文

## Telemetry and Errors

JNI 路径不能变成黑盒。

建议至少统一输出：

- `jni.register_class`
- `jni.register_method`
- `jni.register_field`
- `jni.call`
- `jni.return`
- `jni.ref.new`
- `jni.ref.delete`
- `jni.error`

错误信息至少应带：

- class name
- method / field name
- descriptor
- 期望类型
- 实际类型

这样后面调 `Signature.hashCode()`、`Context.getPackageName()` 这类 shim 时，错误能快速归因，而不是只能看到 Python traceback。

## Testing Strategy

这一轮必须从第一天就有 harness case。

建议最少覆盖四类：

1. descriptor/annotation 匹配成功
2. descriptor/annotation 不匹配并在注册阶段失败
3. Rust-native 和 Python-shim 共享一套 registry dispatch
4. `JNI_OnLoad` 能通过最小 `JavaVM` / `JNIEnv` 主线跑通

这里要强调：

- 不能只做 unit test
- 至少要有一个 case 从 Python 注册 shim，再由 emulator 持有的 Rust registry dispatch

否则接口看起来正确，真正 FFI 桥接时仍然容易翻车。

## Recommended Implementation Order

### Phase 1

- 将顶层 `runtime/` 重构为 `emulator/`
- 新建 `emulator/jni` crate
- 在 `emulator/core` 中新增或重构 `emulator.rs`
- 落地 `types.rs`、`descriptor.rs`、`registry.rs`、`verify.rs`
- 增加 `JType` / `JValue` / `MethodSig` / `FieldSig`
- 先跑纯 Rust unit tests

### Phase 2

- 落地 `dispatch.rs`、`object.rs`、`refs.rs`
- 建最小 `JNIEnvSurface` / `JavaVMSurface`
- 补 Rust-native handler regression

### Phase 3

- 扩展 `emulator/bindings/python`
- 把 Python 对外入口从 `Runtime` 迁到 `Emulator`
- 新建 `python/rundroid/javashim`
- 打通 decorator metadata -> Rust registration bridge

### Phase 4

- 增加 `JNI_OnLoad` harness case
- 增加 telemetry
- 补 openspec 对应 acceptance case

## Conclusion

当前下一步确实应该进入 JNI，但不是“直接做完整 JNI”，而是先做 JNI / shim foundation。

最重要的结构决定应固定为：

- Rust registry 持有 JNI 权威
- 对外 API 由 `Emulator` 统一装配和暴露
- Python 负责声明式 shim
- descriptor 和注解在注册时严格校验
- method/field 分派统一走 typed registry
- `JNI_OnLoad` 进入 runtime 正式主线

这样后续再加 Android class stub、数组、字符串、包装类、更多 `JNIEnv` 函数时，才不会重新掉回中心化 switch-case。
