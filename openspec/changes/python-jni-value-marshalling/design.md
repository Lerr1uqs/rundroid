## Context

项目当前的 JNI foundation 已经具备：

- `JType` / `JValue` 的强类型值模型
- `ObjectStorage` / `ObjectStore` 的对象身份与存储
- `MethodSig` / `FieldSig` / `ClassId` 的签名级调度
- Python `JavaClass` / `JavaObject` 的对象模型

真正没接通的是 Python/Rust 之间的值编组：

- Python 参数传入 Rust 时，`str`、`bytes`、对象参数会掉进 `Null`
- Rust 回 Python 时，`JValue::Object` 直接变成 `None`
- 这会直接破坏 `Signature([B)V`、`java/lang/String`、byte array 的基础样板

约束：

- 不引入新的重依赖。
- 保持 fail-fast，不做大量兜底。
- 默认语义要“无痛”，但保留显式 wrapper 以便需要身份的场景。
- 不把 `JavaString` 设计成唯一入口，默认 Python 原生类型应尽量可直接用。

## Goals / Non-Goals

**Goals:**

- 打通 `str` / `bytes` / `None` / primitive / object 的双向编组。
- 让 `java/lang/String` 与 `byte[]` 可以用 Python 原生类型直接写测试。
- 提供显式 wrapper 作为 opt-in 身份层。
- 覆盖 guest→Python、Python→guest 两个方向的回归测试。

**Non-Goals:**

- 不在本 change 中实现全部 primitive array 类型的完整对称映射。
- 不在本 change 中实现 `GetStringUTFChars` / `GetByteArrayElements` 的完整 C API 级别仿真。
- 不在本 change 中扩展到 `Object[]` 的全套自动转换。
- 不改变现有 `JValue` / `ObjectStorage` 的基本类型系统。

## Decisions

### 决策 1：默认采用自动 coercion，避免脚本层必须显式包装

Python 侧输入/输出优先支持原生值：

- `str` ↔ `java/lang/String`
- `bytes` ↔ `byte[]`
- `None` ↔ `null`
- `bool` / `int` / `float` ↔ 对应 primitive

理由：

- Python 脚本层以易写为第一优先级。
- `Signature([B)V`、`String` 是基础用例，不应要求手工 wrapper。
- 值语义天然适合自动拷贝式转换。

备选方案：

- 只提供显式 wrapper：身份清晰，但脚本成本高，基础用例太重。
- 全部都只回 `None`：当前状态，等于不可用。

### 决策 2：显式 wrapper 仅作为 opt-in 身份层

保留一个轻量 wrapper 层，用于需要稳定 identity 的对象：

- `JavaString`
- `JavaByteArray`

默认场景下不用它们；只有需要复用 handle / object identity 时才显式构造。

理由：

- 自动 coercion 会丢失身份，不能覆盖所有场景。
- wrapper 作为 opt-in 不会破坏脚本便利性。

### 决策 3：Rust 侧以 `ObjectStorage` 为最终 authority，Python 侧只做语义层封装

所有 `String` / `byte[]` / wrapper 的真实载体仍由 Rust `ObjectStore` 持有。
Python 侧只负责：

- 把原生值转成适当的 `JValue::Object(oid)` / `JValue::Null`
- 把 `JValue::Object(oid)` 再投影回 Python 值或 wrapper

理由：

- 现有 Rust foundation 已经有 `make_string`、`make_wrapper`、`make_primitive_array`。
- authority 继续留在 Rust，避免 Python 侧自建第二套对象真相。

### 决策 4：对 `JValue::Object` 回 Python 必须做 storage-aware 分发

回 Python 时不再统一返回 `None`，而是按 `ObjectStorage` 分发：

- `ObjectStorage::String` → Python `str`
- `ObjectStorage::PrimitiveArray(Byte)` → Python `bytes`
- `ObjectStorage::Wrapper` → 对应 Python 标量或 wrapper
- 其他对象 → 保留为 Python `JavaObject` / handle wrapper（按现有对象模型）

理由：

- `None` 会吞掉真实参数与返回值。
- 编组层必须知道对象的 storage 类型，否则对象语义全丢。

### 决策 5：`convert_pyargs_to_jniargs` 与 `py_object_to_jvalue` 共用一个类型规则表

两个方向的编组规则必须成对实现，避免“进得去、回不来”：

- Python -> JNI 参数
- Python 返回 -> JValue
- JValue -> Python 参数

理由：

- 现在的 bug 本质是单向补丁，缺少闭环。
- 同一类型的往返必须稳定一致。

## Risks / Trade-offs

- [风险] 自动 coercion 可能隐藏身份丢失 → 缓解：显式 wrapper 保留 opt-in identity 路径。
- [风险] `bytes`/`str` 与对象引用的边界有歧义 → 缓解：规则优先级固定，且在测试里锁定。
- [风险] `Object[]` / 更复杂数组会拖大范围 → 缓解：本 change 先只接通最关键的 `String`、`byte[]`、primitive、object。
- [风险] 回 Python 时做 storage-aware 分发会增加 binding 复杂度 → 缓解：复杂度只集中在三个编组点，换来整体可用性。

## Migration Plan

1. 先把 Python->Rust 的 `str`、`bytes`、primitive、None 编组打通。
2. 再把 Rust->Python 的 `JValue::Object` / `Null` 回编组打通。
3. 引入 `JavaString` / `JavaByteArray` 作为显式 wrapper。
4. 补 `Signature([B)V`、`String`、`byte[]` 的回归测试。
5. 若某步影响面过大，可先仅开启自动 coercion，wrapper 延后。

## Open Questions

- `JavaString` / `JavaByteArray` 是否作为 public API 暴露，还是先只在内部使用？
- `Object[]` 是否在同一 change 中顺带支持，还是单独开后续 change？
