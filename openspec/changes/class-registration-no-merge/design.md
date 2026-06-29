# Design

## 动机核对：spec 已要求 fail-fast，实现跑偏

`openspec/specs/jni-shim/spec.md` 的 "Registry-backed class and member definitions" 已含 scenario **Registration collisions fail fast**：

> 重复注册同一个 class、method 或 field 签名 → 立即返回显式错误，不 SHALL 静默覆盖。

`register_or_merge_class` 的合并分支违反这条：同名 class 二次注册时静默替换 method/field。本 change 是修正 spec/实现偏离，不是新增约束。

## 调研：merge 的真实依赖面（砍除前确认零生产风险）

| 依赖点 | 生产代码消费者 | 仅测试 |
|---|---|---|
| `register_or_merge_class` 合并分支 | `register_framework_stub`（test harness 用） | registry/framework merge 单测 |
| `FrameworkRegistry::install` 幂等 | **无**——`AndroidVM::new` 不装 framework，Python 绑定层也不调 | `install_is_idempotent_via_merge` |
| `override_method` / `override_field` | **无**——只被 merge 编排调用 | class.rs / framework_harness 单测 |

framework 注册的 class 全是 `android/*`（Context / Signature / Bundle ...），Python 用户注册自己的 class（`com/scene/*` 等），命名空间不重叠——**正常用法从不触发 merge**。砍除对正常用法零影响，唯一行为变化是"Python 想覆盖一个已存在的 class 名"从静默 merge 变成明确报错（正是 spec 要求）。

## 关键决策

### 1. `register_class` 成为唯一入口（顺带收敛命名）

砍掉 `register_or_merge_class` 后，`register_class`（已存在即 `DuplicateRegistration`）成为唯一 class 注册入口。这同时解决了此前命名不适——主语义（fail-fast）占有朴素名字，不再有暴露实现分支的 `register_or_merge_class`。硬报错语义本就是默认；调用统计也印证：`register_or_merge_class` 服务生产路径，`register_class` 此前仅测试用。

### 2. `override_method` / `override_field` 删除（不保留）

这俩是 JClassDef 的低层"替换"能力，生产代码的唯一消费者是 merge 编排。删除而非保留，理由：

- YAGNI——"干掉全部相同的情况"要求彻底清理重复定义的容错路径，保留即留下"可绕过 fail-fast"的口子。
- 若未来确需"替换 method"语义，应在显式、受控的 API 上重新引入，而非沿用为 merge 服务的隐式方法。

删除波及 `class.rs` 自身单测与 `framework_harness.rs:353` 一处用例，改写为 `add_method` 或重写场景。

### 3. framework install 幂等 → 二次报错

`install_is_idempotent_via_merge` 是靠 merge 实现的幂等。生产代码无 install 调用（framework 装配仅在 case-runner / 测试路径），不存在"必须容忍重复 install"的生产约束。二次 install 直接在首个重复 class 上 `DuplicateRegistration`，不引入 installed flag（YAGNI）。测试改写为"二次 install 报错"。

### 4. Python 错误映射

`register_java_class` / `register_framework_stub` 捕获 `DuplicateRegistration` → `ValueError`，信息含 class 名 + "重复定义暂不支持"。fail-fast、可定位。

## 不受影响

- **继承**：`JClassDef.superclass` + `class_chain` / `resolve_inherited_method` / `resolve_method_by_id` 完全独立于 merge，零改动。
- **framework stub 能力本身**：builtin class 仍可经 `install` / `register_framework_stub` 注册，只是不再支持 Python 覆盖同名 class。

## 风险

低。生产无 framework install、merge 无生产消费者；唯一行为变化（Python 覆盖已存在 class 名 → 报错）是 spec 既定要求。全量测试 + Python 回归在 Phase 4 兜底。
