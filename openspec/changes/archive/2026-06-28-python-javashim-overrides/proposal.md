## Why

`jni-shim-foundation` 定义了最小 Python shim 轮廓，但没有单独明确它如何和 framework stub 共存、如何做 override、以及优先级如何固定。

如果这层不拆出来，后面很容易把 Python 重新写成“弱类型补丁层”。

## What Changes

这个 change 定义 Python javashim override 模型。

本次变更引入：

- 新 capability：`python-javashim`
- metadata-only decorators
- explicit `register(...)`
- Python class metadata 到 Rust `JClassDef` 的显式同步桥
- Python shim 与 framework stub 的 override 优先级
- strict type verify 与 runtime return validation

这里特别要求：

- Python 只是注册接口，不是最终 authority
- Rust builtin 和 Python 注册结果必须进入同一套 Rust VM 数据模型

本次变更不要求：

- guest ABI table
- framework class 集合本身

## Capabilities

- python-javashim
- testing-harness
