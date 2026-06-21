## Context

unidbg 的 `BaseVM`、`DvmClass`、`DvmObject`、`DvmMethod`、`DvmField` 其实已经说明了一件事：

- Android JNI 支持的核心不是 bytecode VM
- 而是一个“足够像 Java 世界”的状态模型

Rust 重写时应先把这层固定下来，再往上接 ABI、framework、shim。

## Architecture

建议模型：

```text
Emulator
  └── AndroidRuntime
        └── AndroidVM
              ├── class registry
              ├── object store
              ├── ref table
              ├── exception state
              ├── apk context
              └── framework registry
```

推荐结构：

```rust
pub struct AndroidVM {
    pub classes: ClassRegistry,
    pub objects: ObjectStore,
    pub refs: RefTable,
    pub exceptions: ExceptionState,
    pub apk: Option<ApkContext>,
}
```

这里必须明确：

- `AndroidVM` 的 authority 是 `JavaClassDef` / `JClassDef`
- method / field 不再作为和 class 并列的一等顶层 registry
- method / field 是 class 的成员定义，必要时只维护索引视图，不单独成为权威状态

也就是说，当前实现路线不是：

- `class registry`
- `method registry`
- `field registry`

而是：

- `class registry`
- 每个 class 内部持有 instance/static method table
- 每个 class 内部持有 instance/static field table
- 需要按签名快速查找时，再建立从签名到 class member 的索引

更接近当前 `emulator/jni/src/class.rs` + `registry.rs` 的方向：

- `JniRegistry.classes: HashMap<String, JClassDef>`
- `JClassDef.methods`
- `JClassDef.static_methods`
- `JClassDef.fields`
- `JClassDef.static_fields`

因此 `android-vm-state-model` 应与 `python-javashim-overrides` 的注册机制对齐：

- Python `@java_class` 是 class-centric declaration
- `register(emulator, [Cls])` 最终生成一个完整 class definition
- Rust 接收的是 class definition，而不是零散 method/field 的全局散注册

同时还要补上 Rust builtin 注册入口：

- Rust builtin class 声明也属于注册层，不属于最终 authority
- Rust builtin 和 Python shim 都必须先被规整为统一 class definition
- 最终统一注册到 `Emulator` 持有的 `AndroidRuntime`
- `AndroidRuntime` 内部的 `AndroidVM` / `JniRegistry` 才是最终状态归属

推荐链路：

```text
Python decorators/register(...)
Rust builtin macros/builders
        ↓
normalize to unified JClassDef / class-member model
        ↓
Emulator-owned AndroidRuntime
        ↓
AndroidVM / JniRegistry / object store / refs / exceptions / apk context
```

这也意味着当前类似 `PyEmulator` 上的以下状态不应继续作为 VM 主状态存在：

- `class_types`
- `method_names`
- `java_instances`

它们最多只能是 binding 层的临时适配缓存。

正确方向应为：

- class/member/object identity 由 `AndroidRuntime` / `AndroidVM` 持有
- Python binding 通过 runtime 的 class definition、member identity、`ObjectId` 做适配
- 不允许反过来由 `PyEmulator` 的 Python map 决定 class/member/object 的最终语义

## Key decisions

### 1. VM 不是 bytecode engine

当前阶段明确不做 Dalvik/ART 字节码解释。

VM 仅负责 JNI-facing Java world。

### 2. 内部 authority 使用 typed id，但 class 是聚合根

不以 hash 或签名字串作为内部唯一权威。

推荐：

- `ClassId`
- `ObjectId`
- `MethodId`
- `FieldId`

但这些 typed id 的组织关系要改成：

- `ClassId` 对应一个完整 class definition
- `MethodId` / `FieldId` 归属于某个 `ClassId`
- 不建议先独立建全局 `MethodRegistry` / `FieldRegistry`，再反向挂回 class
- 更合理的是 class 内部成员表为 authority，全局只保留可选索引

### 3. object storage 分层

建议区分：

- string
- primitive wrapper
- primitive array
- object array
- framework stub instance
- generic host-side value

不要全部塞进一个 `Box<dyn Any>` 然后运行时乱猜。

如果某些 object 最终由 Python shim 提供 backing instance，也应当：

- 先在 `AndroidVM` 中取得正式 `ObjectId`
- 再由 binding 层按 `ObjectId` 关联到 Python backing object
- 不应由 `PyEmulator.java_instances` 一类 guest-handle map 单独充当最终 object authority

### 4. local/global/weak refs 独立建模

`RefTable` 不只是 object map 的别名。

它必须显式表达：

- ref kind
- frame ownership
- lifetime cleanup

### 5. APK context 一等存在

`packageName`、`versionName`、`versionCode`、`manifest`、`signatures`、`assets` 必须收敛进 `ApkContext`。

framework stub 通过 `ApkContext` 取值，不允许到处散拿。

### 6. 和 unidbg 的对应关系

unidbg 确实有：

- `resolveClass`
- `classMap`
- `DvmClassFactory`

所以它本质上也是“先 resolve class，再从 class 进入 method/field/object 语义”。

只是 unidbg 的对象体系里还混有：

- `DvmMethod`
- `DvmField`
- 可选 `ProxyClassFactory`

而我们当前目标已经明确不走 host JVM reflection / `ProxyJni` 路线。

因此在 `rundroid` 里更适合把模型收口为：

- Rust VM 维护 class-centric state
- Python decorator 负责声明 class member metadata
- Rust builtin 声明负责提供 framework/builtin class metadata
- register 阶段把 metadata 压成 `JClassDef`
- 最终统一进入 `AndroidRuntime`
- JNI / framework / native lifecycle 都围绕 class definition 工作

## Implementation order

1. `JClassDef` / `JObject`
2. `RefTable`
3. `ExceptionState`
4. `ApkContext`
5. unit/integration tests
