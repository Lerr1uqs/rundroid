## ADDED Requirements

### Requirement: JavaClass MRO override is keyed by Java descriptor

`JavaClass.__init_subclass__` 在扫描 MRO 收集 `@java_method` 元数据时 SHALL 以
Java method descriptor 作为覆写判定键，而不是 Python 方法名。

#### Scenario: Subclass overrides parent method with same descriptor but different Python name

- **WHEN** 父类声明 `@java_method("foo()I") def base_impl(...)`
- **AND** 子类声明 `@java_method("foo()I") def child_impl(...)`
- **THEN** 子类 class 创建后 `__java_methods__` SHALL 仅保留子类对应的 `foo()I`
- **AND** 父类对应的 `foo()I` SHALL NOT 出现在该子类最终方法集合中

#### Scenario: Registration does not fail for descriptor-based override

- **WHEN** 子类以相同 Java descriptor 覆写父类方法
- **AND** 调用 `register(emulator, [ChildClass])`
- **THEN** 注册流程 SHALL 成功
- **AND** SHALL NOT 因重复 method 注册而失败

### Requirement: JavaClass MRO keeps overloads with different descriptors

同一 Java 方法名下，只要 descriptor 不同，MRO 收集结果 SHALL 同时保留这些重载。

#### Scenario: Same Java name with different descriptors remain in dispatch table

- **WHEN** 某 JavaClass 最终视图里存在 `foo(I)I` 与 `foo(D)I`
- **THEN** `__java_dispatch__["foo"]` SHALL 同时包含这两个重载
- **AND** 这些条目 SHALL NOT 因为 Java 方法名相同而互相覆盖

#### Scenario: Descriptor override does not remove unrelated overload

- **WHEN** 父类声明 `foo()I` 与 `foo(I)I`
- **AND** 子类仅覆写 `foo()I`
- **THEN** 子类最终方法集合 SHALL 包含子类版本的 `foo()I`
- **AND** SHALL 继续包含未被覆写的 `foo(I)I`

### Requirement: Descriptor-based override drives instance dispatch

MRO 处理后的最终方法集合 SHALL 同时驱动 Rust 注册列表与 Python 实例分派表，
两者语义 MUST 一致。

#### Scenario: Instance call resolves to subclass implementation after override

- **WHEN** 子类以相同 descriptor 覆写父类方法并成功构造实例
- **AND** 该方法通过 Java 名被实例调用
- **THEN** 调用 SHALL 命中子类方法体
- **AND** SHALL NOT 命中被覆写的父类实现
