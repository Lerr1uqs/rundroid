# rundroid 上下文传递：下一阶段工作

## 已完成的两个 change（刚 archive）

### 1. android-vm-state-model（已 archive）
建立了 Rust 侧 JNI 状态骨架：
- `JniRegistry` — class/method/field 注册表，`register_or_merge_class` 实现 Python override > framework stub 优先级
- `ObjectStore` — `ObjectId → (class_name, ObjectStorage)` 分层对象存储（String/Wrapper/Array/StubInstance/HostValue）
- `RefTable` — `handle(u32) → (ObjectId, RefKind)` 引用表，区分 Local/Global/Weak
- `AndroidVM` — 聚合以上三者的 VM 状态容器
- `AndroidRuntime` — Emulator 持有的高级整合点（`pub vm: AndroidVM`）
- `dispatch.rs` — `dispatch_call` / `dispatch_static` / `dispatch_field_get` 等统一分发函数
- `JniEnvSurface` / `JavaVMSurface` — 最小 JNI surface 骨架（当前未被 case-runner 使用）

### 2. object-model-bridging（本次 change，刚 archive）
将 Python 对象实例接入 ObjectStore + RefTable：
- `PyEmulator` → `PyEmulatorBridge`（`#[pyclass(name = "Emulator")]` 不变）
- 新增 `PythonShimAdapter`：`class_types` + `method_names` adapter cache（非 authority）
- `new_java_instance`：Python 实例化 → `ObjectStore::insert(HostValue { data: Box::new(py_obj) })` → `RefTable::new_global(object_id)` → 返回 handle
- `call_java_method`：`RefTable::resolve(handle)` → `ObjectStore::storage(ObjectId)` → HostValue → Python 直调 / method 未命中 → Rust dispatch 回落 / StubInstance → 直接报错
- `release_java_instance`：`ObjectStore::remove(ObjectId)` + `RefTable::delete_global(handle)`
- `java_instance` / `read_instance_field`：全部通过 ObjectStore 查找
- `register_framework_stub`：改用 `register_or_merge_class`

---

## 当前架构全景

```
┌─────────────────────────────────────────────────────────────────┐
│                    Python 侧（rundroid/）                        │
│  @java_class / @java_method / @java_field decorator              │
│  register(emu, [MyShim]) → emu.register_java_class(cls)          │
│  emu.new_java_instance("class") → handle                         │
│  emu.call_java_method(handle, "method()I", args) → result        │
│  emu.release_java_instance(handle)                               │
└──────────────────────────────┬──────────────────────────────────┘
                               │ PyO3 FFI
┌──────────────────────────────┴──────────────────────────────────┐
│               Rust 侧（emulator/bindings/python/src/lib.rs）     │
│                                                                   │
│  PyEmulatorBridge {                                              │
│    engine: EngineHolder,        // Unicorn 引擎                  │
│    linux: Arc<Mutex<LinuxRuntime>>,  // syscall                  │
│    graph: ModuleGraph,          // ELF 模块依赖图                 │
│    runtime: AndroidRuntime,     // JNI canonical authority       │
│    shim: PythonShimAdapter,     // adapter cache（非 authority）  │
│    next_object_id: u64,         // ObjectId 自增计数器            │
│  }                                                               │
│                                                                   │
│  AndroidRuntime {                                                │
│    pub vm: AndroidVM {                                           │
│      pub classes: JniRegistry,  // class/method/field 注册表     │
│      pub objects: ObjectStore,  // 实例池 ★                      │
│      pub refs: RefTable,        // handle 表 ★                   │
│      pub exceptions: ExceptionState,                             │
│      pub apk: Option<ApkContext>,                                │
│    }                                                             │
│  }                                                               │
└──────────────────────────────────────────────────────────────────┘
```

---

## 下一个 change：jni-function-table（建议）

### 为什么是这一步

当前状态：
- ✅ Python 侧可以注册 class、创建实例、调用方法（`call_java_method`）
- ✅ Rust 侧有完整的 ObjectStore + RefTable + JniRegistry
- ✅ `JniEnvSurface` / `JavaVMSurface` 骨架已有但未接入实际执行流
- ❌ **guest 代码（ARM64 native .so）无法调用 JNI 函数**——因为 syscall hook 里没有 JNI 分发
- ❌ `wrap_python_method` 闭包缺少实例绑定——它捕获的是原始 function 对象，不绑定 `self`

**核心场景**：guest .so 里执行 `FindClass` → `GetMethodID` → `NewObject` → `CallVoidMethod`，这整条链不工作。

### 要做什么

#### 1. 实现 JNI function table

在 syscall hook 或 engine hook 中拦截 guest 对 JNI 函数指针表的调用。guest 代码通过 `(*env)->NewObject(env, cls, ctor, bytes)` 调用——这是通过 JNIEnv 函数指针表间接调用。

需要实现的函数（最小集，按优先级）：

| 函数 | 作用 | 优先级 |
|------|------|--------|
| `FindClass` | 查找已注册的 class | P0 |
| `GetMethodID` / `GetStaticMethodID` | 查找 method → MethodId | P0 |
| `NewObject` | 实例化对象 → 进 ObjectStore + RefTable | P0 |
| `CallVoidMethod` / `CallBooleanMethod` / `CallIntMethod` 等 | 调用 instance method | P0 |
| `CallStaticVoidMethod` / 等 | 调用 static method | P1 |
| `GetFieldID` / `GetStaticFieldID` | 查找 field → FieldId | P1 |
| `GetIntField` / `SetIntField` / 等 | 读写 field | P1 |
| `NewGlobalRef` / `DeleteGlobalRef` | RefTable 操作 | P2 |
| `NewStringUTF` / `GetStringUTFChars` | String 对象 | P2 |
| `ExceptionCheck` / `ExceptionClear` | 异常处理 | P2 |

#### 2. 修复 `wrap_python_method` 实例绑定

当前 `wrap_python_method`（`javashim.rs:58`）生成的闭包：
```rust
fn_ref.call1((py_args,))  // self = py_args_tuple（错误！）
```

修复方案：闭包需要能通过 `ObjectId` 从 `ObjectStore` 查找实例。具体做法：
- 修改 `MethodImpl::RustNative` 签名，或新增 `MethodImpl::PythonOverride` 变体，让 handler 能收到 `ObjectId`
- `call_java_method` 的 Python 直调路径暂时保留（功能正确），等 JNI function table 就位后，所有 dispatch 统一走 `JniEnvSurface::call_method(obj_id, sig, args)`

#### 3. 接入 case-runner（端到端验证）

在 case-runner 的 `GuestRuntime` 中：
- 构造 `JNIEnv` 函数指针表并映射到 guest 地址空间
- guest .so 执行 JNI 调用时 → Rust 拦截 → 分发到 AndroidRuntime
- case 3 目前 svc 处失败，因为 syscall dispatch 完成了但 JNI 层没接

---

## 关键文件一览

### JNI crate（emulator/jni/src/）— Rust JNI 核心，16 个文件

| 文件 | 作用 | 重点关注 |
|------|------|----------|
| `lib.rs` | crate 入口，pub use 所有符号 | 30+ 公开类型 |
| `types.rs` | `JType`, `JValue`, `MethodSig`, `FieldSig`, `ObjectId`, `ClassId`, `IdAllocator` | 类型系统根基 |
| `object_store.rs` | `ObjectStore` — 对象池，`ObjectId → (class_name, ObjectStorage)` | `insert/remove/storage/class_name` |
| `object.rs` | `JavaObject` 视图 + 工厂函数（`make_string`, `make_wrapper`, `make_stub`, `make_host_value`） | `make_host_value` 是 Python 对象入口 |
| `refs.rs` | `RefTable` — handle 表，`new_local/new_global/delete_global/resolve/clear_frame` | `clear_frame` 会清除 local refs |
| `registry.rs` | `JniRegistry` — class 注册表 + `dispatch_call/dispatch_static` 分发 | `register_or_merge_class` merge 语义 |
| `dispatch.rs` | 底层分发函数，按 `MethodImpl` 调用 handler | `dispatch_call` 需要 `&mut RefTable` |
| `class.rs` | `JClassDef`, `JMethodDef`, `JFieldDef` + `ClassBuilder` | `override_method` 替换已有实现 |
| `android_vm.rs` | `AndroidVM` — 聚合 `JniRegistry + ObjectStore + RefTable + ExceptionState + ApkContext` | `pub objects: ObjectStore`, `pub refs: RefTable` |
| `android_runtime.rs` | `AndroidRuntime` — 对 `AndroidVM` 的包装 + `classes()/refs()/classes_mut()/refs_mut()` accessor | `register_class` → `vm.classes.register_class` |
| `jnienv.rs` | `JniEnvSurface` — 最小 JNIEnv（`call_method/call_static_method/get_field/new_local_ref` 等） | `call_method` 目前忽略 `_obj: ObjectId`！ |
| `javavm.rs` | `JavaVMSurface` — 最小 JavaVM（`attach_current_thread/detach_current_thread`） | 单线程模型 |
| `exception.rs` | `ExceptionState`, `ExceptionRecord` | |
| `args.rs` | `JniArgs` — 类型化参数获取 | `int_at(0)`, `long_at(1)` 等 |
| `descriptor.rs` | `MethodSig::parse`, `FieldSig::parse` | 字符串 → 类型化签名 |
| `verify.rs` | `PythonCallableAnnotations` — Python 注解 vs descriptor 校验 | |
| `apk_context.rs` | `ApkContext`, `SignatureData` | APK 元数据 |
| `field.rs` | `FieldAccess`, `SharedField` | `SharedField` 用 `Mutex<JValue>` 支持共享可变 |
| `error.rs` | `JniError` — 所有 JNI 操作错误类型 | `ClassNotFound`, `MethodNotFound`, `TypeMismatch`, `NullNotAllowed` |

### Python binding（emulator/bindings/python/src/）

| 文件 | 作用 |
|------|------|
| `lib.rs` | `PyEmulatorBridge`（#[pyclass(name = "Emulator")]）+ `PythonShimAdapter` + `LoadCtxAdapterPy` + `LinkCtxAdapterPy` + `SyscallDispatcherPy` |
| `javashim.rs` | `wrap_python_method` / `wrap_python_method_no_args` / `py_object_to_jvalue` / `validate_return_value` |

### ELF 三层（emulator/elf/）

| 文件 | 作用 |
|------|------|
| `parser/` | ELF 解析（基于 `elf = "0.8"` crate） |
| `loader/` | LOAD segment → 内存映射 |
| `linker/` | 符号解析 + relocation patch |

### 其他关键 crate

| 文件 | 作用 |
|------|------|
| `emulator/os/linux/` | Linux syscall 实现（open/read/mmap/fstat/ioctl 等），**syscall dispatch 入口** |
| `emulator/backends/unicorn/` | Unicorn engine backend |
| `emulator/backends/api/` | `Engine` trait + `SyscallHook` trait + `GuestCPU` trait |
| `emulator/case-runner/` | 装配层，串起整个执行流 |
| `emulator/core/` | `Arch`, `IdAllocator`, `ModuleId` 等基础类型 |
| `emulator/memory/` | 内存 region tracker |
| `python/` | Python 包（`rundroid`），decorator + register + verify |

---

## 当前数据流（实现 JNI function table 前）

```
Python 测试代码:
  register(emu, [MyShim])
    → emu.register_java_class(cls)
      → 解析 metadata → wrap_python_method → MethodImpl::RustNative
      → JniRegistry::register_or_merge_class
    → emu.new_java_instance("class")
      → Python 实例化 → ObjectStore::insert(HostValue) → RefTable::new_global
    → emu.call_java_method(handle, sig, args)
      → RefTable::resolve → ObjectStore::storage → HostValue
      → py_obj.bind(py).call_method(name, args)  ← Python 直调，self 正确
      → 返回值校验

Guest .so 代码（当前不可达）:
  (*env)->NewObject(env, cls, ctor, bytes)
    → ??? 没接入，svc 处失败
```

---

## 关键已知问题

### 1. wrap_python_method 实例绑定缺陷

**位置**: `emulator/bindings/python/src/javashim.rs:58-85`

**问题**: 闭包 `fn_ref.call1((py_args,))` 把 JNI args tuple 当成了 `self`

**修复时机**: JNI function table 实现时。届时 `CallVoidMethod(jobj, methodID, args)` 自然持有 `jobj` → `ObjectId`，可以传给 handler。handler 从 `ObjectStore` 查出实例后再调 Python 方法。

**当前缓解**: `call_java_method` 走 Python 直调路径（通过 `bound.call_method(name, args)`），self 正确绑定。`wrap_python_method` 闭包在 registry 中但尚未通过 `dispatch_call` 被调用。

### 2. JniEnvSurface::call_method 忽略 ObjectId

**位置**: `emulator/jni/src/jnienv.rs:45-52`

```rust
pub fn call_method(
    &mut self,
    _obj: ObjectId,           // ← 前缀 _ ，未被使用
    sig: &MethodSig,
    args: JniArgs,
) -> Result<JValue, JniError> {
    self.registry.dispatch_call(sig, &args, self.refs)
}
```

`dispatch_call` 不需要 `ObjectId`——它只查找 method handler 然后调用。但这意味着 Python override handler（`wrap_python_method` 闭包）拿不到实例，无法绑定 `self`。

### 3. ObjectId 分配未使用全局 IdAllocator

**位置**: `emulator/bindings/python/src/lib.rs` — `PyEmulatorBridge.next_object_id: u64`

当前使用自增计数器，未走 `JniRegistry` 的 `IdAllocator`。`IdAllocator` 在 `JniRegistry` 内部 private，没有公开的 `allocate_object_id()` API。

**修复**: 在 `JniRegistry` 或 `AndroidRuntime` 上暴露 `allocate_object_id()`，让 Python bridge 通过它分配。

### 4. read_instance_field/get_field 读的是 Python attr 不是 JNI field

当前 `read_instance_field` 读的是 Python 实例的 Python 属性（`py_obj.getattr(field_name)`），不是 JNI registry 中注册的 `JFieldDef`。这意味着 `@java_field` 注册的 Rust field handler 和 Python 属性之间没有强一致性。

---

## 测试状态

### Rust 测试（`cargo test --workspace`）
- `rundroid-jni`: **85 tests** — 全部通过（registry, dispatch, object_store, refs, class, verify, exception, apk）
- `rundroid-linux`: 23 tests — 全部通过
- `rundroid-elf-*`: 全部通过
- `rundroid-backend-unicorn`: 1 test（arm64_smoke）— 通过
- `rundroid-case-runner`: 3 tests — 全部通过

### Python 测试（`PYTHONPATH=python uv run pytest python/tests/test_javashim.py`）
- **14 tests** — 全部通过
- test_signature_full_jni_flow — ✓
- test_counter_instance_flow — ✓
- test_multiple_instances — ✓
- test_release_java_instance — ✓
- test_python_override_beats_framework_stub — ✓（含 frameworkOnly 回落）
- test_bad_annotation_fails_at_registration — ✓

⚠️ Windows 上 Unicorn engine Drop 时会有 "Windows fatal exception: access violation"，这是已知的 Unicorn engine cleanup 问题，不影响测试结果。

---

## 构建/运行命令速查

```bash
# 编译检查
cargo check -p rundroid-bindings-python

# Rust 全量测试
cargo test --workspace

# JNI crate 测试
cargo test -p rundroid-jni

# 构建 Python wheel
uvx maturin build -m emulator/bindings/python/Cargo.toml --release
uv pip install --reinstall target/wheels/rundroid_bindings_python-*.whl

# Python 测试
PYTHONPATH=python uv run pytest python/tests/test_javashim.py -v

# openspec 验证
openspec validate --type change <name> --strict
```

## 项目配置要点

- Windows + Git Bash + VS 2022
- cmake: `C:\Program Files\CMake\bin\cmake.exe`
- ninja: `C:\Users\PC\AppData\Local\Microsoft\WinGet\Links\ninja.exe`
- `.cargo/config.toml` 里配了 `[env] PATH` 前置 cmake+ninja
- cargo 镜像: rsproxy.cn（`~/.cargo/config.txt`）
- Python: uv 管理，venv 在 `.venv/`
- NDK 交叉编译（如果需要编译 fixture .so）: `/f/android-ndk/toolchains/llvm/prebuilt/windows-x86_64/bin/aarch64-linux-android21-clang`
- fixture .so: `resources/smoke/build/libsmoke.so`
- unidbg 参考源码: `F:\reverse-workspace\unidbg`
- unidbg test binaries: `F:/reverse-workspace/unidbg/unidbg-android/src/test/resources/example_binaries/arm64-v8a/`

## 设计约定（来自 AGENTS.md）

- 中文注释，函数注释 + 复杂算法注释
- 禁止 `get_xxx` getter，直接用 `xxx()` 取字段
- 首字母缩写全大写：`CPU`, `ARM`, `JNI`, `VM`
- let-it-failed，不写兜底策略
- 链式风格 `Type::build(...)` / `TypeBuilder(...).set(...).build()`
- `ObjectStore` 是对象数据的唯一权威，不建平行 authority
- Python 侧一般不需要实例化 JavaObject，实例化在 guest 层（native 代码通过 JNI），Python 侧只负责定义 + 注册
