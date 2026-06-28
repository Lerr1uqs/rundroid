# Design: python-jni-execution

## 背景：Python 绑定层的 JNI 能力缺口

case-runner（Rust-only 装配层）早已跑通完整 JNI guest 执行：`jni_hook.rs` 的
`JniTrampolineHook` 实现 `CodeHook`，在 trampoline 触发时分派 `(*env)->FindClass` /
`Call*Method` 等到 `JniEnvSurface`；`runtime.rs::init_jni` 把 JNIEnv + JavaVM ABI 表
映射进 guest 内存并安装 hook（见 `jni_function_table_test` 的 8 个端到端 case）。

但 Python 绑定层（`PyEmulatorBridge`）只有 `emu.call(纯导出)`：能跑 libsmoke 的算术
/ syscall，guest 一旦走 JNI 函数表回调就崩——没映射 ABI 表、没装 hook、没有
`init_jni` / `jni_env_pointer` / `jni_onload`。本 change 把这套能力桥到 Python。

## 架构决策：方案 D（移除 AndroidRuntime 套壳）

`JniTrampolineHook` 钳死要 `Arc<Mutex<AndroidVM>>`：hook 是 `Box<dyn CodeHook>`，
存在 engine 里，在 `emu_start` 期间触发，必须捕获一个能跨任何 `&self` 借用存活的
VM 句柄 → Arc（共享所有权）；dispatch 会改 VM（NewObject / RegisterNatives）→ Mutex
（可变）。

要让绑定层与 hook **共享同一个 VM**，VM 必须抬升成 `Arc<Mutex<AndroidVM>>`。而
`AndroidRuntime` 当前是纯转发空壳：

```rust
pub struct AndroidRuntime { pub vm: AndroidVM }
// 100% 纯委托：classes()/refs()/objects()/object_id_alloc()/register_class()/...
```

`AndroidVM` 自带 `with_apk` / `Default` / 共享 Arc 字段，并自称"唯一 VM authority"。
套壳零状态、零行为，唯一消费者是绑定层——直接用 `AndroidVM` 顶替。

### 纠正一个早先的误判

曾考虑方案 C（保留 `RwLock<AndroidRuntime>`，把 `runtime.vm` 改成
`Arc<Mutex<AndroidVM>>`），理由是"保 RwLock 重入更安全"。**该理由不成立**：VM 一旦
是 `Arc<Mutex<AndroidVM>>`，inner Mutex 才是非重入瓶颈，外层 RwLock 纯装饰。C 与 D
重入特征**完全相同**，D 还省掉 jni crate churn。故选 D。

### AndroidRuntime 消费面（删除影响范围）

`AndroidRuntime` 不只是绑定层在用，删除时这些消费者一并改直持 `AndroidVM`（字段全
pub，机械迁移）：

- `emulator/jni/src/android_runtime.rs`（删除本体 + 5 个测试迁到 `android_vm.rs`）
- `emulator/jni/src/lib.rs`（删 `mod` / `pub use`）
- `emulator/jni/src/framework/registry.rs`（`install` / `new_stub_instance` /
  `new_signature` 的 `rt: &mut AndroidRuntime` → `vm: &mut AndroidVM`，`rt.classes_mut()`
  → `vm.classes` 等，+ 5 个测试）
- `emulator/jni/src/framework/context.rs`（`FrameworkCtx::new` 的来源注释）
- `emulator/jni/tests/framework_harness.rs`（helper 与 case 改 `AndroidVM`）
- `emulator/core/src/emulator.rs`（`Emulator.android: AndroidRuntime` → `AndroidVM`）
- `emulator/bindings/python/src/lib.rs`（绑定层，见下）

## 分层

1. **新 crate `rundroid-jni-trampoline`**（`emulator/jni_trampoline/`）—— 从
   `case-runner/src/jni_hook.rs` 原样抽 `JniTrampolineHook` + `dispatch_jni_call` +
   `read_cstr_from_guest` / `read_u64_from_guest` / `read_varargs`。加 `verbose:
   Arc<AtomicBool>` 共享开关（消费方可在 hook 安装后 toggle）。依赖
   `rundroid-jni` + `rundroid-backend` + `rundroid-telemetry`，`#![forbid(unsafe_code)]`。
2. **case-runner** —— `pub use rundroid_jni_trampoline as jni_hook;` re-export；
   `runtime.rs` 改 import。`init_jni` 内构造 `Arc::new(AtomicBool::new(false))` 传 hook。
3. **jni crate 删 `AndroidRuntime`** —— 见上节消费面。迁移后 `cargo test -p rundroid-jni`
   全绿（原 138 + harness 测试等价迁移）。
4. **根 `Cargo.toml`** —— 加 `emulator/jni_trampoline` member + `[workspace.dependencies]`
   `rundroid-jni-trampoline`。
5. **绑定层 `lib.rs`** —— `runtime: RwLock<AndroidRuntime>` → `vm: Arc<Mutex<AndroidVM>>`；
   构造函数建 `Arc::new(Mutex::new(AndroidVM::new()))`；~15 处 VM 访问点改
   `self.vm.lock().unwrap().X`（一层锁，比 C 更短）；保留"释放 guard 再进 Python"纪律；
   `shim.objects` / `id_alloc` 从 `Arc::clone(&guard.objects)` / `guard.object_id_alloc`
   取（构造时一次性 clone）。新增字段 `jni_verbose: Arc<AtomicBool>` + `jni_env_ptr` /
   `jni_vm_ptr` 缓存。新增方法：
   - `init_jni()`：映射 JNIEnv + JavaVM ABI 表、安装 `JniTrampolineHook`（传
     `Arc::clone(&self.jni_verbose)` 与 `Arc::clone(&self.vm)`），缓存 env/vm 指针。
   - `jni_env_pointer()` / `java_vm_pointer()`：返回 guest 指针。
   - `jni_onload()`：遍历已装载模块，调 `JNI_OnLoad(java_vm, 0)`，校验返回 version。
   - `read_guest(addr, len)`：读 guest 字节（测试校验缓冲）。
   - `set_jni_verbose(bool)`：`self.jni_verbose.store(...)`。

## 实施顺序（先最小闭环，再做清理）

这个 change 的实现顺序不应按"重构体量"排，而应按"验收闭环最短路径"排：

1. **先抽共享 trampoline crate 并保持 case-runner 不回退**。这一步只做复用基建，风险局部。
2. **再打通 Python 最小 JNI execution surface**：`init_jni`、`jni_env_pointer` /
   `java_vm_pointer`、`jni_onload`、`read_guest`、`set_jni_verbose`。此时先用一个最小
   fixture 验证 `JNI_OnLoad` → `FindClass` → `GetMethodID` → `NewObject` →
   `CallIntMethod` 这条主链，确保问题定位集中在 surface 本身，而不是复杂场景噪声。
3. **随后收敛 VM authority**：删除 `AndroidRuntime` 包装层，让绑定层、framework 和
   trampoline hook 都直持同一个 `AndroidVM`。
4. **最后上 rich scene**：`libscene.so` 负责压测继承、跨 class 调用、syscall、
   marshalling 和 verbose trace 的交叉正确性。

## verbose trace

`dispatch_env` 已在 `JNIEnvABI::slot_spec(index)` 拿到 slot 名；各 arm 已解码关键值
（FindClass 的 name、GetMethodID 的 name+sig、CallIntMethod 的 obj/mid 等）。verbose
开时 `println!` 一行 unidbg 式（执行期打 host stdout，pytest `capsys` 可捕获断言）。
trace 的 detail 由 `dispatch_jni_call` 经一个 `&mut JniTrace` 写入，`dispatch_env` 末尾
统一拼 `[I] JNIEnv->FindClass(name="...") => 0x...`。

## 重入约束（内在限制）

guest JNI dispatch 在 `emu_start` 期间触发；触发到 Python `@java_method` override 时，
该 override **不得**再入 VM（`avm.new_object` / `emulator.call`）——否则与 hook 持守的
VM Mutex 自锁死锁。这是单线程仿真的内在限制（同 unidbg），绑定层文档必须明确标注。
测试 fixture 中的 Python override 只允许纯计算（读字段、算返回值），不依赖 dispatch
期间的 VM re-entry。

## Fixture 分层

### 最小 fixture

最小 fixture 只服务于"Python JNI execution surface 是否打通"这个问题。它应尽量小，
只覆盖：

- `JNI_OnLoad`
- `FindClass`
- `GetMethodID`
- `NewObject`
- `CallIntMethod`

该 fixture 的价值是把故障面收窄到 ABI 表映射、hook 安装、env/vm 指针传递和基础
dispatch；一旦它不绿，就不应继续调 rich scene。

## Fixture：`libscene.so`（`resources/scene/src/scene.c`）

签名 / 授权校验 native 模块（经典逆向场景）：

- `JNI_OnLoad(JavaVM*)`：`GetEnv` → `RegisterNatives` 注册 `verifyNative`。
- `Java_com_scene_Native_run(JNIEnv*, jint)`：`FindClass("com/scene/Signer")` →
  `GetMethodID` → `NewObject` → `CallIntMethod(hash)`；交叉调 `com/scene/Crypto` 的
  static `mix`；syscall（`getrandom` / `openat`+`read` / `mmap`）；返回 checksum。
- 继承：`com/scene/Verifier extends com/scene/Signer`（测超类 method 解析）。
- static + instance 混用、primitive 参数 marshalling、交叉依赖（Signer→Crypto、Verifier→Signer）。

编译命令写在文件头：
`aarch64-linux-android21-clang -shared -fPIC -O2 -o libscene.so scene.c`

## Python 测试：`python/tests/test_native_scene.py`

注册 `com/scene/{Signer, Verifier, Crypto}` 为 Python shim（部分真实 Python 逻辑如
hashCode = Java 31-multiplier，部分 framework stub 测 override 优先级）。流程：
`Emulator(...)` → `register_java_class ×N` → `load("scene", bytes)` → `init_jni()` →
`set_jni_verbose(True)` → `jni_onload()` → `call("Java_com_scene_Native_run", env_ptr, input)` →
断言返回值。`capsys` 捕获 verbose，断言 `FindClass` / `GetMethodID` / `CallIntMethod` /
`RegisterNatives` trace 出现。
