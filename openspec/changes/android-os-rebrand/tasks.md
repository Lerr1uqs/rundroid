# Implementation Tasks

> 顺序按 design 的 Migration Plan：先命名收敛保编译绿 → 抽 `Syscall` 类型 → `Unimplemented` 变体 + 上层策略 → 宏建全量表 → 全量验证。每组结束都要 `cargo test --workspace` 绿。

## 1. 命名收敛（crate / 目录 / 类型，纯改名保行为不变）

- [ ] 1.1 `git mv emulator/os/linux emulator/os/android`（目录重命名，保留 git 历史）
- [ ] 1.2 `emulator/os/android/Cargo.toml`：`name = "rundroid-linux"` → `rundroid-android`，`description` 改为 Android 表述
- [ ] 1.3 `emulator/os/android/src/**`：类型 `LinuxRuntime` → `Kernel`（`kernel/mod.rs` 定义 + 所有 `impl` 块 + `lib.rs` re-export + 文档注释）；`lib.rs` 顶层 crate 文档 `linux` → `android`
- [ ] 1.4 根 `Cargo.toml`：`[workspace.dependencies]` 与 `members` 里 `rundroid-linux`/`os/linux` → `rundroid-android`/`os/android`（含 `path`）
- [ ] 1.5 `emulator/case-runner`：`Cargo.toml` 依赖名 + `use rundroid_linux` → `rundroid_android`；`runtime.rs`/`case.rs` 里 `LinuxRuntime` → `Kernel`、字段 `linux_inner` → `kernel_inner`、`linux()` → `kernel()`
- [ ] 1.6 `emulator/bindings/python`：`Cargo.toml` 依赖名 + `src/lib.rs` 4 处 `Arc<Mutex<LinuxRuntime>>` → `Kernel`、`use rundroid_linux` → `rundroid_android`
- [ ] 1.7 `emulator/driver/src/builtin/urandom.rs` 与 `emulator/backends/api/src/engine.rs`：注释里 `LinuxRuntime`/`linux` 引用跟随更新
- [ ] 1.8 `tests/cases/02-openat`、`tests/cases/03-dev-urandom` 的 `case.toml`：若有路径/名引用则更新
- [ ] 1.9 `ROADMAP.md`：仓库布局索引 + crate 分层图 + 核心概念地图里 `os/linux`→`os/android`、`LinuxRuntime`→`Kernel`、`rundroid-linux`→`rundroid-android`
- [ ] 1.10 `cargo test --workspace` 全绿（本组只改名，已实现 syscall 行为不变）

## 2. Syscall 类型抽取（`sys_*` 迁到 `impl Syscall`）

- [ ] 2.1 在 `emulator/os/android/src/syscall.rs` 定义 `pub struct Syscall;`（零字段分派器，文档注释说明职责 + 预留 tracing 扩展位）
- [ ] 2.2 `Kernel` 加字段 `pub syscalls: Syscall`，构造 `build()` 内初始化
- [ ] 2.3 `dispatch` + 16 个 `sys_*` handler 从 `impl Kernel` 迁到 `impl Syscall`；签名加 `kernel: &mut Kernel` 首参，OS 操作由 `self.xxx()` 改为 `kernel.xxx()`；`sys_*` 间互调（若有）同步改
- [ ] 2.4 `Kernel::dispatch(&mut self, nr, x0..x5, mem)` 改为一行转发 `Syscall::dispatch(self, nr, x0, x1, x2, x3, x4, x5, mem)`；`syscall.rs` 顶部模块文档更新（ABI 边界承载主体 = `Syscall` 类型）
- [ ] 2.5 `case-runner` `SyscallDispatcher`：`linux.dispatch(...)` → `kernel.dispatch(...)`（裸指针桥接 `SAFETY:` 注释保留，仅类型名跟随）
- [ ] 2.6 Python bindings hook：`dispatch` 调用点类型名跟随
- [ ] 2.7 `os/android/src/syscall.rs` 模块内单测：`rt`/`LinuxRuntime` 变量名 → `kernel`/`Kernel`，`rt.dispatch` → `kernel.dispatch`
- [ ] 2.8 `cargo test --workspace` 全绿（结构重组，行为不变）

## 3. `Unimplemented` 变体 + 上层策略

- [ ] 3.1 `SyscallResult` 新增变体 `Unimplemented { nr: u64, name: &'static str }`
- [ ] 3.2 `Syscall::dispatch` 未实现分支（原 `_ => ENOSYS`）改为返回 `SyscallResult::Unimplemented { nr, name }`（`name` 本步暂用占位，第 4 组宏接入真实名）；完全未知号 `name = "unknown"`
- [ ] 3.3 定义 `UnimplementedPolicy { Panic, Enosys }`（放在 `os/android` 公开导出，供上层引用）
- [ ] 3.4 `case-runner` `SyscallDispatcher` 持 `UnimplementedPolicy`（默认 `Panic`），match `SyscallResult` 时 `Unimplemented` 按策略：`Panic` → `panic!`（消息含 nr+name）；`Enosys` → 降级写回 `ENOSYS` 到 x0
- [ ] 3.5 Python bindings hook 持同样策略 + `Unimplemented` 分支处理
- [ ] 3.6 单测：未实现号 + 默认策略 `#[should_panic]`（消息含 nr/name）；策略 `Enosys` 时降级返回 `ENOSYS`；`Syscall` 层本身只产 `Unimplemented` 不自决（策略无关单测）
- [ ] 3.7 `cargo test --workspace` + case `01-04` 全绿（已实现号不受影响）

## 4. 全量 syscall 号表（`define_android_syscalls!` 宏）

- [ ] 4.1 数据准备：从 `include/uapi/asm-generic/unistd.h` 整理全量 ~460 条 ARM64 Linux/Android syscall `nr → name`（已实现的 16 条对齐现有常量值）
- [ ] 4.2 实现 `define_android_syscalls!`（crate 内 `macro_rules!`）：每条 arm `nr => name => impl <handler>` 或 `nr => name => stub`，展开为 ① syscall 号常量 ② `syscall_name(nr: u64) -> Option<&'static str>` ③ `Syscall::dispatch` 的 match（`impl` arm 调 `Self::sys_*`，`stub` arm 返回 `Unimplemented { nr, name }`，`_` 返回 `name="unknown"`）
- [ ] 4.3 用宏替换第 3 组的临时未实现分支 + 已实现 match：16 已实现标 `impl sys_*`，其余 ~440 标 `stub`
- [ ] 4.4 `syscall_name` 单测：抽样校验已知号（`read`=63/`write`=64/`exit`=93/`exit_group`=94/`openat`=56/`futex`=98/`mmap`=222 等）名正确
- [ ] 4.5 dispatch 全表覆盖单测：取一个 `stub` 号（如 `futex`=98）断言返回 `Unimplemented { nr, name: "futex" }`；取一个号外未知号断言 `name="unknown"`
- [ ] 4.6 `cargo test --workspace` 全绿

## 5. 收尾验证

- [ ] 5.1 `cargo test --workspace` 全量绿（原 syscall 单测含 pread64 四测 + 新 Unimplemented/策略/宏测试）
- [ ] 5.2 `cargo run -p rundroid-cli -- case tests/cases/01-pure-export-call/case.toml` + `02-openat` + `03-dev-urandom` + `04-mmap-rw` 端到端全绿
- [ ] 5.3 `cd python && uv run pytest tests/ -p no:cacheprovider` 全绿（Python 侧 hook 改动回归；teardown 噪声忽略，看 exit 0 + N passed）
- [ ] 5.4 `openspec validate --type change android-os-rebrand --strict`（cwd 仓库根）通过
- [ ] 5.5 `ROADMAP.md` 核心概念地图补 `Syscall` 条目、分层图 `os/android`；同步 memory（`MEMORY.md` 一行指针 + 本 change 要点）
