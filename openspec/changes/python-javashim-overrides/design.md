## Context

Python javashim 的目标不是取代 VM，也不是兜底所有 framework。

它的作用是：

- target-specific override
- 快速补 class / method / field 行为
- 在不改 Rust 的情况下写 crackme case

## Architecture

推荐优先级：

1. Python explicit override
2. Rust framework stub
3. fail-fast unsupported error

但这只是“查找优先级”，不是“两套状态模型”。

底层必须满足：

- Rust builtin 和 Python shim 共用同一套 class/member authority
- Python 不是 VM 权威，只是注册入口
- Rust 才是最终同步点
- 最终落点是 `Emulator` 持有的 `AndroidRuntime`
- `PyEmulator` 不应继续持有独立的 class/member/object 最终状态

推荐 Python 结构：

```text
python/rundroid/javashim/
  __init__.py
  base.py
  decorators.py
  registry.py
  types.py
  verify.py
```

关键规则：

- decorator 只挂 metadata
- import 不自动注册
- 必须显式 `register(emulator, ClassSpec)`
- descriptor 与 annotation strict match
- 返回值必须 runtime verify

这里再补一条边界：

- Python javashim 不是在 Rust 里分别注册零散 method/field
- 它的职责是把一个 Python class 收敛成一个完整的 class definition
- Rust VM 接收后应进入 class-centric registry

同步链路应为：

1. `@java_class` 写入 class metadata
2. `@java_method` / `@java_field` 写入 member metadata
3. `register(emulator, [Cls])` 收集成员，整理成 class-level metadata list
4. `emulator.register_java_class(cls)` 进入 PyO3 bridge
5. Rust bridge 构造 `JClassDef`
6. 统一注册到 `Emulator` 持有的 `AndroidRuntime`
7. `AndroidRuntime` 内部的 `AndroidVM` / `JniRegistry` 成为最终 authority

如果 Rust 侧已经存在 builtin class，则 Python 注册应表现为：

- override 指定 member
- 或补充尚未实现的 member
- 但无论哪种，都必须落在同一套 Rust class/member 结构上

对当前 Python binding 的约束也应明确：

- `class_types` 不应再作为 class authority，只能作为可选 instantiation adapter
- `method_names` 不应再作为 method authority，只能作为从 runtime member 到 Python callable 的辅助映射
- `java_instances` 不应再作为 object identity authority，只能作为按 `ObjectId` 或等价 runtime identity 关联的 backing store

长期方向是：

- `method_names` 这类名字桥大部分应被更正式的 `MethodImpl::PythonShim(...)` 替代
- `class_types` / `java_instances` 若仍保留，也应下沉到专门 binding adapter，而不是挂在 `PyEmulator` 主状态上

对应地，Rust builtin 的并行链路应为：

1. Rust macro / builder / builtin 声明生成 class metadata
2. metadata 被规整为统一 `JClassDef`
3. 统一注册到 `Emulator` 持有的 `AndroidRuntime`
4. `AndroidRuntime` 内部的 `AndroidVM` / `JniRegistry` 成为最终 authority
