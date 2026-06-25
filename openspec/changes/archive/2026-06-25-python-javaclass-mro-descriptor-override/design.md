## Context

`python-javaclass-call` 在 `JavaClass.__init_subclass__` 中扫描 MRO，并宣称“最近一层定义胜出”。
但实际实现按 Python `attr_name` 去重，而不是按 `@java_method` 的 Java descriptor 去重。

这会带来两个问题：

1. 子类若用不同 Python 函数名覆写父类同一 Java 方法，父子方法都会进入 `__java_methods__`，
   在 Rust 注册时触发重复注册错误。
2. `__java_dispatch__` 里也会混入同一 descriptor 的多份实现，虽然运行时按“首个匹配”可能偶然可用，
   但注册路径已经先失败，且行为不稳定。

约束：

- 不改变现有 `@java_method` 装饰器 API。
- 不改变“同一 Java 方法名下不同 descriptor 可以并存”的重载模型。
- 继续保持 import 阶段纯 Python、无 runtime side effect。

## Goals / Non-Goals

**Goals:**

- 让 MRO 覆写语义以 Java descriptor 为准，而不是 Python 方法名。
- 保证同一 descriptor 在 `__java_methods__` 中只保留最近一层实现。
- 保证不同 descriptor 的重载继续保留，并维持现有 `__java_dispatch__` 的 argc 分派行为。
- 为该行为补充稳定的回归测试。

**Non-Goals:**

- 不升级重载解析到 descriptor 级精确类型匹配，仍然沿用现有 argc 分派。
- 不处理 `@classmethod` 支持策略，这属于独立问题。
- 不修改 Rust registry merge 语义，本 change 只修正 Python 侧输入集合。

## Decisions

### 决策 1：MRO 去重键改为 Java descriptor

`JavaClass.__init_subclass__` 在扫描每个候选方法时，先解析出 `desc`，再以 `desc` 作为覆盖键。

理由：

- 覆写的真实语义是“这个 Java 方法签名由谁实现”，不是“这个 Python 函数名是否重复”。
- Python shim 允许用户用不同 Python 名去承载同一个 Java descriptor，这是当前设计的一部分。

备选方案：

- 继续用 `attr_name` 去重：错误，无法表达“不同 Python 名映射同一 Java 方法”的覆写。
- 用 `java_name` 去重：也不够，因为会错误吞掉同名不同签名的重载。

### 决策 2：`__java_methods__` 和 `__java_dispatch__` 共用同一份 descriptor 过滤语义

方法收集时，若某个 descriptor 已经被更近一层类占用，则父类该 descriptor 直接跳过，
既不进入 `__java_methods__`，也不进入 `__java_dispatch__`。

理由：

- 两份结构都应反映同一个“最终蓝图视图”。
- 只修一份会造成注册路径和 Python 调用路径语义分叉。

### 决策 3：重载保留规则不变

若两个方法 Java 名相同但 descriptor 不同，例如 `foo(I)I` 和 `foo(D)I`，
它们仍都进入 `__java_methods__` 和 `__java_dispatch__`。

理由：

- 这是已有 `python-javaclass-call` 的能力，不属于本次修复范围。
- 本 change 只修复“同一 descriptor 的覆写”，不回退既有重载能力。

### 决策 4：测试直接覆盖“不同 Python 名、相同 descriptor”的真实故障模式

新增/调整测试时，优先覆盖如下模式：

- 父类：`base_impl -> foo()I`
- 子类：`child_impl -> foo()I`

注册后应只保留 `child_impl` 对应的方法定义，实例调用也应返回子类结果。

理由：

- 这是当前实现实际出错的最小复现。
- 它比“同名同函数名覆写”更严格，能防止实现退化回 `attr_name` 去重。

## Risks / Trade-offs

- [风险] 修改去重键后，若现有代码误依赖“父类同 descriptor 也被保留”的错误行为，行为会变化
  → 缓解：这是修正为 spec 语义，补明确测试并在 change 中记录。
- [风险] 若 descriptor 解析失败，收集顺序会受异常影响
  → 缓解：保持现有 fail-fast，不为非法 descriptor 加兜底。
- [风险] 同一类内部若手工构造重复 descriptor，仍可能在注册阶段失败
  → 缓解：本 change 只处理 MRO 覆写，不放宽同层重复定义。

## Migration Plan

1. 修改 `JavaClass.__init_subclass__` 的收集与去重逻辑。
2. 补充/调整 `python/tests/test_javaclass_call.py` 回归用例。
3. 在 `python/` 项目上下文执行相关 pytest 用例，确认继承覆写与既有重载能力都不回归。
4. 如需回退，只需恢复旧的 MRO 收集逻辑并删除新增测试。

## Open Questions

- 无。本次 change 的目标和边界都比较清晰。
