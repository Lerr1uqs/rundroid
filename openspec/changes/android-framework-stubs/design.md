## Context

unidbg 的 `AbstractJni` 之所以难维护，是因为：

- 类、字段、方法行为都堆进签名 switch
- 没有 class-level ownership
- framework 行为和 target-specific patch 混在一起

Rust 版必须把 framework stub 设计成注册式类模块。

## Architecture

推荐：

```rust
pub struct FrameworkRegistry {
    classes: HashMap<String, FrameworkClassSpec>,
    services: ServiceRegistry,
}

pub struct FrameworkClassSpec {
    pub class_name: String,
    pub constructors: Vec<FrameworkConstructorSpec>,
    pub static_fields: Vec<FrameworkFieldSpec>,
    pub instance_fields: Vec<FrameworkFieldSpec>,
    pub static_methods: Vec<FrameworkMethodSpec>,
    pub instance_methods: Vec<FrameworkMethodSpec>,
}
```

这层需要和现有 `JClassDef` / `JniRegistry` 收敛，而不是另起一套平行 class 系统。

推荐原则：

- Rust builtin framework class 是 Rust 侧注册入口
- Python javashim 是 Python 侧注册入口
- 两类入口都不是最终状态归属
- 两者最终都写入 Rust VM 持有的统一 class/member authority
- 两者统一先进入 `Emulator` 持有的 `AndroidRuntime`
- framework registry 可以是 builtin source catalog，但不是脱离 VM 的第二份最终状态

## Initial class set

优先覆盖：

- `android/app/Application`
- `android/content/Context`
- `android/content/ContextWrapper`
- `android/content/pm/PackageManager`
- `android/content/pm/PackageInfo`
- `android/content/pm/Signature`
- `android/content/pm/ApplicationInfo`
- `android/content/res/AssetManager`
- `android/os/Bundle`
- `android/os/IBinder`
- `android/os/IServiceManager`
- `java/lang/String`
- `java/lang/Class`
- `java/lang/Integer`
- `java/lang/Long`
- `java/lang/Boolean`
- `java/util/ArrayList`
- `java/util/List`
- `java/util/Map`
- `java/util/Set`
- `java/util/Iterator`
- `java/util/Enumeration`

## Rules

1. framework behavior 通过 class spec 注册
2. `getSystemService` 走 service registry
3. package/signature/asset 行为只从 `ApkContext` 读取
4. 不允许再新增 giant signature switch 作为正式主线
5. Rust builtin 与 Python override 必须进入同一套 class/member 数据模型
