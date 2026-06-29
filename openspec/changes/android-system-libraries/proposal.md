## Why

真实 Android .so 都依赖系统库（libc/libdl/libm/liblog/libz/libc++ 等），通过 DT_NEEDED 声明依赖。当前 rundroid 的 ELF loader/linker 支持 DT_NEEDED 解析，但 resolve 只在已加载模块 + 依赖图中查找符号——没有系统库符号源。这意味着任何真实 Android ELF（如 JNI .so）在加载阶段就会因为 unresolved 符号而失败，根本无法运行。

unidbg 的做法是直接把 Android SDK23 的 9 个系统库 .so 作为 classpath 资源，让它们走和 guest .so 完全相同的 ELF 加载流程：PT_LOAD 映射进 Unicorn、relocation、符号解析。libc 的函数体真实在 Unicorn 里跑；malloc/printf 等最终经过 SVC 到 SyscallHandler。dlopen/dlsym/dlclose/dlerror 则用 hook 拦截，不走仿真。

rundroid 需要同样的能力：提供一份 Android SDK23 arm64 系统库的副本，接入现有 ELF loader/linker 管道，使 guest .so 的 DT_NEEDED 能被解析。

## What Changes

- **复制系统库**：从 unidbg 仓库的 `unidbg-android/src/main/resources/android/sdk23/lib64/` 复制 9 个 .so（libc/libdl/liblog/libm/libz/libcrypto/libssl/libstdcpp/libcpp）到 `resources/android/sdk23/lib64/`，不提交到 git（gitignore）。
- **新增 `SystemLibraryResolver`**：linker resolve DT_NEEDED 时三段回退：current module → DT_NEEDED 依赖图 → SystemLibraryResolver。命中后走 `load_inner` 与 guest .so 相同的加载流程。
- **系统库标记**：加载时标记 `allow_unresolved=true`（unresolved 符号不导致 link 失败），跳过 init_array 执行。
- **libdl hook**：检测到模块是 libdl.so 时，dlopen/dlsym/dlclose/dlerror 用 trampoline hook 拦截，不走仿真执行。第一期 dlopen/dlsym 走 stub（返回空/null），dlclose/dlerror 空操作。
- **路径约定**：系统库目录 `resources/android/sdk23/lib64/`，编译时由 workspace root 推导。

## Capabilities

### New Capabilities

- `android-system-libraries`: 系统库解析与加载契约——`SystemLibraryResolver` 回退查找、系统库 `allow_unresolved` 标记 + 跳过 init_array、libdl hook 拦截、路径约定与资源管理。

### Modified Capabilities

- `dependency-linking`: DT_NEEDED 解析流程新增 SystemLibraryResolver 三段回退（current → deps → system），loader 支持 `allow_unresolved` 标记扩展。核心契约（依赖图驱动、稳定符号解析顺序、SONAME 身份显式）语义不变。

## Impact

- **新增文件**：
  - `resources/android/sdk23/lib64/*.so`（9 个系统库，~3.5MB，gitignore 排除）
  - `resources/android/sdk23/lib64/.gitignore`（`*` 忽略所有 .so，保留目录结构）
  - `emulator/elf/loader/src/system_resolver.rs`（`SystemLibraryResolver` 实现）
- **修改文件**：
  - `emulator/elf/loader/src/lib.rs`：`LoaderConfig` 或 `Loader` 新增 `system_resolver` 字段 + 加载管道接入三段回退
  - `emulator/elf/loader/src/loader.rs`：`load_inner` 或链接过程新增 `allow_unresolved` 标记处理 + init_array 跳过逻辑
  - `emulator/elf/linker/src/lib.rs`：符号解析路径新增 system library 回退
  - `emulator/os/android/src/kernel/mod.rs`：libdl hook 识别与 trampoline 注册（若 hook 逻辑放在 OS 侧）或 `emulator/case-runner/src/`（若放 assembly 层）
  - `.gitignore`：追加 system library 目录
  - `emulator/elf/tests/`：新增系统库加载测试
  - `Cargo.toml` 或相关 crate：若新增公共类型扩展
- **API**：`LoaderConfig` 可选新增 `system_library_dir: Option<PathBuf>`；linker resolve 内部新增回退但对外接口不变。libdl hook 是内部实现细节，不暴露。
- **行为**：DT_NEEDED 含 `libc.so` 等系统库名的 guest .so 加载不再因 unresolved 符号而失败；系统库符号被解析进依赖图，libc 函数在 Unicorn 中真实执行（通过 syscall hook）。
- **测试**：现有 ELF loader/linker 单测必须保持全绿；新增系统库加载端到端测试（加载 libc.so 验证符号可解析）；case-runner 集成测试验证带 DT_NEEDED 的 guest .so 加载成功。
