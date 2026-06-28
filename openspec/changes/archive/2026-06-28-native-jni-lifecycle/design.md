## Context

unidbg 在 native JNI 生命周期上有两条关键路径：

1. `RegisterNatives`
2. `Java_*` 符号名 fallback

同时模块装载后还要支持 `JNI_OnLoad`。

这三件事必须统一成一个 lifecycle 模型，而不是散在 loader、JNI、module helper 里。

## Architecture

推荐：

```rust
pub struct NativeRegistry {
    by_method: HashMap<MethodId, GuestPtr>,
}

pub struct NativeJniLifecycle {
    pub natives: NativeRegistry,
}
```

主线：

1. `RegisterNatives` 读取 `JNINativeMethod[]`
2. 解析 method name + descriptor + fn ptr
3. 绑定到 `MethodId`
4. 未注册时，按 `Java_*` mangled name fallback 查找
5. 模块完成装载后，若存在 `JNI_OnLoad`，则通过 `JavaVM*` 调用

## Rules

- `RegisterNatives` 优先于 dynamic lookup
- `JNI_OnLoad` 返回版本必须校验
- telemetry 必须区分 registered / mangled fallback / onload call
