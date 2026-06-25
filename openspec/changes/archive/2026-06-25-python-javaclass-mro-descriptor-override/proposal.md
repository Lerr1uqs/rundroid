## Why

`python-javaclass-call` 已经把 Python shim 的实例构造、调用分派和 VM handle 接起来了，
但 `JavaClass.__init_subclass__` 当前按 Python 方法名去重，而不是按 `@java_method`
声明的 Java descriptor 去重。结果是子类想覆写父类同一 Java 方法时，只要 Python 函数名不同，
父子两份实现都会进入注册表，最终在 Rust 注册阶段报重复注册，破坏了 MRO 的覆盖语义。

## What Changes

- 将 `JavaClass.__init_subclass__` 的 MRO 收集/去重规则改为以 `@java_method` descriptor 为准，
  不再以 Python `attr_name` 作为唯一覆盖键。
- 明确区分“覆写”和“重载”：
  同一 descriptor 在 MRO 中只保留最近一层实现；
  同一 Java 方法名下的不同 descriptor 继续并存，供 argc 分派使用。
- 为继承覆写场景补充回归测试，覆盖：
  子类覆写父类 descriptor、父类 descriptor 不应重复注册、实例调用命中子类实现。
- 文档化这一规则，使 `JavaClass` 的 MRO 行为与 Python 继承直觉一致。

## Capabilities

### New Capabilities

- `python-javaclass-mro-override`: 规范 JavaClass 在 MRO 扫描时按 Java descriptor 处理覆写与重载的行为

### Modified Capabilities

无。

## Impact

- 受影响代码：
  - `python/rundroid/javashim/base.py`
  - `python/tests/test_javaclass_call.py`
- 间接受影响：
  - `python/rundroid/javashim/registry.py` 读取的 `__java_methods__`
  - `emulator/bindings/python/src/lib.rs` 的 class 注册路径
- 对外 API 不新增也不删除，但会修正继承场景下的运行时行为，使子类覆写按 spec 生效。
