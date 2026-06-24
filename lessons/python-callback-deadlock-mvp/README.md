# Python Callback Deadlock MVP

这个目录是一个独立教学示例，用最小代码复现下面这类死锁：

1. 外层函数先拿到一把互斥锁
2. 持锁期间调用用户回调
3. 用户回调里再次进入同一资源
4. 内层再次申请同一把锁
5. 因为锁还在当前线程手里，所以永远卡住

这和很多“Rust 持有 `MutexGuard`，随后回调 Python；Python 又调用回 Rust，再次访问同一状态”的问题是同一个结构。

## 文件

- `deadlock_demo.py`
  - `run_deadlock_demo()`：故意写错，稳定复现死锁
  - `run_safe_demo()`：先复制需要的数据，再释放锁，再回调，演示正确做法

## 运行

```powershell
python .\deadlock_demo.py
```

如果你只想看死锁复现：

```powershell
python .\deadlock_demo.py deadlock
```

只想看安全版本：

```powershell
python .\deadlock_demo.py safe
```

## 为什么会死锁

核心不是 Python 或 Rust 本身，而是锁的生命周期：

```text
outer() 获取 lock
  -> callback()
      -> inner()
          -> 再次获取同一个 lock
```

`threading.Lock` 不是可重入锁。同一线程重复获取时，不会报错，而是一直等，形成自锁。

## 这个示例想说明什么

- 不要在持有共享状态锁时执行用户回调
- 回调前先把需要的数据复制出来
- 释放锁后再进入 callback / Python / plugin / hook
- 如果确实要支持重入，需要重新设计所有权和锁边界，而不是靠运气
