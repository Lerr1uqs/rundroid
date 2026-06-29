## 1. Spec And Model Cleanup

- [ ] 1.1 盘点 `MethodId` / `jmethodID` 在 `types.rs`、`class.rs`、`registry.rs`、`jnienv.rs`、`native_registry.rs` 中的当前语义冲突，并整理为实现注释与迁移点
- [ ] 1.2 引入区分“内部 canonical member identity”和“guest method token”的类型与命名，消除现有混用

## 2. Method Id Strategy

- [ ] 2.1 设计并实现 `JavaMethodIdGenerator` trait 及默认 unidbg 兼容 `Java String.hashCode()` 生成器
- [ ] 2.2 在 registry 建立 canonical method 与 guest method token 的稳定双向映射
- [ ] 2.3 为 method-id 冲突实现 fail-fast 检测与清晰错误信息

## 3. JNI Dispatch Integration

- [ ] 3.1 改造 `GetMethodID` / `GetStaticMethodID` 走可配置 generator，而不是依赖旧递增 `MethodId`
- [ ] 3.2 改造 `Call*Method` / `CallStatic*Method` / `NewObject` 按 guest method token 回查 canonical method
- [ ] 3.3 改造 `RegisterNatives` / native lookup 与新的 method-id strategy 对齐

## 4. Verification

- [ ] 4.1 增加单元测试：默认 hash 值、自定义 generator、继承链调用、RegisterNatives 绑定、冲突失败
- [ ] 4.2 增加 harness 或 integration 回归：`GetMethodID` 返回值与 unidbg 兼容，且后续 `Call*Method` 可执行
- [ ] 4.3 运行 `openspec validate --type change jni-method-id-strategy --strict`
