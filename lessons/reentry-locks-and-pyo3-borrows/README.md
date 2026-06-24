# 跨语言回调的递归重入：调宿主语言前，必须释放所有锁与借用

> 相关代码：`emulator/bindings/python/src/lib.rs`（`call_java_method`）、`emulator/bindings/python/src/javashim.rs`（`wrap_python_method`）
> 配套 MVP：`lessons/python-callback-deadlock-mvp/`（用最小 Python demo 复现「持锁回调 → 重入」的 Mutex 自锁）

## 1. 现象

经 Rust bridge（方向 A）触发的 Python 方法，方法体内只要再构造一个对象
（`self._avm.new_object(Other)` → `register_java_object`），就立刻卡死或报错：

- 表现一：**死锁**（线程挂死，pytest 永不返回）
- 表现二：`RuntimeError: Already borrowed`

这是**同一个根因的两层故障**，修一层不够，两层都得修。

## 2. 为什么会犯错

我这次的重点是「让方向 A 的分派复用 `__java_dispatch__`」，于是把调用路径改成了
`instance.<java_name>(args)`。我只盯着「分派对不对」，**完全没顺着方法体往下想**：
方法体里 `self._avm.new_object(...)` 会回调进 Rust 的 `register_java_object`，
而外层 Rust 栈上还攥着一堆东西没放。

具体两层：

### 层 A — `ObjectStore` 的 Mutex 自锁

```text
call_java_method:
    let store = objects.lock();      // 拿到锁
    ...                               // 进入 Python 调用，store 还攥着
        instance.method()             // 方法体
            self._avm.new_object()    // → register_java_object
                objects.lock()        // 再拿同一把锁 → 死锁
```

`std::sync::Mutex` **不是可重入锁**。同一线程第二次 `.lock()` 会永久阻塞。
（这层和 `python-callback-deadlock-mvp` 是完全一样的结构，那边有可运行 demo。）

### 层 B — pyo3 `#[pyclass]` 的借用冲突

就算把层 A 的 Mutex 放了，还会撞上这层：

- `call_java_method` 签名是 `&self` → pyo3 给它一个**不可变借用**（`PyRef`）。
- `register_java_object` 签名是 `&mut self` → pyo3 要**可变借用**（`PyRefMut`）。

pyo3 在 `#[pyclass]` 内部维护了一个类似 `RefCell` 的借用计数器。规则和 `RefCell`
一样：**多个不可变借用可以共存；不可变 + 可变不行**。外层 `call_java_method` 的 `PyRef`
还在栈上，内层 `register_java_object` 想拿 `PyRefMut` → pyo3 抛 `Already borrowed`。

层 A 修了之后，层 B 才浮上来（之前层 A 先死锁，根本走不到层 B）。所以「修了锁还报错」
不是修错了，是第二层露出来了。

## 3. 原理

### 3.1 `Mutex` / `RwLock` 不是可重入的

- `Mutex`：同线程重复 `lock()` → 死锁。
- `RwLock`：同线程先 `read()`、再 `write()`（持有读时拿写）→ 死锁。
- 想支持重入得用专门的 reentrant 锁，但更常见的、更正确的做法是** redesign 锁边界**，
  而不是换一把「能重入」的锁来掩盖设计问题。

### 3.2 pyo3 `#[pyclass]` 的借用模型 ≈ `RefCell`

pyo3 为每个 `#[pyclass]` 生成一个内部借用追踪：

| 方法签名 | pyo3 借用 | 能否与同类借用共存 |
|---|---|---|
| `&self`  | `PyRef`（不可变）   | 多个 `PyRef` 可以 |
| `&mut self` | `PyRefMut`（可变） | 只能独占 |

**递归调用同一个 `#[pyclass]` 的方法时**，外层借用的生命周期覆盖内层。
要内层能成功借用，外层就不能是互斥的那种。

### 3.3 「跨语言回调」的本质

Rust 调 Python（或任何宿主回调），Rust 这边的调用栈**没断**——外层 frame 持有的
每一个 guard、每一个 pyo3 借用，在 Python 执行期间**依然生效**。一旦 Python 回调再
踩进 Rust、碰到同一把锁或同一个 `#[pyclass]` 的互斥借用，就炸。

关键认知：**「回调大概率会重入」是默认假设，不是意外。** 方法体里 `new_object`、
调别的对象方法、读 field……这些都可能回到 Rust。happy path（不重入）只会让 bug 隐藏得更深。

## 4. 正确模式

### 模式一：锁内只做最小读取，复制出 owned 数据，立刻释放锁，再回调

```rust
// ❌ 错：持锁进 Python
let store = objects.lock().unwrap();
let py = store.storage(id).downcast...;
py.call_method(...)            // 期间 store 还攥着 → 方法体重入必死

// ✅ 对：锁内只 clone，出作用域即释放，再回调
let py_obj = {
    let store = objects.lock().unwrap();
    store.storage(id)...clone_ref(py)   // Py<PyAny> 是 refcount clone，廉价
};  // ← 锁在这里释放
py_obj.call_method(...)        // 锁外，方法体可安全重入 objects
```

`call_java_method` 和 `wrap_python_method` 都按这个改。

### 模式二：会被重入的 `#[pyclass]` 方法用 `&self` + 内部可变性

```rust
// ❌ 错：register_java_object(&mut self) 和外层 call_java_method(&self) 借用冲突
fn register_java_object(&mut self, ...) { self.runtime.allocate_object_id(); ... }

// ✅ 对：改 &self，状态用 RwLock 内部可变
// runtime 字段：RwLock<AndroidRuntime>
fn register_java_object(&self, ...) {
    let mut rt = self.runtime.write().unwrap();   // write guard
    rt.allocate_object_id();
    ...
}
```

`&self` → `PyRef`，两个 `PyRef` 可共存，递归重入不再撞借用。

### 模式三：调用 Python 前必须连 RwLock 的 read guard 也放掉

把 `runtime` 包进 `RwLock` 后，`call_java_method` 里读 `runtime` 时拿的 read guard，
**同样不能跨 Python 调用持有**（否则方法体里 `register_java_object` 的 write guard 会和
这个 read guard 死锁 RwLock）。所以 `call_java_method` 的结构是：read guard 读一下马上放、
objects 锁内 clone 后马上放，**两手都空了**再进 Python。

一句话总括：**调宿主语言前，手里不攥任何锁、不攥任何 pyo3 借用。**

## 5. 经验教训（泛化）

1. **Rust → 宿主语言回调 = 默认会重入。** 任何被回调可能再碰到的锁 / 借用，回调前都得释放。
2. **「持锁进回调」是反模式。** 锁内只做最小读取（最好只复制 owned 数据），出锁再回调。
3. **pyo3 递归调用同一个 `#[pyclass]`，必须用 `&self` + 内部可变性（`RwLock`/`Mutex`）。**
   `&mut self` 方法不能在被 `&self` 方法回调的路径上出现。
4. **`RwLock`/`Mutex` 一样会自锁。** 换成 `RwLock` 不是免死金牌，read 持守时 write 照样死。
5. **修了一层别急着庆祝。** 多层故障（这里 Mutex + pyo3 借用）会逐层暴露，修第一层后
   第二层才显形——这很正常，继续修，别误以为「越改越坏」。

## 6. 下次自查清单

写「Rust 调 Python / 宿主回调」的代码前，问自己：

- [ ] 这次 `Python::with_gil` 调用，我手里攥着哪些 `MutexGuard` / `RwLockReadGuard` / `RwLockWriteGuard`？回调前能不能全部 drop 掉？
- [ ] 这个 `#[pyclass]` 方法是 `&self` 还是 `&mut self`？回调路径上有没有递归调用**同一个 pyclass** 的 `&mut self` 方法？有的话，把那个方法改成 `&self` + 内部可变性。
- [ ] 方法体（用户写的 Python）有没有可能回调进 Rust？（`new_object` / `call` / 读 field / 注册……）把它当作「一定会」来设计锁边界。
- [ ] happy path 跑通 ≠ 没问题。有没有专门测「方法体内重入 Rust」的回归用例？

## 7. 这次的具体改动

- `lib.rs`：`call_java_method` 锁内只 clone `Py<PyAny>` + `class_name`，立即释放，再进 Python；
  `runtime` 包成 `RwLock<AndroidRuntime>`；`register_java_object` 由 `&mut self` 改 `&self`。
- `javashim.rs`：`wrap_python_method` 同样改为锁内只 clone instance 引用、释放后再 `call_method1`。
- 回归测试：`tests/test_javaclass_call.py::test_method_body_new_object_via_rust_bridge_no_deadlock`
  （修复前死锁 / `Already borrowed`，修复后秒过）。
