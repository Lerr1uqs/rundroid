## Why

当前仓库对 `MethodId` 的语义存在冲突：`types.rs` 仍把它描述成 class-local member id，而 `registry.rs` / `class.rs` 已经为了修复继承解析冲突，开始在注册阶段把它重排成全局递增 id。这个状态会让 `GetMethodID`、`Call*Method`、`RegisterNatives`、文档和测试对同一个 `jmethodID` 各自假设不同语义，后续再接 Python FFI 与差分验证时会继续放大歧义。

参考 unidbg，guest 可见的 `jmethodID` 更接近“按 Java 签名字符串生成的稳定 token”，而不是内部 authority 本身。现在需要正式区分“Rust 内部 canonical member identity”和“guest ABI 可见 method id 生成策略”，把 method id 变成可插拔策略，并给出默认的 unidbg 兼容实现。

## What Changes

- 引入 `JavaMethodIdGenerator` 能力边界，用于根据 canonical method signature 生成 guest 可见 `jmethodID`
- 明确区分两层身份：
  - Rust 内部 authority 继续使用 class-centric registry + canonical `MethodSig`
  - guest ABI 可见 `jmethodID` 仅作为稳定 lookup token，不再等同于内部 member authority
- 默认提供 unidbg 兼容策略：对 `"java/lang/String-><init>([B)V"` 这类 canonical 签名执行 Java `String.hashCode()` 语义的 32 位哈希，再按 ABI 规则扩展为 `jmethodID`
- 规定 hash 冲突必须显式失败，不允许静默覆盖或自动退化到别的策略
- 统一 `GetMethodID` / `GetStaticMethodID` / `Call*Method` / `RegisterNatives` / `NewObject` 对 method id 的解析路径
- 补充针对默认 hash 策略、自定义策略、继承解析、native 注册绑定和冲突失败的测试

## Capabilities

### New Capabilities

- `jni-method-id-strategy`: 定义 guest 可见 `jmethodID` 的可插拔生成与解析语义

### Modified Capabilities

- `android-vm-model`: 澄清 method member identity 与 guest method token 的分层语义
- `jni-shim`: 规定 `GetMethodID` / `Call*Method` 使用可配置 method id 策略，并默认兼容 unidbg hash 行为
- `native-jni-lifecycle`: 规定 `RegisterNatives` 绑定的 key 与 method id 策略一致
- `testing-harness`: 增加 method id 策略与冲突回归覆盖

## Impact

- 影响 Rust 侧 `emulator/jni` 的 `types.rs`、`class.rs`、`registry.rs`、`jnienv.rs`、`native_registry.rs`
- 可能影响 `RuntimeConfig` 或等价装配入口，以便注入 method id 生成策略
- 会修正文档与测试中对 `MethodId` “class-local / global unique / hash token” 的混合表述
- 为后续 Python FFI 暴露自定义 method id 策略保留稳定扩展点，但本 change 不要求先把 Python 配置面做完
