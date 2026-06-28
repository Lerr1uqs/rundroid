## ADDED Requirements

### Requirement: Python values SHALL coerce to JNI values automatically

Python 侧传入 Rust 的参数 SHALL 支持自动 coercion 到 JNI 值。

#### Scenario: Primitive Python values convert to primitive JNI values

- **WHEN** Python 传入 `None`、`bool`、`int` 或 `float`
- **THEN** binding SHALL 将其转换为对应的 `JValue::Null`、`JValue::Boolean`、`JValue::Int` / `Long`、`JValue::Double`

#### Scenario: Python bytes and str convert to object JNI values

- **WHEN** Python 传入 `bytes` 或 `str`
- **THEN** binding SHALL 将其转换为可进入 `ObjectStore` 的对象值
- **AND** SHALL NOT 静默吞成 `Null`

### Requirement: JNI object values SHALL convert back to Python values by storage type

Rust 返回到 Python 的 `JValue::Object` SHALL 按 `ObjectStorage` 类型回编组。

#### Scenario: Java String returns as Python str

- **WHEN** Rust 返回一个 `java/lang/String` 对象
- **THEN** Python SHALL 收到对应的 `str`

#### Scenario: byte array returns as Python bytes

- **WHEN** Rust 返回一个 `byte[]`
- **THEN** Python SHALL 收到对应的 `bytes`

#### Scenario: Null is preserved as Python None

- **WHEN** Rust 返回 `JValue::Null`
- **THEN** Python SHALL 收到 `None`

### Requirement: Python shim method arguments SHALL receive concrete values for String and byte[]

guest/native 调用 Python `@java_method` 时，参数编组 SHALL 将 `String` 与 `byte[]` 还原成可直接使用的 Python 值。

#### Scenario: Signature byte array argument is received as bytes

- **WHEN** guest 调用 `Signature([B)V`
- **THEN** Python 方法参数 SHALL 是 `bytes`
- **AND** SHALL NOT 是 `None`

#### Scenario: Java String argument is received as str

- **WHEN** guest 调用带 `String` 参数的方法
- **THEN** Python 方法参数 SHALL 是 `str`
- **AND** SHALL NOT 是 `None`

### Requirement: Explicit wrappers SHALL be available for identity-sensitive builtins

Python 侧 SHALL 提供显式 wrapper，用于 identity-sensitive 的内置 Java 值。

#### Scenario: Python can explicitly create a Java string wrapper

- **WHEN** 用户需要保留字符串对象身份
- **THEN** Python SHALL 提供一个显式的 Java string wrapper 创建路径

#### Scenario: Python can explicitly create a byte array wrapper

- **WHEN** 用户需要保留 byte array 对象身份
- **THEN** Python SHALL 提供一个显式的 byte array wrapper 创建路径

### Requirement: Explicit wrappers SHALL manage release through Python lifecycle hooks

显式 wrapper SHALL 提供显式 `release()`，并在不形成循环引用时通过 `__del__` 触发释放兜底。

#### Scenario: Wrapper release is explicit and idempotent

- **WHEN** 用户手动调用 wrapper 的 `release()`
- **THEN** 对应的 JNI / ObjectStore 资源 SHALL 被释放
- **AND** 再次调用 `release()` SHALL NOT 引发重复释放错误

#### Scenario: Wrapper __del__ triggers best-effort release

- **WHEN** wrapper 被 Python GC 回收且未形成循环引用
- **THEN** `__del__` SHALL 尝试触发释放
- **AND** SHALL NOT 依赖循环引用场景才能完成释放

### Requirement: Marshalling SHALL remain fail-fast on unsupported values

对未支持的值类型，binding SHALL 直接报错，不得静默降级为 `Null`。

#### Scenario: Unsupported Python value raises error

- **WHEN** Python 传入未支持的复杂对象且没有 wrapper/coercion 规则
- **THEN** binding SHALL 抛出异常
- **AND** SHALL NOT 将其静默变成 `Null`
