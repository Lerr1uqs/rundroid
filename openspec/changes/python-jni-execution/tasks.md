## 阶段 1：共享 trampoline 基建

- [x] 根 `Cargo.toml` 加 workspace member `emulator/jni_trampoline` + `[workspace.dependencies]` 加 `rundroid-jni-trampoline`
- [x] 抽 `rundroid-jni-trampoline` 共享 crate（`JniTrampolineHook` + `dispatch_jni_call` + helpers + verbose `Arc<AtomicBool>`），`#![forbid(unsafe_code)]`
- [x] case-runner re-export `rundroid_jni_trampoline as jni_hook` + 改 import + `init_jni` 传 verbose；`cargo test -p rundroid-case-runner`（`jni_function_table_test` 绿）

## 阶段 2：Python 最小 JNI 执行闭环

- [x] 绑定层 `lib.rs`：引入共享 VM 句柄（`Arc<Mutex<AndroidVM>>`）与 `jni_verbose` / `jni_env_ptr` / `jni_vm_ptr` 缓存，暴露 `init_jni` / `jni_env_pointer` / `java_vm_pointer` / `read_guest` / `jni_onload` / `set_jni_verbose`
- [x] 补绑定层 API 注释与文档，明确 guest JNI dispatch 期间 Python override 不得 re-enter VM / engine
- [x] 写一个最小 JNI fixture 与 Python 端到端测试，至少覆盖 `JNI_OnLoad`、`FindClass`、`GetMethodID`、`NewObject`、`CallIntMethod`
- [x] `cd python && source .venv/Scripts/activate && maturin develop`；最小 JNI fixture 对应的 `uv run pytest` 跑绿

## 阶段 3：VM authority 收敛

- [x] jni crate 删 `AndroidRuntime`（`android_runtime.rs` + lib.rs 的 `mod`/`pub use`）；消费方（`framework/registry.rs`、`framework/context.rs`、`core/emulator.rs`、`tests/framework_harness.rs`、`bindings/python/src/lib.rs`）改直持 `AndroidVM`
- [x] 迁移 `AndroidRuntime` 既有测试到 `android_vm.rs` 或等价位置；`cargo test -p rundroid-jni` 全绿
- [x] Python 现有测试回归不破：`cd python && source .venv/Scripts/activate && maturin develop` 后现有 `uv run pytest` 通过

## 阶段 4：rich scene 集成压测

- [x] 写 `resources/scene/src/scene.c` + NDK 编译出 `libscene.so`
- [x] 写 `python/tests/test_native_scene.py`，覆盖 `RegisterNatives`、跨 class 调用、继承、syscall、verbose trace；`uv run pytest` 跑绿

## 阶段 5：规范校验

- [x] `openspec validate --type change python-jni-execution --strict` 通过
