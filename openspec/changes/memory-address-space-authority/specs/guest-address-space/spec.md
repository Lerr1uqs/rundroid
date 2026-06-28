## ADDED Requirements

### Requirement: MemoryAddressSpace is the single guest VMA authority

runtime SHALL 通过单一 `MemoryAddressSpace` 管理所有 guest VMA 事实。ELF image、匿名 `mmap`、fd/device `mmap`、JNI ABI、trampoline、stack、scratch 等区间不 SHALL 各自维护互不知情的地址分配真相。

#### Scenario: All guest mappings share one authority

- **WHEN** runtime 需要为 ELF、Linux `mmap`、JNI ABI 或 runtime scratch 建立 guest 映射
- **THEN** 它 SHALL 先通过同一个 `MemoryAddressSpace` 请求地址分配或固定地址校验
- **AND** 不 SHALL 通过私有 reserve cursor、独立 `next_mmap` 或等价的第二套真相直接决定地址

### Requirement: Allocation modes are Reserved and Dynamic

`MemoryAddressSpace` SHALL 把 guest 地址分配模式限定为 `Reserved` 与 `Dynamic` 两类。`Reserved` 表示固定地址/固定布局请求；`Dynamic` 表示由地址空间 authority 统一找洞分配。

#### Scenario: Reserved allocation validates fixed address

- **WHEN** 调用方向 `MemoryAddressSpace` 发起 `Reserved` 分配请求
- **THEN** 请求 SHALL 显式提供目标地址
- **AND** `MemoryAddressSpace` SHALL 在 materialize 前校验该区间是否与现有 VMA 冲突

#### Scenario: Dynamic allocation chooses address from shared VMA view

- **WHEN** 调用方向 `MemoryAddressSpace` 发起 `Dynamic` 分配请求
- **THEN** `MemoryAddressSpace` SHALL 基于当前已记录的 VMA 视图完成对齐与 gap search
- **AND** 调用方不 SHALL 自行推进独立游标来绕过该分配逻辑

### Requirement: Overlap is rejected before backend mapping

guest 区间重叠 SHALL 在 `MemoryAddressSpace` 内、backend `mem_map` 之前被检测并上抛结构化错误。backend 不 SHALL 作为主冲突检测路径。

#### Scenario: Reserved overlap raises immediately

- **WHEN** 某个 `Reserved` 分配请求与现有 VMA 相交
- **THEN** `MemoryAddressSpace` SHALL 在调用 backend 前返回 overlap 错误
- **AND** 不 SHALL 先调用 backend 再依赖引擎拒绝来暴露冲突

#### Scenario: Dynamic allocation skips occupied gaps

- **WHEN** `Dynamic` 分配需要在某个 arena 内选择地址
- **THEN** `MemoryAddressSpace` SHALL 跳过与现有 VMA 冲突的区间并继续寻找可用 gap
- **AND** 找不到可用区间时 SHALL 返回失败而不是返回一个已被占用的地址

### Requirement: Successful allocation is eagerly materialized

bootstrap 阶段 `rundroid` SHALL 采用 eager materialize 语义：任何成功创建的 guest VMA 都必须立即落到 backend。当前阶段不支持 lazy paging、基于 page fault 的按需建图、file pager 或未 materialize 的 VMA。

#### Scenario: Backend failure leaves no ledger entry

- **WHEN** `MemoryAddressSpace` 完成预检查后调用 backend 建图
- **THEN** 只有 backend 成功时它 SHALL 把该区间写入 VMA 账本
- **AND** 如果 backend 失败，`MemoryAddressSpace` SHALL 返回失败且账本中不保留残留区间

#### Scenario: Unmapped guest access does not trigger auto paging

- **WHEN** guest 访问某个未映射地址
- **THEN** runtime SHALL 直接表现为未映射访问失败
- **AND** 不 SHALL 自动创建页面或按需 materialize 新区间

### Requirement: VMA ledger tracks protect and unmap effects

`MemoryAddressSpace` SHALL 维护与 backend 一致的 VMA 账本，`mprotect` 与 `munmap` 的效果必须回写到账本并影响后续分配。

#### Scenario: Protect updates VMA permission view

- **WHEN** runtime 对一段已映射 guest 区间执行 `mprotect` 或等价权限收紧
- **THEN** `MemoryAddressSpace` SHALL 在 backend 成功后更新该区间的权限视图
- **AND** 后续调试查询或 maps 输出 SHALL 反映更新后的权限

#### Scenario: Munmap releases gaps for future dynamic allocation

- **WHEN** runtime 对一段已映射 guest 区间执行 `munmap`
- **THEN** `MemoryAddressSpace` SHALL 在 backend 成功后删除或拆分对应 VMA
- **AND** 后续 `Dynamic` 分配 SHALL 能看到新释放出来的 gap

### Requirement: VMA entries carry mapping usage metadata

`MemoryAddressSpace` SHALL 为每个 VMA 记录足够的用途元数据，以区分不同 guest 区间的来源与意图。

#### Scenario: Mapping records preserve usage identity

- **WHEN** runtime 创建一个新的 guest VMA
- **THEN** 该记录 SHALL 至少包含地址范围、权限、分配模式与 usage/source 元数据
- **AND** usage 元数据 SHALL 能区分 ELF image、匿名 `mmap`、file/device `mmap`、JNI ABI、trampoline、stack、scratch 或等价类别
