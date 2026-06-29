## 1. Zygote struct 与 crate 创建

- [ ] 1.1 在 `emulator/os/android/` 新建 crate（`rundroid-android` 或子 crate），Cargo.toml 仅依赖 `rundroid-memory`、`rundroid-core` 与 `rundroid-backend-api`
- [ ] 1.2 在 `emulator/os/android/src/zygote.rs` 中定义 `Zygote` struct，包含：
  - 对 Engine（或窄 Materialize 接口）的引用
  - 对 Kernel（LinuxRuntime）的引用
  - stack_base / tls_size 等配置字段
  - `bootstrap()` 方法作为唯一入口
- [ ] 1.3 补 Zygote struct 的基本构造单测

## 2. auxv 构造

- [ ] 2.1 在 `Zygote` 中实现 `build_auxv()` 内部方法，在栈上写 auxv 序列：
  - AT_RANDOM(25) + 指向 16B 随机数的指针
  - AT_PAGESZ(6) + 0x1000
  - AT_NULL(0, 0) 终止
- [ ] 2.2 实现随机数生成：从 `LinuxRuntime::rng_seed` 或使用简单 PRNG 生成 16B 随机数，确保每次 bootstrap 不同
- [ ] 2.3 补单测：验证 auxv 内存布局正确（type/value 顺序、AT_NULL 终止、地址指向有效随机数）

## 3. main thread TLS

- [ ] 3.1 在 `Zygote` 中实现 `setup_tls()`，在栈上分配 TLS 块（至少 512B，8B 对齐）
- [ ] 3.2 填充最小 pthread_internal_t：errno=0, tid=1
- [ ] 3.3 写入 TPIDR_EL0：通过 `Engine::reg_write(Arm64Reg::TpidrEl0, tls_addr)` 或 `msr` 指令执行
  - 若 Arm64Reg 枚举缺少 `TpidrEl0`，先在 engine trait 增加该变体
- [ ] 3.4 补单测：验证 TPIDR_EL0 写入后 guest 读回正确地址（如果用 mock engine，验证 reg_write 调用参数）

## 4. stack 布局

- [ ] 4.1 在 `Zygote` 中实现 `layout_stack()`，在栈顶排布：
  - `argc = 0`（8B）
  - `argv` NULL 终止（空）
  - `envp` NULL 终止（空）
  - auxv 序列（复用 2.1 的结果）
- [ ] 4.2 SP 设置为栈顶（argc 地址）
- [ ] 4.3 补单测：验证栈顶内容布局正确性

## 5. constructor 执行

- [ ] 5.1 在 `Zygote` 中实现 `run_constructors(modules: &[LoadedModule], init_order: &[ModuleId])`：
  - 按 `init_order` 遍历，对每个 module 执行 `InitPlan`
  - `legacy_init(DT_INIT)`：读地址 → `emu_start` 零参数调用
  - `init_array`：遍历每个 slot addr → `mem_read` 读函数指针 → 逐个 `emu_start`
- [ ] 5.2 错误处理：constructor 失败（`emu_start` 返回 error）立即上抛，不跳过
- [ ] 5.3 补单测：
  - DT_INIT 执行（mock 或真实函数）
  - init_array 多条目按序执行
  - 多个 module 按拓扑序执行
  - constructor 失败传播

## 6. 集成到 case-runner

- [ ] 6.1 改造 `emulator/case-runner/src/runtime.rs` 的 `load_and_link`：
  - relocation + link 完成后 → 构造 Zygote → 调用 `zygote.bootstrap(modules, init_order)` → 然后 `detect_jni_onload`
- [ ] 6.2 调整 `call_export`：确认已有 SP/TLS/auxv 上下文状态正确（不覆盖 Zygote 的初始化结果）
- [ ] 6.3 将 `emulator/os/android/` crate 加入工作区 `Cargo.toml`

## 7. 测试与验证

- [ ] 7.1 扩展现有 case 1（smoke）：验证纯导出调用在 Zygote 初始化后仍正常工作（constructor 不影响导出）
- [ ] 7.2 新增 case：带 constructor 的 .so（编译 fixture），验证 constructor 执行 + 后续 JNI_OnLoad 正常
- [ ] 7.3 新增 case：constructor 依赖 auxv/TLS（如 `__stack_chk_fail` 不触发），验证 stack canary 读取正常
- [ ] 7.4 运行 `cargo test --workspace`，所有测试通过
- [ ] 7.5 运行 `openspec validate --type change android-process-bootstrap --strict`，所有 artifact 验证通过
