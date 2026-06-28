# rundroid ROADMAP

> 给未来 Claude 的项目导航。**memory 清空后，先读这个 + `AGENTS.md` 重建心智模型。**
> 这是一张地图（索引/概念/关系），不是教程。需要细节时按索引跳到具体文件。

## 一句话定位

Rust 重写 unidbg 底层（unicorn 仿真 + android 执行 + linux syscall），Python 做 stub/hook 层，**舍弃 Java**。bootstrap 阶段只做 **ARM64 + Android + Unicorn**。目标是 Rust 提供 core + stub API，打包成 Python ffi，让 Python 脚本写 hook/breakpoint/tracing/补环境。

## 仓库布局（索引）

```
rundroid/
├── AGENTS.md                 # 规则：代码风格 / 职责 / 流程（每会话加载，必读）
├── ROADMAP.md                # 本文件：地图
├── Cargo.toml                # 虚拟 workspace（根无 [package]）
├── .cargo/config.toml        # Windows: PATH 前置 cmake+ninja（给 unicorn-engine 编译）
├── emulator/                 # 所有 Rust crate
│   ├── core/                 # 基础：RuntimeConfig / Arch / BackendKind / IdAllocator / ModuleId
│   ├── telemetry/            # 事件（TelemetryRouter / EventSink / TelemetryEventKind）
│   ├── memory/               # RegionTracker（guest 内存区账本）
│   ├── backends/
│   │   ├── api/              # trait: Backend(工厂)/Engine(会话)/GuestCPU(hook内视图)/SyscallHook/CodeHook + MemPerms + Arm64Reg
│   │   └── unicorn/          # UnicornBackend 实现（svc=intr hook, code hook, mem_protect）
│   ├── elf/
│   │   ├── parser/           # ParsedElf / DynamicInfo(soname/needed/relro) / ElfParseError(BadMagic/Truncated/...)
│   │   ├── loader/           # ElfLoader/LoadContext/LoadedModule(relro) — reserve footprint RWX → 写段 → 收紧权限
│   │   └── linker/           # ElfLinker/LinkContext/ModuleGraph/resolve(BFS依赖闭包)/init(Kahn拓扑)
│   ├── driver/               # VirtualDevice trait / VirtFile(Host/Bytes/Dynamic) / builtin(urandom/random/null/zero)
│   ├── os/linux/             # LinuxRuntime / kernel/* / memory_bridge.rs / fd.rs / vfs.rs
│   ├── jni/                  # JNI 子系统（见下"核心概念"）
│   ├── jni_trampoline/       # 共享 JNI trampoline hook + dispatch（case-runner / Python 绑定共用）
│   ├── case-runner/          # GuestRuntime(装配层) / case / manifest / runtime
│   ├── cli/                  # rundroid-cli: `case <toml>` / `list <dir>`
│   └── bindings/python/      # pyo3 cdylib _rundroid + javashim（Python 端在 python/）
├── python/                   # Python stub 层：rundroid/(avm.py, base.py, values.py) + tests/ + .venv
├── tests/cases/              # case.toml 数据：01-pure-export-call / 02-openat / 03-dev-urandom / 04-mmap-rw
├── resources/                # fixture：smoke/jnitest/src/*.c → build/*.so（gitignore，clone 后 NDK 重编）
├── openspec/                 # change 管理（见"openspec 流程"）
├── project_design/           # 项目设计文档（严禁擅改）
└── lessons/                  # 犯错清单 / 经验
```

## crate 分层与依赖（从底到顶）

```
core ── telemetry ── memory
  │
backend(api) ◄── backend-unicorn        # Engine trait + 实现
  │
elf-parser ◄── elf-loader ◄── elf-linker# ELF 三层，严格分层
  │
driver                                   # 设备/VirtFile 抽象
  │
os/linux ◄───────────────────────────── # LinuxRuntime(syscall/fd/vfs)
  │
jni ◄─────────────────────────────────── # AndroidVM/registry/JNIEnvABI
  │
jni_trampoline                           # 共享 trampoline hook + dispatch
  │
case-runner ◄── cli                      # 装配 + 调度入口
  │
bindings/python (pyo3) → python/         # ffi + stub
```

**关键边界**：`backend` 只定义 trait（不依赖 unicorn）；`jni` 是 `#![forbid(unsafe_code)]` 且**不依赖 backend**（所以 ABI 元数据在 jni crate，解寄存器/dispatch 在 case-runner）；`case-runner` 是唯一同时持有 backend + jni + elf + linux 的装配层。

## 核心概念地图

| 概念 | 位置 | 一句话职责 |
|---|---|---|
| `GuestRuntime` | case-runner/runtime.rs | 装配总览：持 engine + linux + elf graph + jni 表 + regions，实现 LoadContext/LinkContext |
| `Backend`/`Engine`/`GuestCPU` | backends/api | 工厂/会话句柄/hook 内受限视图（GuestCPU 的 mem_* 返回 bool 用于 EFAULT 上报） |
| `LinuxRuntime` | os/linux | OS 状态(vfs/fds/mmap区/brk/rng) + syscall dispatch（**linux-syscall-layering 将拆 kernel/+syscall**） |
| `AndroidVM` | jni/android_vm.rs | JNI 聚合根（registry/objects/refs/exceptions/natives/apk）的最终 authority |
| `JniRegistry` | jni/registry.rs | class/method/field 权威 + dispatch 主线（Rust handler / Python shim / framework stub 统一入口） |
| `JniEnvSurface` | jni/jnienv.rs | host 侧 Rust 分派层（find_class/call_int_method_by_id/...） |
| `JNIEnvABI`/`JavaVMABI` | jni/abi.rs | guest 可见 ABI 表布局 + slot catalog + GetEnv/Attach/Detach 纯逻辑（**不碰目标侧内存**） |
| `JniTrampolineHook` | jni_trampoline/src/lib.rs | CodeHook：trampoline 拦截 → function_index 反算 slot → dispatch_jni_call → JniEnvSurface |
| `ModuleGraph` | elf/linker | 模块表 + deps 边 + by_soname 索引；resolve 按 requester 依赖闭包 BFS |
| `avm` (Python) | python/rundroid/avm.py | JNI/VM 门面：new_object/new_string/call_java_method_typed/find_class |

## 必须理解的 5 条主线（机制）

1. **JNI trampoline + code hook 拦截**：guest `(*env)->Fn(env,...)` → 函数表每格指向 trampoline NOP → backend code hook 在 NOP 执行前回调 → `function_index(addr)=(addr-trampoline_base)/4` 反算 slot → 查 catalog 分流 → dispatch。Rust handler 在 host 跑，**不进 guest**（类比 svc 拦截）。
2. **syscall svc hook + 目标侧回写**：guest `svc #0` → SyscallHook → `LinuxRuntime::dispatch(nr,x0..x5, &mut MemoryBridge)` → handler 调 OS 语义拿数据 → **write/map 回写失败即 EFAULT**（不允许"返回长度但目标缓冲没变"的假成功）。
3. **ELF 装载链**：parser → loader（footprint 整块 RWX reserve → 写段 → 按 p_flags mem_protect 收紧）→ linker（relocation 写回 → resolve 按依赖图 → RELRO mem_protect 只读）→ case-runner 自动检测 `JNI_OnLoad`；Python 绑定层显式 `jni_onload()`。
4. **ABI slot catalog 驱动 dispatch**：`JNIEnvABI`/`JavaVMABI` 持静态 `JniSlotSpec{name,offset,handler:Bridge|Unimplemented}` catalog，dispatch 查 catalog 决定放行/fail-fast/telemetry（声明式 catalog，handler 实现因依赖 backend 留装配层）。
5. **Python↔Rust**：pyo3 cdylib `_rundroid` + Python `javashim`（`JavaClass`/`JavaObject`/`avm`）。`avm` 是 JNI/VM 唯一门面；marshalling 单一规则源（`py_to_jvalue`/`jvalue_to_py`）；guest JNI 经 trampoline hook 回调 Python override。死锁规避：锁内不进 Python、绑定层与 hook 共享同一个 `Arc<Mutex<AndroidVM>>`。

## openspec 流程

- **change 目录**：`openspec/changes/<change>/{proposal.md, design.md, specs/<cap>/spec.md, tasks.md}`。
- **spec-driven schema**：proposal → (design + specs) → tasks；`applyRequires: tasks`。
- **技能**：`/openspec-propose`（新建 change 全套 artifact）、`/openspec-apply-change`（按 tasks 实现）、`/openspec-archive-change`（归档）。
- **校验**：`openspec validate --type change <name> --strict`（cwd 必须在仓库根）。
- **归档**：archive 后 spec 进 `openspec/specs/<cap>/` 成为权威。
- **关键习惯**：design 过时是常见的（change 创建早、代码演进晚）——apply 前先对照 spec（SHALL 约束）而非 design（建议），过时建议要 pause 对齐。

## 构建 / 测试 / 运行

```bash
# Rust 全量测试
cargo test --workspace

# 跑 case（必须 cd 仓库根！working dir 漂移会让 case.toml 相对路径失败）
cargo run -p rundroid-cli -- case tests/cases/04-mmap-rw/case.toml

# 编译 fixture（NDK；命令也写在 src/*.c 文件头）
cd resources/smoke && /f/android-ndk/toolchains/llvm/prebuilt/windows-x86_64/bin/aarch64-linux-android21-clang -shared -fPIC -O2 -o build/libsmoke.so src/smoke.c

# Python（改了 Rust 绑定必须重跑 maturin develop）
cd python && uv run pytest tests/ -p no:cacheprovider
cd python && source .venv/Scripts/activate && maturin develop
```

**Windows 工具链坑**：unicorn-engine 2.x 需 cmake+ninja（`.cargo/config.toml` 前置 PATH）；**不要** `CC=clang`（链接 pthread.lib 失败）。Cargo 走 rsproxy.cn 镜像。

## 代码风格（AGENTS.md 摘要，强制）

- **中文注释**：函数注释 + 复杂算法 + 特殊 case 说明。
- **高内聚低耦合 OOP**；**禁 `get_xxx`**，直接 `xxx()`；链式 builder（`XxxBuilder().set().build()` / `Xxx::build()`）。
- **首字母缩写类名全大写**：`CPU`/`JNI`/`ARM`/`ABI`/`JavaVMABI`（不用 `Cpu`/`JavaVmAbi`）。
- **fail-fast**：不写大量兜底，未覆盖 case 直接 panic/Err，方便调试。
- **ut/harness 必须注释标注**；Python 不用 `Any`，都要 typing。
- **写完必须跑通**，不允许写完就不管。


## 常见坑

- **working dir 漂移**：bash 工具记 cwd，`cd` 进子目录后持久化。每条涉及相对路径的命令前 `cd /f/coding-workspace/rundroid && ...`。
- **CRLF warning**：Windows 上 git 自动转换，无害。
- **`resources/*/build/*.so` gitignore**：clone 后必须 NDK 重编（命令在 src 文件头），否则 case 跑不了。
- **borrow 绕过**：`LinkCtxAdapter`/`SyscallDispatcher` 用裸指针绕"重叠 mut 借用"误报，有 `SAFETY:` 注释；change 应减少而非扩大 unsafe 边界。
- **死锁**：JNI trampoline hook 持 `Arc<Mutex<AndroidVM>>`，锁内**不进 Python**；Python override 在 guest JNI dispatch 期间不得 re-enter VM / engine。
- **pytest teardown 噪声**：进程退出时 "Windows fatal exception: access violation" 栈是 unicorn/pyo3 teardown 噪声，`pytest exit 0` + `N passed` 即真绿。

## 快速上手：读这 7 个文件进状态

1. `AGENTS.md` — 规则（每会话加载）
2. 本 `ROADMAP.md` — 地图
3. `emulator/case-runner/src/runtime.rs` — 装配总览（GuestRuntime 串联 backend/elf/jni/linux）
4. `emulator/jni/src/lib.rs` + `abi.rs` — JNI 子系统全貌 + ABI 表面
5. `emulator/jni_trampoline/src/lib.rs` — trampoline 拦截 → dispatch 链路
6. `emulator/os/linux/src/syscall.rs` — syscall dispatch（linux-syscall-layering 将拆）
7. `openspec/changes/` — change 历史（看最近几个 change 的 design/specs/tasks 了解演进）
