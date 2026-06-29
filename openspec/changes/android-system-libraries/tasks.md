# Implementation Tasks

> 顺序按 design 的 Migration Plan：复制系统库 + gitignore → SystemLibraryResolver（路径推导 + load 接入）→ allow_unresolved/skip_init 标记 → libdl hook → 测试 + 回归。每组结束都要 `cargo test --workspace` 绿（不依赖真实系统库存在的单测保持绿）。

## 1. 复制系统库 + gitignore

- [ ] 1.1 创建目录 `resources/android/sdk23/lib64/`（如不存在）
- [ ] 1.2 从 `F:/reverse-workspace/unidbg/unidbg-android/src/main/resources/android/sdk23/lib64/` 复制 9 个 .so（libc/libdl/liblog/libm/libz/libcrypto/libssl/libstdcpp/libcpp）到 `resources/android/sdk23/lib64/`
- [ ] 1.3 在 `resources/android/sdk23/lib64/` 创建 `.gitignore`，内容为 `*`（忽略所有文件，仅保留目录结构）
- [ ] 1.4 仓库根 `.gitignore` 追加 `/resources/android/sdk23/lib64/*.so`（确保即使子目录 .gitignore 失效也有兜底规则）
- [ ] 1.5 验证：`git status` 不显示 .so 文件为 untracked；`readelf -h resources/android/sdk23/lib64/libc.so`（MSYS2/Cygwin 有 readelf）确认 ARM64 ELF 格式

## 2. SystemLibraryResolver（路径发现 + 加载接入）

- [ ] 2.1 创建 `emulator/elf/loader/src/system_resolver.rs`：定义 `SystemLibraryResolver` struct，持 `root_path: PathBuf`（系统库目录），构造函数通过 `CARGO_MANIFEST_DIR` 向上锚定 workspace root 拼接 `resources/android/sdk23/lib64/`
- [ ] 2.2 `SystemLibraryResolver` 暴露 `resolve_dependency(&self, soname: &str) -> Option<(PathBuf, Vec<u8>)>`：根据 soname 在目录中查找同名 .so，命中则读出字节；未命中返回 None
- [ ] 2.3 `emulator/elf/loader/src/lib.rs`：`LoaderConfig` 或 `Loader` 新增可选字段 `system_resolver: Option<Arc<SystemLibraryResolver>>`
- [ ] 2.4 `emulator/elf/loader/src/loader.rs`：`Loader::load` 的 DT_NEEDED 解析循环中，当在已加载模块 + 依赖图中未命中时，插入第三步回退——调用 `system_resolver.resolve_dependency(soname)`，命中则用 `load_inner` 装载（与 guest .so 相同流程），装载后标记 `allow_unresolved=true` + `skip_init=true`
- [ ] 2.5 模块引用：`emulator/elf/loader/src/lib.rs` 重新导出 `SystemLibraryResolver`
- [ ] 2.6 单测：构造 mock SystemLibraryResolver（指向临时目录，放一个极小 ARM64 .so 或伪造 ELF），验证三段回退在 guest 未命中时能走到 resolver 并成功加载；验证未命中 resolver 时保持 unresolved
- [ ] 2.7 `cargo test -p rundroid-elf-loader` 绿（新增单测 + 原有单测全保绿）

## 3. allow_unresolved + skip_init 标记

- [ ] 3.1 `emulator/elf/loader/src/loader.rs`：定义 `LoadFlags` bitflags 或 struct，含 `ALLOW_UNRESOLVED` 和 `SKIP_INIT` 两标志，默认全 0
- [ ] 3.2 `Loader::load_inner`（或下层调用）接受 `LoadFlags` 参数；系统库加载时传入 `ALLOW_UNRESOLVED | SKIP_INIT`
- [ ] 3.3 `emulator/elf/linker/src/lib.rs`（linker 层）：在 relocation/init 阶段检查模块的 `LoadFlags`
  - `ALLOW_UNRESOLVED`：遇到 unresolved 符号时不报错，记录到 `Vec<String>` 列表，relocation 地址写 0，继续执行
  - `SKIP_INIT`：init 顺序生成时跳过该模块
- [ ] 3.4 linker 暴露 `unresolved_symbols()` 查询方法返回 unresolved 符号列表
- [ ] 3.5 单测：构造一个有 unresolved 符号的模块（伪造符号表，未在依赖图中提供），验证 `allow_unresolved=true` 时不 panic、relocation 写 0、`unresolved_symbols()` 返回列表；验证 `allow_unresolved=false`（默认）时仍报错
- [ ] 3.6 单测：验证标记 `skip_init=true` 的模块不在 init 顺序中
- [ ] 3.7 `cargo test --workspace` 绿

## 4. libdl hook 拦截

- [ ] 4.1 在 loader（或 linker，取决于架构）中增加 `maybe_install_libdl_hook` 逻辑：当加载的模块 soname 或文件名匹配 `libdl.so` 时，记录该模块基址与 dlopen/dlsym/dlclose/dlerror 四个导出函数的地址
- [ ] 4.2 在 `emulator/elf/loader/src/` 或 `emulator/case-runner/src/` 中实现 `LibDlHook` 或直接把 hook 注册到 backend 的 `install_code_hook`（类似 `install_syscall_hook` 的 `on_code`）
- [ ] 4.3 hook handler 实现：
  - dlopen trampoline：guest 跳入 → hook 拦截 → 返回 NULL（0）
  - dlsym trampoline：guest 跳入 → hook 拦截 → 返回 NULL（0）
  - dlclose trampoline：guest 跳入 → hook 拦截 → 返回 0
  - dlerror trampoline：guest 跳入 → hook 拦截 → 返回 NULL（0）
- [ ] 4.4 hook 安装时机：在 linker relocation 完成后，libdl.so 的 GOT 表已填入最终地址时安装 trampoline（确保 hook 覆盖 dl 函数入口）
- [ ] 4.5 单测：加载真实 libdl.so（sdk23 副本），验证四个 dl 函数基址与符号表匹配；通过 mock backend code hook 验证 trampoline 被触发
- [ ] 4.6 `cargo test --workspace` 绿

## 5. 测试 + 回归验证

- [ ] 5.1 端到端测试：用例加载一个声明 `DT_NEEDED libc.so` 的 mock ELF（或真实的 libsmoke.so），验证 SystemLibraryResolver 能找到并加载 libc.so，链接完成后 libc 符号在 resolve 范围内
- [ ] 5.2 系统库加载单测：单独加载 libc.so（不通过 guest 触发），验证 `allow_unresolved` 工作、init 被跳过、unresolved 符号列表非空（libc 确实有 unresolved 符号）、linker 不 panic
- [ ] 5.3 case-runner 集成测试：`cargo run -p rundroid-cli -- case tests/cases/01-pure-export-call/case.toml` 仍绿（该 case 不依赖 DT_NEEDED，是纯导出调用，检查回归）
- [ ] 5.4 手动验证：`cargo run -p rundroid-cli -- case` 或 Python 端 `cd python && uv run pytest tests/ -p no:cacheprovider` 绿（现有测试不受影响，teardown 噪声忽略）
- [ ] 5.5 `openspec validate --type change android-system-libraries --strict` 通过
- [ ] 5.6 `cargo test --workspace` 全量绿
