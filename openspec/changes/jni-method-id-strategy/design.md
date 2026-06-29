## Context

当前 `rundroid` 已经采用 class-centric registry 作为 JNI member 的内部 authority，这是正确方向；但 `MethodId` 同时承担了两种职责：

1. Rust 内部成员身份
2. guest JNI ABI 里的 `jmethodID`

这两层职责在实现里已经冲突：

- `types.rs` 仍把 `MethodId` 解释为 class-local typed id
- `class.rs` 在 `add_method` 阶段先分配 class-local 顺序号
- `registry.rs` 又在 merge/register 时把 method id 统一重排成全局递增值，试图修补继承链按裸 id 查找的歧义
- `jnienv.rs` 的 `dispatch_by_method_id` / `RegisterNatives` / `NewObject` 都把这个值当作 guest 可见 token 使用

而 unidbg 的做法更明确：`GetMethodID` 返回的是对 `"class->name(desc)"` 求 hash 后的稳定 token；内部真正的 method authority 仍然是 `DvmMethod` 对象和 class member map。也就是说，guest 可见 token 与内部 authority 并不是一回事。

本 change 的目标不是回到 unidbg 的“只靠 hash 做一切 authority”，而是保留 `rundroid` 已有的 canonical member model，同时把 guest 可见 method id 生成方式抽象出来。

## Goals / Non-Goals

**Goals:**

- 明确区分内部 canonical member identity 与 guest 可见 `jmethodID`
- 为 guest 可见 `jmethodID` 建立可插拔的 `JavaMethodIdGenerator` 边界
- 默认兼容 unidbg：canonical method signature 走 Java `String.hashCode()` 语义
- 统一 `GetMethodID`、`Call*Method`、`NewObject`、`RegisterNatives` 的 method id 主线
- 对 method id 冲突采用 fail-fast，而不是静默覆盖

**Non-Goals:**

- 不把 `MethodSig` / `JClassDef` / class-centric registry 改回全局 method registry authority
- 不把 field id 一并重构为同样策略；field 可后续独立 change 处理
- 不要求本次就完成 Python 侧 method id 生成器配置接口
- 不尝试兼容 unidbg 的所有可选 hash 算法；本次只要求默认策略与扩展点

## Decisions

### 1. 引入两层 identity：内部 member key 与 guest method token 分离

决策：

- `MethodSig` 继续是 class member 的 canonical key
- `JClassDef.methods/static_methods` 继续持有实际 member 定义
- guest ABI 层暴露的 `jmethodID` 改为“method token”，由生成器产出

原因：

- class-centric authority 是当前项目明确的架构方向，不能为了兼容 unidbg 的 token 形式而退回“hash 即 authority”
- `GetMethodID` 返回给 guest 的本来就只是后续 JNI 调用的 lookup token，不需要承担内部 authority 职责

备选方案：

- 继续把 `MethodId` 既当 typed id 又当 guest token：已被实践证明会导致语义漂移
- 完全改成 unidbg 风格、内部也只用 hash：会削弱 typed model，且碰撞处理更脆弱

### 2. 定义 `JavaMethodIdGenerator` trait，默认实现为 unidbg 兼容 hash

决策：

- 新增 `JavaMethodIdGenerator` trait，输入为 canonical method signature 字符串（`class->name(desc)`），输出为 guest method token
- 默认实现 `JavaStringHashMethodIdGenerator` 采用 Java `String.hashCode()` 的 32 位有符号哈希语义
- ARM64 下 guest `jmethodID` 统一按零扩展到 `u64` 暴露

原因：

- 这是与 unidbg 行为最接近的默认值，便于差分、日志比对和用户迁移
- trait 边界允许未来注入 FNV / Murmur / xxHash 或严格递增策略，而不污染 JNI 主线

备选方案：

- 只做单一内置 hash，不留 trait：后续会把实验空间锁死
- 默认用全局递增：与 unidbg 观察值不一致，不利于对照与复现

### 3. 冲突检测必须是注册期 fail-fast

决策：

- generator 产出的 token 若在同一 registry 中对应多个不同 method signature，注册时立即失败
- `GetMethodID` / `RegisterNatives` / `Call*Method` 不允许遇到多义 token

原因：

- 当前项目明确要求 let-it-failed，便于调试
- hash token 一旦多义，guest 侧后续所有调用都会变成未定义行为

备选方案：

- 运行时按 class 链二次 disambiguation：复杂且隐蔽，调试成本高
- 冲突时自动换备用 token：会破坏稳定性，也偏离“默认兼容 unidbg”的预期

### 4. `RegisterNatives` 的绑定 key 改为 guest method token 对应的 canonical method

决策：

- `RegisterNatives` 仍先按 `name + descriptor` 命中 canonical member
- 命中后注册到统一的 native binding 表
- native binding 表的稳定 lookup key 与 guest method token 一致，而不是旧的递增 `MethodId`

原因：

- guest 后续传回来的就是 `jmethodID` token，native binding 与 dispatch 必须对齐同一 token 语义

备选方案：

- native registry 单独继续使用内部 typed id：会造成 guest `jmethodID` 与 native binding key 再次分裂

### 5. 配置面先放在 Rust 装配层，Python 延后

决策：

- 首轮只要求 Rust 装配层可选择 generator；可放入 `RuntimeConfig` 或 emulator 组装入口
- Python FFI 如果还没有稳定 builder，不在本次要求中强推公开配置 API

原因：

- 先把 core 语义和测试闭环打通，比先暴露不稳定 binding API 更重要

备选方案：

- 同步开放 Python 配置接口：会扩大变更面，且当前 Python surface 仍在演进

## Risks / Trade-offs

- [默认 hash 与历史测试值不一致] → 通过 spec 明确这是行为变更，并补充差分/迁移测试
- [32 位 hash 存在天然碰撞风险] → 注册期 fail-fast；后续需要更强策略时可通过 trait 注入
- [类型命名可能继续混淆] → 在实现中把“内部 typed member id”和“guest method token”用不同类型命名，避免继续复用同一 `MethodId`
- [配置入口放错层会增加耦合] → 优先放在装配/config 层，不把策略选择散落到 `JniEnvSurface`

## Migration Plan

- 先在 spec 中把语义改清楚
- 实现时新增 generator 与 method token 类型，并让 registry 同时保存 canonical member 与 guest token 映射
- 用新映射替换 `resolve_method_by_id`、`lookup_native` 等按旧 `MethodId` 工作的主线
- 更新单元测试与 harness fixture，验证默认 token 值与 unidbg 一致
- 若已有调用方依赖“递增 method id”，在变更说明中明确这是非兼容行为调整

## Open Questions

- `FieldId` 是否后续也要对齐成同样的可插拔 token 策略，目前本 change 不覆盖
- 最终公开命名是否使用 `JavaMethodIdGenerator` / `MethodIdTokenGenerator` / `JMethodIdGenerator`，实现时需要统一术语
