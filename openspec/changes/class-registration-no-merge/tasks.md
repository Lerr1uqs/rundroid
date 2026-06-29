# Implementation Tasks

## Phase 1: 删 merge，统一注册入口

- [ ] 删除 `JniRegistry::register_or_merge_class`（`emulator/jni/src/registry.rs`），其合并分支整体移除。
- [ ] 迁移所有调用方到 `register_class`：
  - [ ] `emulator/bindings/python/src/lib.rs` — `register_java_class`（Python 用户注册路径）。
  - [ ] `emulator/bindings/python/src/lib.rs` — `register_framework_stub`（harness 注册路径）。
  - [ ] `emulator/jni/src/framework/registry.rs` — `FrameworkRegistry::install` 内逐个 builtin 注册。
- [ ] 确认 `register_class` 在 class 已存在时返回 `JniError::DuplicateRegistration`（沿用现有行为，不新增类型）。
- [ ] `cargo test -p rundroid-jni` 跑绿。

## Phase 2: 删 override_method / override_field

- [ ] 删除 `JClassDef::override_method` / `override_field`（`emulator/jni/src/class.rs`）——生产无消费者。
- [ ] 删除 `class.rs` 中对应的 override 单测（`override_method` / `override_field` 测试块）。
- [ ] 改写 `emulator/jni/tests/framework_harness.rs` 中用到 `override_method` 的用例（改用 `add_method` 或重写场景）。
- [ ] `cargo test -p rundroid-jni` 跑绿。

## Phase 3: framework install 幂等改报错

- [ ] 删除/改写 `install_is_idempotent_via_merge` 测试（`emulator/jni/src/framework/registry.rs`）为"二次 install 报 `DuplicateRegistration`"。
- [ ] 确认 install 内部逐个 `register_class`，首个重复即报错（无需额外 flag）。
- [ ] `cargo test -p rundroid-jni` 跑绿。

## Phase 4: Python 错误信息 + 全量回归

- [ ] `register_java_class` / `register_framework_stub` 把 `DuplicateRegistration` 映射成 `ValueError`，信息含 class 名 + "重复定义暂不支持"。
- [ ] 重建 Python 绑定：`cd python && source .venv/Scripts/activate && maturin develop`。
- [ ] 补一个 Python 测试：重复注册同名 class 抛 `ValueError` 且信息点名 class（`python/tests/`）。
- [ ] 全量回归：`uv run pytest tests/` 绿；`cargo test --workspace` 绿。
- [ ] 删除/改写任何依赖 merge 的 Python 测试。

## Phase 5: 归档与验证

- [ ] `openspec validate --type change class-registration-no-merge --strict` 通过。
- [ ] 更新 MEMORY（merge 语义已移除，注册入口统一为 `register_class`）。
