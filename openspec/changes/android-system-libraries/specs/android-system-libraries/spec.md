## ADDED Requirements

### Requirement: SystemLibraryResolver provides DT_NEEDED fallback

loader SHALL 提供 `SystemLibraryResolver` 组件，在 linker resolve DT_NEEDED 符号时作为三段回退的末段：current module → DT_NEEDED 依赖图 → SystemLibraryResolver。SystemLibraryResolver SHALL 接收系统库目录路径，当某个 DT_NEEDED soname（如 `libc.so`）在前两段未找到时，SHALL 查系统库目录是否存在同名 .so 文件。命中 SHALL 通过 `load_inner` 与 guest .so 相同的加载流程装载，并标记为系统库（`allow_unresolved=true` + `skip_init=true`）。未命中 SHALL 保持 unresolved 状态，不视为致命错误以外的异常。

#### Scenario: System library resolves when guest and deps don't have it

- **WHEN** guest .so 声明 `DT_NEEDED libc.so`
- **AND** libc.so 不在当前已加载模块或依赖图中
- **THEN** SystemLibraryResolver SHALL 在系统库目录中找到 `libc.so`
- **AND** loader SHALL 以普通 ELF 加载流程装载它（PT_LOAD 映射、relocation、符号导出）

#### Scenario: System library not found leaves unresolved

- **WHEN** guest .so 声明 `DT_NEEDED nonexistent.so`
- **AND** nonexistent.so 不在系统库目录中
- **THEN** SystemLibraryResolver SHALL 返回 None
- **AND** linker SHALL 保持该依赖 unresolved，不额外介入

#### Scenario: Guest dep takes priority over system library

- **WHEN** guest 自身携带一个同名 `libfoo.so` 作为 DT_NEEDED 第一段可解析的依赖
- **THEN** linker SHALL 使用 guest 提供的 libfoo.so
- **AND** SHALL NOT 回退到系统库目录中的同名文件

### Requirement: System libraries allow unresolved symbols

loader SHALL 将系统库模块标记为 `allow_unresolved=true`。linker 在符号解析阶段遇到系统库自身导入的符号在依赖图中找不到时，SHALL NOT 视为致命错误，SHALL 将该符号记录到 `unresolved_symbols` 列表（供诊断/telemetry），并将引用地址落地为 0（弱符号语义），继续完成链接。

#### Scenario: Unresolved symbol in system library does not fail

- **WHEN** linker 解析系统库（如 libc.so）的导入符号 `__cxa_finalize`
- **AND** 该符号不在任何已加载模块的导出表中
- **THEN** linker SHALL NOT 报错或 panic
- **AND** SHALL 将该符号添加到 unresolved_symbols 记录
- **AND** SHALL 将引用该符号的 relocation 地址写为 0
- **AND** SHALL 继续执行后续 relocation

#### Scenario: Guest unresolved symbol still fails

- **WHEN** linker 解析 guest .so 的导入符号
- **AND** 该符号不在 guest 导出、依赖图或系统库中
- **THEN** linker SHALL 保持现有行为（error / panic / 以类型约定的方式报告失败）
- **AND** SHALL NOT 因为系统库 allow_unresolved 而宽容 guest 的 unresolved

### Requirement: System libraries skip init_array

loader SHALL 将系统库模块标记为 `skip_init=true`。linker 构建 init 调用顺序时 SHALL 跳过标记为 `skip_init` 的模块，不调用其 `.init_array` 或 `.init` 段中的函数。

#### Scenario: Init plan excludes system libraries

- **WHEN** linker 生成 init 调用顺序
- **AND** 模块图中包含标记 `skip_init=true` 的系统库模块
- **THEN** SHALL NOT 将该系统库的 init_array 或 init 函数加入 init 顺序
- **AND** SHALL 正常包含 guest .so 模块的 init 函数

#### Scenario: System library init skipped even if non-empty

- **WHEN** 系统库 libc.so 包含非空的 `.init_array` 段
- **AND** 该模块标记了 `skip_init=true`
- **THEN** linker SHALL 跳过执行它的 init_array
- **AND** SHALL NOT 产生错误或警告

### Requirement: libdl exported functions intercepted via hook

loader SHALL 识别模块身份（通过 soname 或文件名匹配 `libdl.so`）。当系统库 `libdl.so` 被加载后，SHALL 将其 `dlopen` / `dlsym` / `dlclose` / `dlerror` 四个导出函数的入口点替换为 trampoline hook（类似 `install_syscall_hook` 的 `on_code` 机制）。第一期 hook 实现为 stub：dlopen 返回 NULL（失败），dlsym 返回 NULL（未找到），dlclose 返回 0（成功），dlerror 返回 NULL（无错误）。

#### Scenario: dlopen returns NULL via hook

- **WHEN** guest 调用 `dlopen("libfoo.so", RTLD_NOW)`
- **AND** libdl.so 的 dlopen 函数被 trampoline 拦截
- **THEN** guest SHALL 收到返回 NULL（0）
- **AND** SHALL NOT 执行 libdl.so 中的 dlopen ARM64 代码

#### Scenario: dlsym returns NULL via hook

- **WHEN** guest 调用 `dlsym(handle, "func_name")`
- **AND** libdl.so 的 dlsym 函数被 trampoline 拦截
- **THEN** guest SHALL 收到返回 NULL（0）
- **AND** SHALL NOT 执行 libdl.so 中的 dlsym ARM64 代码

#### Scenario: dlclose returns success via hook

- **WHEN** guest 调用 `dlclose(handle)`
- **AND** libdl.so 的 dlclose 函数被 trampoline 拦截
- **THEN** hook SHALL 返回 0（表示成功）
- **AND** SHALL NOT 执行任何卸载操作

#### Scenario: dlerror returns NULL via hook

- **WHEN** guest 调用 `dlerror()`
- **AND** libdl.so 的 dlerror 函数被 trampoline 拦截
- **THEN** hook SHALL 返回 NULL（表示无错误）
- **AND** SHALL NOT 执行 libdl.so 中的 dlerror ARM64 代码

#### Scenario: libdl detection is by soname

- **WHEN** loader 加载一个系统库
- **AND** 该库的 soname 或文件名匹配 `libdl.so`
- **THEN** loader SHALL 在 relocation 完成后安装 trampoline hook
- **AND** SHALL NOT 误匹配其他 .so（如 `libcutils.so` 含 `dl` 子串）

### Requirement: System libraries directory is gitignored

仓库根下的 `resources/android/sdk23/lib64/` 目录中的 `.so` 文件 SHALL 被 `.gitignore` 排除，不提交到版本控制。目录结构自身（含 `.gitignore` 占位）SHALL 提交以保留路径约定。

#### Scenario: System library .so files are not tracked

- **WHEN** 运行 `git status` 检查仓库状态
- **THEN** `resources/android/sdk23/lib64/*.so` SHALL 被 gitignore 规则排除
- **AND** SHALL NOT 出现在 `git add` 的待跟踪文件列表中

### Requirement: System library path derived from workspace root

SystemLibraryResolver SHALL 从 `env!("CARGO_MANIFEST_DIR")` 向上锚定到 workspace root，再拼接 `resources/android/sdk23/lib64/` 得到系统库目录。路径 SHALL 使用 `std::path::PathBuf` 与 `Path::join` 以跨平台兼容（Windows `\`、Unix `/`）。

#### Scenario: Path resolves to resources/android/sdk23/lib64

- **WHEN** SystemLibraryResolver 初始化
- **THEN** 它 SHALL 构造一个路径指向仓库根下的 `resources/android/sdk23/lib64/`
- **AND** 该路径 SHALL 在文件系统上可访问（当系统库 .so 已复制后）

#### Scenario: Windows path separator is supported

- **WHEN** 仓库在 Windows 上运行
- **THEN** 路径 SHALL 使用 `\` 分隔符
- **AND** SHALL 能被 `std::fs::read` 正确打开
