## ADDED Requirements

### Requirement: Framework stubs are regression-tested through harness

testing harness SHALL 覆盖核心 Android framework stub 行为。

#### Scenario: Package metadata path is covered

- **WHEN** harness 运行 package/signature case
- **THEN** case SHALL 能断言 `getPackageName()`、`getPackageInfo()`、`Signature.hashCode()` 或等价行为

#### Scenario: Service lookup path is covered

- **WHEN** harness 运行 `getSystemService()` case
- **THEN** case SHALL 能断言 service lookup 通过 registry 返回稳定 stub
