## ADDED Requirements

### Requirement: Declarative Case Format

项目 SHALL 为 runtime smoke/regression testing 提供声明式 case 格式。

#### Scenario: Case manifest exists

- **WHEN** 新增一个 runtime case
- **THEN** 它 SHALL 包含 `case.toml`
- **AND** 它 SHALL 包含 `script.py`
- **AND** harness SHALL 不需要修改核心代码就能运行它

### Requirement: Resource URI Resolution

harness SHALL 通过逻辑 resource URI 解析大资源，而不是依赖硬编码绝对路径。

#### Scenario: Resolve resource URI

- **WHEN** case 引用 `resource:<pack>/...`
- **THEN** harness SHALL 通过资源系统解析它
- **AND** 如果 pack 不可用，SHALL 给出可操作的错误

### Requirement: Optional Java Oracle

differential harness SHALL 允许 Java baseline oracle 是可选项。

#### Scenario: Rust-only workflow

- **WHEN** Java oracle 不可用
- **AND** case 将 oracle 标记为 optional
- **THEN** Rust runner SHALL 仍然能成功执行这个 case
- **AND** harness SHALL 记录 oracle 步骤被跳过
