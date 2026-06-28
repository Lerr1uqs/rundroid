## REMOVED Requirements

### Requirement: Multiple registration surfaces converge through AndroidRuntime

## ADDED Requirements

### Requirement: Multiple registration surfaces converge through AndroidVM

runtime SHALL 把所有注册面收敛到 `Emulator` 直接持有的 `AndroidVM`（单一 authority）。
`AndroidRuntime` 包装层 SHALL 被移除——它仅是 `AndroidVM` 的纯转发空壳（零额外状态）。

#### Scenario: Builtin and Python-registered classes converge to AndroidVM

- **WHEN** runtime 注册 Rust builtin framework class 或 Python javashim class
- **THEN** 两者 SHALL 收敛到同一套 `Emulator` 直接持有的 `AndroidVM` / `JniRegistry`
- **AND** 它们 SHALL 共享相同的 class/member 数据结构
- **AND** 不 SHALL 分别维护两套彼此独立的 method/field authority

#### Scenario: Binding and JNI trampoline hook share one AndroidVM

- **WHEN** Python 绑定层初始化 JNI 执行（`init_jni`）
- **THEN** 绑定层 SHALL 以 `Arc<Mutex<AndroidVM>>` 持有 VM
- **AND** JNI trampoline hook SHALL 拿到同一 VM 的 `Arc::clone`
- **AND** 经绑定层注册的 class SHALL 对 guest JNI dispatch 可见（同一 registry）

#### Scenario: AndroidRuntime wrapper is removed

- **WHEN** Emulator 持有 VM 状态
- **THEN** SHALL 直接持有 `AndroidVM`（不经 `AndroidRuntime` 包装）
- **AND** 项目中 SHALL NOT 存在仅转发 `AndroidVM` 的 `AndroidRuntime` 类型

#### Scenario: Python binding caches are never the final VM state

- **WHEN** Python binding 为适配 shim 调用而维护 class/object/member 相关缓存
- **THEN** 这些缓存 SHALL NOT 成为最终 VM authority
- **AND** class/member/object identity SHALL 以 `AndroidVM` 状态为准
- **AND** 类似 `class_types`、`method_names`、`java_instances` 的结构若仍存在，SHALL 仅作为 binding-layer adapter cache

### Requirement: No VM re-entry during guest JNI dispatch

guest JNI dispatch（在 `emu_start` 期间）触发 Python override 时，该 override SHALL NOT 再次获取 VM 锁。这是单线程仿真的内在约束（同 unidbg）。

#### Scenario: Python JNI override does not re-enter the VM

- **WHEN** guest JNI dispatch 调到一个 Python `@java_method` override
- **THEN** 该 override SHALL NOT 调 `avm.new_object` / `emulator.call` 等再次入 VM / engine 的路径
- **AND** 绑定层文档 SHALL 明确标注该限制
- **AND** 测试 fixture SHALL 仅使用纯计算型 override（读字段、算返回值），不得依赖 guest dispatch 期间的 VM re-entry
- **AND** 纯计算型 override SHALL 正常工作
