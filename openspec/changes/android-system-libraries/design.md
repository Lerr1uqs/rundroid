## Context

当前 rundroid 的 ELF 加载链路（parse → load → link）完整支持 DT_NEEDED 驱动的依赖图解析。现状：

- `emulator/elf/parser/`：将 ELF 字节解析为不可变视图，含段/动态表/符号表/重定位。
- `emulator/elf/loader/`：单模块装载与目标内存布局（PT_LOAD 映射、字节写入、零填充、段权限）。
- `emulator/elf/linker/`：依赖图符号解析 + relocation 写回 + init 计划。
- `emulator/elf/loader/src/api.rs` / `loader.rs`：`Loader::load` 作为根入口，负责 serial/parallel 依赖装载。
- `dependency-linking` 权威 spec 规定 DT_NEEDED 驱动、稳定符号解析顺序（requester → deps → 依赖闭包）、SONAME 身份显式。

关键缺口：resolve 只查已加载模块 + 依赖图，未提供任何系统库符号源。guest .so 声明 `DT_NEEDED libc.so` 时直接 unresolved → fail。

unidbg 做法：AndroidResolver 作为全局符号源——加载时把 sdk23 系统库 .so 作为普通 ELF 完全加载（PT_LOAD / relocation 全走），libc 代码在 Unicorn 中真实运行。dl* 函数用 hook 拦截。本例照此路线。

本次 change 在现有分层上新增 `SystemLibraryResolver` 作为三段回退末段，不破坏现有 resolve 逻辑；loader 扩展 `allow_unresolved` + skip init_array 标记；libdl 以 hook 拦截。

## Goals / Non-Goals

**Goals:**

- 提供 Android SDK23 arm64 系统库集合（9 个 .so），git 不跟踪。
- 新增 `SystemLibraryResolver`，使 linker 在 DT_NEEDED resolve 时能回退到系统库目录（current → deps → system）。
- 系统库加载时 `allow_unresolved`（unresolved 符号不导致链接失败）并跳过 init_array 执行。
- libdl.so 的 dlopen/dlsym/dlclose/dlerror 用 hook 拦截，不走仿真（第一期 stub 实现：dlopen/dlsym→NULL，dlclose/dlerror→空操作）。
- 现有 ELF loader/linker 单测和集成测试全绿。

**Non-Goals:**

- 不引入新的 Android SDK 版本（只支持 sdk23，与 unidbg 一致）。
- 不实现 libdl hook 的完整语义（dlopen 真加载、dlsym 真查符号）——第一期 stub。
- 不修改 `dependency-linking` 的核心契约（依赖图驱动、稳定符号解析顺序、SONAME 身份显式）——仅扩展其 DT_NEEDED 解析流程以包含系统库回退。
- 不系统性地实现 libc 中的 stub syscall（那是 `android-syscall-surface` 后续补实现的任务）。
- 不改 backend / jni / case-runner 的顶层结构。
- 不处理 TLS 模块（`PT_TLS` 系统库中已有但当前返回 `None`，spec 边界允许的遗留）。

## Decisions

### Decision 1: 真实 ELF 仿真路线（不是 stub 替换）

系统库以真实 ELF .so 文件的形式存在，通过现有 parser/loader/linker 管道与 guest .so 完全相同的流程加载：PT_LOAD 映射进 Unicorn、relocation、符号导出供 guest resolve。libc 的函数体在 Unicorn 中真实执行，malloc/printf/etc 最终经过 SVC 到 SyscallHandler。

- **Rationale**：与 unidbg 一致；无需为每个 libc 函数编写 stub；内存布局、符号地址、函数签名天然精确匹配实际 Android 系统库。
- **Alternatives**：(a) Rust "stub library" 链接 crate——需要为几百个 libc 函数维护签名，工作量大且容易偏离真实行为；(b) 用 trampoline hook 全部拦截——与源码直接跑路线的"代码真跑"目标矛盾。均不取。

### Decision 2: SystemLibraryResolver 作为三段回退末段

linker resolve 符号时，查找顺序严格为：requester 自身导出表 → requester 的 DT_NEEDED 依赖图（现有 `dependency-linking` 规范） → `SystemLibraryResolver`。第三步查系统库目录中所有已加载模块的导出表。

- **Rationale**：不侵入现有 resolve 逻辑；系统库符号优先级低于 guest 显式依赖，避免意外覆盖 guest 自定义符号；SystemLibraryResolver 是"数据"（知道哪些 .so 在目录里），不持有 resolve 逻辑——resolve 逻辑仍在 linker 中，system_resolver 只是"如果这个 DT_NEEDED 未被解析，查一下目录是否有同名 .so 并加载它"。
- **实现**：`SystemLibraryResolver` 持系统库目录路径，提供 `resolve_dependency(&self, soname: &str) -> Option<ParsedElf>`。命中后由外部 loader 以 `load_inner` 加载（与 guest .so 相同流程），加载后打 `allow_unresolved` + `skip_init` 标记。
- **Alternatives**：(a) 系统库在 loader 初始化时预加载——与 lazy resolution 原则冲突，且增加了不必要的启动开销；(b) 系统库作为 linker 内部 fallback 链——增加 linker 对 filesystem 的依赖，污染纯逻辑层。均不取。

### Decision 3: allow_unresolved + skip init_array 由标记控制

系统库加载时标记 `allow_unresolved=true`，linker 解析遇到 unresolved 符号时跳过该模块不 panic（记录到 unresolved 列表供诊断）；`skip_init=true` 表示 init_array 不执行。

- **Rationale**：系统库互相依赖（libc.so 依赖 libdl.so 等），`allow_unresolved` 容忍跨系统库弱符号未解析（如弱符号落地为地址 0）；跳过 init_array 是因为系统库 init 函数在非完整 Android 环境中可能依赖未初始化的其他系统服务。
- **实现**：`LoaderConfig` 新增 `system_library_dir: Option<PathBuf>`；`ModuleRecord` 或加载参数新增 `flags: LoadFlags { allow_unresolved: bool, skip_init: bool }`。自定义模块（guest .so）默认 flags 全 false；系统库模块自动设 true。
- **Alternatives**：系统库与 guest .so 完全同权——若 unresolved 则 panic——但 libc 等系统库本身依赖 Android linker 未提供的符号（如 `__cxa_finalize`），不允许 unresolved 则加载必失败。不取。

### Decision 4: libdl hook 用 trampoline 拦截（不仿真执行）

检测到模块名或 soname 为 `libdl.so` 时，其 dlopen/dlsym/dlclose/dlerror 四个导出函数在 relocation 完成后，函数入口被替换为 trampoline（跳转到 Rust handler，类似 `install_syscall_hook` 的 `on_code` 机制）。第一期实现为 stub：dlopen/dlsym→返回 NULL，dlclose/dlerror→空操作成功。

- **Rationale**：libdl 的函数语义严重依赖 Android linker 内部状态（soinfo 链表、符号缓存），仿真执行 libdl.so 中的 ARM64 代码会反复触发 linker 内部的未实现路径或死循环；hook 拦截绕过这些内部状态依赖。
- **Alternatives**：(a) 不拦截 libdl——guest 调 dlopen/dlsym 走 libdl 代码 → 读 soinfo 全局链表 → segfault（无真实 linker）；(b) 实现完整 dlopen 加载逻辑——本 change 范围不足（第一期不实现真加载）。均不取。hook stub 是可扩展基础，后续 change 可把 stub 改为定向加载。

### Decision 5: 系统库路径由 workspace root 推导，不做 compile-time 嵌入

系统库目录 `resources/android/sdk23/lib64/` 在仓库根下。`SystemLibraryResolver` 的路径通过 `env!("CARGO_MANIFEST_DIR")` 向上锚定到 workspace root 拼接 `resources/android/sdk23/lib64/` 得到。不做 `include_bytes!` 嵌入（~3.5MB，不必要地增大二进制）。

- **Rationale**：case-runner 和测试均从 workspace root 运行，推导路径可靠；gitignore 排除 .so 文件，不增加仓库体积；运行时文件 I/O 读取 .so 字节，与 guest .so 加载路径一致。
- **Alternatives**：运行时环境变量/配置路径——增加配置复杂度。编译时嵌入——无必要增大二进制。均不取。

## Risks / Trade-offs

- **[系统库与 host 架构不匹配]** → SDK23 lib64 是 ARM64 ELF，在 x86_64 host 上测试时 unicorn 能仿真 ARM64 代码，ELF parser 能解析；但需要确保测试环境确认 `.so` 确实是 valid ARM64 ELF（`readelf -h` 验证）。
- **[libc real code 触发新 syscall]** → libc 代码在 Unicorn 中真跑意味着会触发当前未实现的 syscall（如 `futex`、`clock_gettime`、`set_tid_address`），触发 `Unimplemented` 默认 panic。第一期必须配 `UnimplementedPolicy::Enosys` 或预知哪些 syscall 必须实现。tasks 中明确标注"libc 加载单测配 Enosys 策略，待 syscall 补实现后再切回 Panic"。
- **[`dependency-linking` spec 修改]** → 仅有扩展（resolve 新增回退段 + loader 标记），无核心契约变更。现有测试全部保持有效。
- **[libdl hook 精度]** → 按模块 soname 匹配 `libdl.so`，确保不误拦截其他 .so 的同名函数。
- **[Trade-off: 9 个系统库 vs 最小集]** → 9 个 .so 全部复制（~3.5MB），与 unidbg 保持一致。若后续仅需 libc/libdl/libm，可裁剪但本 change 不裁剪。
- **[Windows 路径兼容]** → `SystemLibraryResolver` 路径推导使用 `std::path::PathBuf`，跨平台兼容（Windows `\`、Unix `/`）。路径拼接用 `Path::join` 而非字符串拼接。

## Migration Plan

- 无状态迁移，纯新增 + 回退扩展。
- 顺序：复制系统库 + gitignore → `SystemLibraryResolver`（路径推导 + load 接入）→ `allow_unresolved`/`skip_init` 标记 → libdl hook → 测试 + 回归。
- 单 change 原子提交；回滚 = `git revert`。

## Open Questions

- libdl hook 第一期 stub（返回 NULL）是否满足 smoke case（已不依赖 dlopen 的 case）？预期满足——只有使用 JNI_OnLoad 的 .so 需要 dlopen（已在 case-runner 自己的 JNI hook 中处理），smoke 类纯导出 case 用不到。
- 系统库 unresolved 符号列表是否需要暴露给外部诊断/telemetry？本期可以记录到 `loader: emit("system_library.unresolved", ...)` 事件（走现有 telemetry 基础设施），但不强制上层消费。
