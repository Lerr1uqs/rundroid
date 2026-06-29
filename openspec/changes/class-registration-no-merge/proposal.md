## Why

`JniRegistry::register_or_merge_class` 的合并语义**违反 jni-shim spec 已有的 "Registration collisions fail fast" requirement**（`openspec/specs/jni-shim/spec.md`）：spec 要求"重复注册同一个 class/method/field 签名 → 立即返回显式错误，不 SHALL 静默覆盖"，而 merge 分支恰恰在做"静默覆盖"——同名 class 二次注册时，新 method/field 替换旧实现、未覆盖的保留。

merge 语义是为 **Python shim 覆盖 Rust framework stub** 这个能力引入的，但该能力当前**无生产消费者**：

- framework 批量 `install` 在生产代码里从不被调用（`AndroidVM::new` 不装 framework；Python 绑定层也不调），只在 case-runner 与 framework 自身的测试/harness 里跑。
- Python 用户注册自己的 class（如 `com/scene/*`），与 framework 的 `android/*` 命名空间不冲突，正常用法从不触发 merge。

代价却是实打实的：注册入口分裂成两个（`register_class` 严格报错 vs `register_or_merge_class` 合并），主语义（fail-fast）反而不占朴素的 `register_class` 名字、命名暴露实现分支；merge 的"同名替换、其余保留"规则隐式且复杂，与项目 fail-fast 原则相悖。

本 change 删除 merge 语义，让实现对齐 spec：重复注册同名 class 一律 `DuplicateRegistration` 报错。

## What Changes

本次变更引入：

- 删除 `JniRegistry::register_or_merge_class`，其合并分支（`override_method` / `override_field` 编排）一并移除；所有调用方迁移到 `register_class`（重复注册 → `JniError::DuplicateRegistration`）。`register_class` 成为唯一 class 注册入口。
- 删除 `JClassDef::override_method` / `override_field`——生产代码无消费者，仅 merge 编排与自身单测使用。
- `FrameworkRegistry::install`：移除"二次 install 靠 merge 幂等"的依赖，二次 install 直接报错（生产无重复 install 调用）。
- Python `register_framework_stub`：内部改用 `register_class`，重复注册同名 class 返回明确 `ValueError`。
- 重复注册的 Python 错误信息 SHALL 点名冲突的 class 名与"重复定义暂不支持"。
- 删除/改写所有 merge 相关测试（registry / class / framework 的 merge/override 单测、harness 中的 override 用例）。

本次变更不要求：

- 继承（superclass + `class_chain`）语义任何变动——那是独立线，不受 merge 删除影响。
- framework stub 能力本身的移除——builtin class 仍可注册，只是不再支持 Python 覆盖同名 class。
- `JniError::DuplicateRegistration` 类型的新增（复用现有）。

## Capabilities

- jni-shim
