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
        └── AndroidVm
              ├── class registry
              ├── object store
              ├── method registry
              ├── field registry
              ├── ref table
              ├── exception state
              ├── apk context
              └── framework registry
```

推荐结构：

```rust
pub struct AndroidVm {
    pub classes: ClassRegistry,
    pub objects: ObjectStore,
    pub methods: MethodRegistry,
    pub fields: FieldRegistry,
    pub refs: RefTable,
    pub exceptions: ExceptionState,
    pub apk: Option<ApkContext>,
}
```

## Key decisions

### 1. VM 不是 bytecode engine

当前阶段明确不做 Dalvik/ART 字节码解释。

VM 仅负责 JNI-facing Java world。

### 2. 内部 authority 使用 typed id

不以 hash 或签名字串作为内部唯一权威。

推荐：

- `ClassId`
- `ObjectId`
- `MethodId`
- `FieldId`

### 3. object storage 分层

建议区分：

- string
- primitive wrapper
- primitive array
- object array
- framework stub instance
- generic host-side value

不要全部塞进一个 `Box<dyn Any>` 然后运行时乱猜。

### 4. local/global/weak refs 独立建模

`RefTable` 不只是 object map 的别名。

它必须显式表达：

- ref kind
- frame ownership
- lifetime cleanup

### 5. APK context 一等存在

`packageName`、`versionName`、`versionCode`、`manifest`、`signatures`、`assets` 必须收敛进 `ApkContext`。

framework stub 通过 `ApkContext` 取值，不允许到处散拿。

## Implementation order

1. `JClass` / `JObject` / `JMethod` / `JField`
2. `RefTable`
3. `ExceptionState`
4. `ApkContext`
5. unit/integration tests
