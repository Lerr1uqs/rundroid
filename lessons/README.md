# Lessons —— 踩坑复盘

每次犯过的错，沉淀成一篇「为什么会犯错 + 原理 + 经验教训 + 自查清单」，
方便以后复习、避免重犯。每个子目录一个主题。

## 目录

| 主题 | 涉及代码 | 一句话 |
|---|---|---|
| [python-callback-deadlock-mvp](python-callback-deadlock-mvp/) | 纯 Python demo | 最小复现：持锁期间回调，回调再重入同一资源 → Mutex 自锁 |
| [reentry-locks-and-pyo3-borrows](reentry-locks-and-pyo3-borrows/) | `emulator/bindings/python/src/lib.rs`、`javashim.rs` | 跨语言回调的递归重入：调宿主语言前，必须释放**所有锁 + pyo3 借用**（Mutex 自锁 + `Already borrowed` 两层） |
| [dispatch-cache-key-granularity](dispatch-cache-key-granularity/) | `lib.rs`（`PythonShimAdapter`）、`javashim/base.py` | 路由缓存的键粒度必须 ≥ 分派粒度；Java 重载下「方法名」不是身份 |

## 怎么读

- 想看可运行的最小 demo → 从 `python-callback-deadlock-mvp/` 开始（`python deadlock_demo.py`）。
- 想理解「Rust 回调 Python 还能再回 Rust」为什么会死锁 / 借用冲突 → `reentry-locks-and-pyo3-borrows/`。
- 想理解「同名不同签名为什么被误命中」→ `dispatch-cache-key-granularity/`。

每篇都带「下次自查清单」，写新代码前可以对着过一遍。
