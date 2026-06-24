# 路由缓存的键粒度，必须 ≥ 分派粒度：Java 重载下「方法名」不是身份

> 相关代码：`emulator/bindings/python/src/lib.rs`（`PythonShimAdapter`）、`python/rundroid/javashim/base.py`（`JavaObject.__getattr__`）

## 1. 现象

Python 只 override 了 `foo()I`，framework stub 只提供 `foo(I)I`（**同名、不同签名**）。
调用 `foo(I)I` 时，本该回落 framework（返回 100），却被错误地拦截进 Python 路径，
再在 `__getattr__` 里因为 argc 不匹配抛 `TypeError`。

```text
期望：foo(I)I → framework stub → 100
实际：foo(I)I → 被当成 Python override → instance.foo(5) → 无 argc=1 的重载 → TypeError
```

## 2. 为什么会犯错

`PythonShimAdapter` 里有个「是否存在 Python override」的缓存，决定一次调用走 Python
还是回落 framework。它的键是 `(class_name, java_method_name)`——**只有方法名，没有签名**。

这个键是从旧设计里**原样继承**下来的。旧设计里 instance 是普通 Python 类实例、方法按 py 名
直接调，java_name 粒度「够用」。但新模型下：

- **分派**（`__getattr__`）是按 `(java_name, argc)` 解析重载的；
- **路由判定**（缓存）却只看 `java_name`。

两者粒度不一致。路由缓存说「我有 `foo`」，Python 分派却说「我有的是 `foo()`，不是 `foo(I)`」。
于是 `foo(I)I` 被名字骗进 Python 路径，再被 argc 拒掉。

根因不是某行代码写错，而是：**改了一层（分派变成 argc 级），却没回头审计另一层
（路由缓存还是 name 级）。**

## 3. 原理

### 3.1 Java 方法身份 = 名 + 签名，不是只有名

Java（和 C++）支持重载：`foo()I` 和 `foo(I)I` 是**两个不同的方法**，共用一个名字。
所以「java 方法名」**不是身份**，「名 + descriptor（参数+返回类型）」才是。
JNI 的 `MethodSig` 里 `name` + `args` + `ret` 三者合起来才是唯一键。

### 3.2 路由缓存的键粒度 ≥ 路由判定粒度

一个「这事儿归不归我管」的缓存，它的回答会**决定走向**。这种缓存的键，
粒度必须**至少和它所路由的那个判定一样细**。否则会出现：

> 缓存说「我有 foo」（其实是 `foo()I`），调用方信了，结果真正的 `foo(I)I` 调用被错配。

键比判定粗 → **假阳性**（误认领）→ 误路由。这是「粒度不匹配」最危险的方向。

### 3.3 协作的多个子系统，必须用同一把尺子

这里有两个独立子系统在决定「这个方法归 Python 还是 framework」：

| 子系统 | 用的粒度 |
|---|---|
| Rust 路由缓存（override 是否存在） | 原来：`java_name` |
| Python 分派（能不能真的调到） | `java_name` + `argc` |

它们必须对齐到**较细的那把尺子**（这里是 `argc` 级，因为 Python 分派就只到 argc）。
对齐到 name 级是错的（比判定粗）；对齐到完整 descriptor 级也没必要（Python 分派本身
区分不了同 argc 不同类型，那是另一个 documented 限制）。

## 4. 正确做法

把缓存键加到和分派同粒度：

```rust
// ❌ 键只有 name：foo()I 和 foo(I)I 共用一条，互相误命中
method_names: HashMap<(String, String), String>           // (class, java_name)

// ✅ 键含 argc：foo()I（argc 0）和 foo(I)I（argc 1）分属两条
method_names: HashMap<(String, String, usize), String>     // (class, java_name, argc)
```

注册时：`insert(class, java_name, sig.args.len(), py_name)`。
查询时：`resolve(class, java_name, sig.args.len())`。

再补一条：**静态方法不进这个缓存**（静态方法不在 `__java_dispatch__`，走另一条
`dispatch_call` 路径）。缓存只代表「instance-method override 是否存在」，语义要纯。

修完后：`foo()I`（argc 0）命中 Python override；`foo(I)I`（argc 1）查不到 → 正确回落 framework。

## 5. 经验教训（泛化）

1. **「名字不是身份」——任何支持重载的语言（Java / C++ / Kotlin…）都一样。**
   方法的唯一标识是 `名 + 签名`，别用裸名字当哈希键 / 缓存键 / 去重键。
2. **路由 / 命中类缓存，键粒度必须 ≥ 它所决定的判定粒度。** 键偏粗 = 假阳性 = 误路由。
3. **改了某一层的粒度（分派、解析、去重），必须回头审计所有协作的缓存 / lookup。**
   沿用旧键设计前，先问一句「旧键的粒度在新模型下还成立吗」。
4. **两个子系统协作时，对齐到「较细的尺子」。** 宁可键里多带一个字段（多占点内存），
   也不要让粗粒度的一方给细粒度的一方做错误承诺。
5. **测试要覆盖「同名不同签名」的混合场景**，不能只测「不同名的 override + fallback」
   （后者掩盖了粒度问题，正是这次 review 抓到的盲区）。

## 6. 下次自查清单

设计 / 改动「方法 / 符号 → 处理者」的路由时：

- [ ] 这个语言 / 协议里，「名字」是不是唯一标识？有重载吗？（有 → 名字不够）
- [ ] 决定路由的那个判定，最细到什么粒度？（参数个数？类型？descriptor？）
- [ ] 我用来做「归不归我管」判定的缓存 / map，键粒度 ≥ 那个判定粒度了吗？
- [ ] 有没有两个子系统（注册侧 / 查询侧、Rust 侧 / Python 侧）各自维护一份身份判定？
   它们用的是同一把尺子吗？
- [ ] 回归测试里，有没有「同名、不同签名、分别由不同处理者负责」的用例？

## 7. 这次的具体改动

- `lib.rs`：`PythonShimAdapter.method_names` 键改为 `(class, java_name, argc)`；
  `insert_method_name` / `resolve_method_name` 加 `argc` 参数；
  注册时只对 **instance method** 入表（`if !is_static`），`argc = sig.args.len()`；
  `call_java_method` 查询时传 `sig.args.len()`。
- 回归测试：`tests/test_javaclass_call.py::test_same_name_different_signature_override_and_fallback`
  （Python `foo()I` + framework `foo(I)I`，分别走各自路径）。
