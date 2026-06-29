## Context

当前 JNI dispatch 流程（以 instance method 为例）：

```
trampoline dispatch → JNIEnvSurface::call_int_method_by_id
  → dispatch_by_method_id(obj_handle, method_id, raw_args, ret_type)
    → resolve_method_by_id → 找到 MethodSig
    → has_native(method_id) == true → ❌ 返回错误 (未实现)
    → dispatch_call → registry 找 Rust/Python handler
```

本 change 要改为：

```
dispatch_by_method_id
  → resolve_method_by_id → 找到 MethodSig
  → has_native(method_id) == true → ✅ 调 call_guest_function
    → 包装参数 (JNIEnv* + jclass/jobject + args)
    → call_guest_function(fn_ptr, &u64_args)
    → 返回值按 ret_type 分发 → 返回 JValue
  → (原路径) dispatch_call → registry handler
```

## Architecture

### 1. `call_guest_function` trait

`call_export`（在 `GuestRuntime` 中）的 sentinel 机制需要暴露给 JNI 层。JNI 层——`JniEnvSurface`——当前不持有 `Engine`。所以需要一个 trait 把 "进 guest 执行" 抽象出来：

```rust
/// guest 函数调用能力 —— 从 host 侧调 guest 侧一段函数指针。
///
/// 实现方（case-runner / Python binding）提供 sentinel 机制的 engine 访问。
pub trait GuestFunctionCaller: Send {
    /// 调用 guest 函数，传入参数切片（每个 u64 对应一个寄存器），返回 x0。
    fn call_guest_function(&mut self, fn_ptr: u64, args: &[u64]) -> Result<u64, BackendError>;
}
```

`JniEnvSurface` 新增字段 `guest_caller: Option<Box<dyn GuestFunctionCaller>>`，初始化时可选传入。

对比直接传 `&mut dyn Engine`：trait 更轻量，调用方只需关心"进 guest 执行"这一件事，不需要知道 engine 的全部能力（mem_map/reg_write/emu_start 等）。同时也避免 `JniEnvSurface` 对 `Backend` / `Engine` crate 的直接依赖。

当前只在 case-runner / Python binding 层组装时传入 `GuestFunctionCaller` 实现（包装 `Engine::emu_start` + sentinel 管理）。纯 unit test 路径不走 `call_guest_function`（不设此字段），native binding 命中时返回 `JniError::Internal` 兼容现有行为。

### 2. `find_native_guest_fn` 查询序

`JniEnvSurface` / `NativeRegistry` 新增：

```rust
impl JniEnvSurface<'_> {
    /// 查找一个 Java native 方法的 guest 函数指针。
    ///
    /// 查询序：
    /// 1. RegisterNatives 绑定的 maps：MethodId → GuestPtr
    /// 2. Java_* mangled 符号（已装载模块中按符号名查找）
    /// 3. 找不到 → None（由调用方决定报错或回落）
    pub fn find_native_guest_fn(
        &self,
        class_name: &str,
        method_name: &str,
        method_sig: &MethodSig,
    ) -> Option<u64> {
        // 1. 先查 RegisterNatives 注册表
        let mid = self.resolve_method_id(class_name, &method_sig.name)?;
        if let Some(addr) = self.lookup_native(mid) {
            return Some(addr);
        }
        // 2. 再查 Java_* 符号
        self.resolve_java_native_fn(class_name, method_name, &method_sig.raw_descriptor())
    }

    /// 按 Java_* mangling 在已装载模块中查找符号。
    fn resolve_java_native_fn(&self, class_name: &str, method_name: &str, sig: &str) -> Option<u64> {
        // 委托给 GuestRuntime::resolve_java_native 或直接调 mangle_java_method
    }
}
```

注意：`resolve_java_native` 需要在 `GuestFunctionCaller` 的持有方实现（需要访问 `ModuleGraph`），或在 `JniEnvSurface` 上以另一种方式注入。设计上，Java_* 查找需要 loaded modules 的符号表访问——当前在 `GuestRuntime` 中。JNI 层的 `JniEnvSurface` 不直接持有 graph。

**决策**：`find_native_guest_fn` 优先查 `NativeRegistry`（JNI 层自有），Java_* 回落查找通过 `GuestFunctionCaller` trait 扩展（或另设 `SymbolResolver` trait），不在 JNI 层引入 ModuleGraph 依赖。

### 3. 参数编组（JValue → u64）

ARM64 AArch64 ABI 下，JNI native 函数参数传递规则：

| 参数位置 | 寄存器 | JNI 参数 |
|---------|--------|---------|
| arg0    | x0     | JNIEnv*（guest 可见的 env_ptr） |
| arg1    | x1     | jclass（static 方法）或 jobject（instance 方法） = class_id / object_id 的 u64 |
| arg2..  | x2..x7 | 方法参数，按 JType 映射 |

JValue → u64 映射规则：

- `JValue::Int(v)` → `(v as i32) as u64`（符号扩展）
- `JValue::Long(v)` → `v as u64`
- `JValue::Float(f)` → `f.to_bits() as u64`
- `JValue::Double(d)` → `d.to_bits() as u64`
- `JValue::Object(oid)` → `oid.0 as u64`
- `JValue::Null` → `0u64`
- `JValue::Boolean(b)` → `if b { 1 } else { 0 }`
- `JValue::Byte(b)` → `(b as i8) as u64`
- `JValue::Char(c)` → `c as u64`
- `JValue::Short(s)` → `(s as i16) as u64`

### 4. 返回值分发（x0 → JValue）

`call_guest_function` 返回 x0（u64），按 `ret_type` 映射回 `JValue`：

- `JType::Int` → `JValue::Int(x0 as i32)`
- `JType::Long` → `JValue::Long(x0 as i64)`
- `JType::Float` → `JValue::Float(f32::from_bits(x0 as u32))`
- `JType::Double` → `JValue::Double(f64::from_bits(x0))`
- `JType::Void` → `JValue::Void`
- `JType::Object(_)` → `JValue::Object(ObjectId(x0 as u32))`（oid 可能为零表示 null）
- `JType::Boolean` → `JValue::Boolean(x0 != 0)`
- `JType::Byte` → `JValue::Byte(x0 as i8)`
- `JType::Char` → `JValue::Char(x0 as u16)`
- `JType::Short` → `JValue::Short(x0 as i16)`

### 5. 集成点

修改 `dispatch_by_method_id` 和 `dispatch_static_by_method_id`（均在 `jnienv.rs`）：

```
当前: has_native(mid) == true → 返回 Err
改为: has_native(mid) == true && guest_caller.is_some()
  → 查找 MethodId（resolve_method_by_id）
  → 编组参数（第3节）
  → 调 guest_caller.call_guest_function(fn_ptr, &args)
  → 分发返回值（第4节）→ 返回 Ok(JValue)
     guest_caller.is_none()
  → 保持 Err（无 Engine 的测试路径）
```

**方法签名**：`dispatch_by_method_id` 和 `dispatch_static_by_method_id` 内部判断——不是在 `call_int_method_by_id` / `call_void_method_by_id` 等每个公开方法上改，避免 12 个 instance + 12 个 static 方法逐一修改。

### 6. 嵌套支持

当 guest native 函数内部再调 JNI（guest→host），trampoline callback 命中 host handler → host handler 可能再触发 guest native 调用 → 再 `emu_start(fn_ptr)`。Unicorn 原生支持嵌套 `emu_start` —— 不需要额外工程。

## Rules

1. `JniEnvSurface` 不直接依赖 `Backend` / `Engine` trait——通过 `GuestFunctionCaller` trait 解耦
2. `find_native_guest_fn` 查询序：RegisterNatives → Java_* 符号 → None
3. `call_guest_function` 复用 `call_export` 的 sentinel+stack 机制（不新建第二种"进 guest"方式）
4. 参数编组规则在单一函数 `jvalues_to_native_args` 中收敛（不分散到各 `call_xxx_method_by_id`）
5. 返回值分发规则在单一函数 `native_return_to_jvalue` 中收敛
6. Java_* 符号回落查找通过调用方注入（不在 JNI 层依赖 ModuleGraph）
