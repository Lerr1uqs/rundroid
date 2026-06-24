"""最小死锁复现：持锁期间回调，回调再重入同一资源。"""

from __future__ import annotations

import sys
import threading
from typing import Callable


class ObjectStore:
    """模拟共享对象仓库。"""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._value = "payload"

    def with_lock_then_callback(self, callback: Callable[[], None]) -> None:
        """错误示例：持锁期间执行外部回调。"""
        print("[outer] 准备获取锁")
        with self._lock:
            print("[outer] 已获取锁，准备执行 callback")
            callback()
            print("[outer] callback 返回")

    def read_value(self) -> str:
        """读取共享状态。"""
        print("[inner] 准备再次获取同一把锁")
        with self._lock:
            print("[inner] 已获取锁")
            return self._value

    def snapshot_value(self) -> str:
        """安全做法：只在锁内做最小读取。"""
        print("[safe] 准备获取锁做快照")
        with self._lock:
            print("[safe] 已获取锁，复制数据后立即释放")
            return self._value


def run_deadlock_demo() -> None:
    """演示死锁。"""
    store = ObjectStore()

    def callback() -> None:
        print("[callback] 进入 callback，准备重入 ObjectStore.read_value()")
        value = store.read_value()
        print(f"[callback] 读到值: {value}")

    thread = threading.Thread(
        target=lambda: store.with_lock_then_callback(callback),
        daemon=True,
    )
    thread.start()
    thread.join(timeout=1.0)

    if thread.is_alive():
        print()
        print("结果：线程 1 秒后仍未退出，说明已经死锁。")
        print("原因：outer 持有锁时调用 callback，callback 又去 read_value() 再拿同一把锁。")
    else:
        print("未复现死锁，这不符合预期。")


def run_safe_demo() -> None:
    """演示正确边界：不要持锁执行回调。"""
    store = ObjectStore()

    value = store.snapshot_value()
    print("[safe] 锁已释放，现在再执行用户逻辑")

    def callback() -> None:
        print("[safe-callback] callback 内可以安全重入 read_value()")
        current = store.read_value()
        print(f"[safe-callback] 读到值: {current}")

    print(f"[safe] 外层先拿到快照: {value}")
    callback()
    print("结果：安全版本正常结束，没有死锁。")


def main() -> None:
    mode = sys.argv[1] if len(sys.argv) > 1 else "all"
    if mode == "deadlock":
        run_deadlock_demo()
        return
    if mode == "safe":
        run_safe_demo()
        return

    print("=== deadlock demo ===")
    run_deadlock_demo()
    print()
    print("=== safe demo ===")
    run_safe_demo()


if __name__ == "__main__":
    main()
