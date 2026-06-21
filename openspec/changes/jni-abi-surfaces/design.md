## Context

JNI ABI surfaces 是 Android VM 能否“像真的”工作的核心。

这层必须保持 guest 可见：

- `_JavaVM`
- `_JNIEnv`
- invoke table
- function table
- svc handler

## Architecture

推荐结构：

```rust
pub struct JavaVmAbi {
    pub java_vm_ptr: GuestPtr,
    pub invoke_table_ptr: GuestPtr,
    pub slots: Vec<JavaVmSlot>,
}

pub struct JniEnvAbi {
    pub jni_env_ptr: GuestPtr,
    pub function_table_ptr: GuestPtr,
    pub slots: Vec<JniSlot>,
}
```

每个 slot 至少表达：

- name
- offset
- guest svc address
- bound handler id

## Key decisions

### 1. ABI 表必须真实落在 guest memory

不要退化成纯 host-side interface。

### 2. 入口覆盖按 phase 推进

第一阶段覆盖：

- `GetVersion`
- `FindClass`
- `GetMethodID`
- `GetStaticMethodID`
- `GetFieldID`
- `GetStaticFieldID`
- `NewObject`
- `Call*Method`
- `GetEnv`
- `AttachCurrentThread`
- `DetachCurrentThread`

### 3. ABI handler 只做桥接

slot handler 负责：

- 解 guest 参数
- 查 registry
- 调 dispatch

不要在 handler 里硬塞 framework 业务逻辑。
