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
