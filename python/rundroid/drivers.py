"""VirtualDevice 基类、VirtFile、decorator 元数据。

本模块提供 Python 侧声明虚拟设备与文件节点的全部基础设施。
decorator 只做"声明元数据"，不会在 import 时自动挂载到 runtime。
"""

from typing import Optional, Callable, List, Type


class VirtualDevice:
    """虚拟设备基类。

    子类可重写：
    - `open(self, flags: int, mode: int)` — 设备被 open 时调用
    - `read(self, fd: int, size: int) -> bytes` — 返回最多 size 字节
    - `write(self, fd: int, data: bytes) -> int` — 返回实际写入长度
    - `close(self, fd: int)` — 设备被 close 时调用

    所有方法都有默认空实现。
    """

    def open(self, flags: int, mode: int) -> None:
        """设备被 open 时调用。无状态设备可留空。"""
        pass

    def read(self, fd: int, size: int) -> bytes:
        """从设备读取最多 size 字节。默认返回 EOF。"""
        return b""

    def write(self, fd: int, data: bytes) -> int:
        """向设备写入数据。返回实际写入字节数。默认丢弃写入。"""
        return len(data)

    def close(self, fd: int) -> None:
        """设备被 close 时调用。"""
        pass


def device(path: str):
    """类 decorator：声明此类为虚拟设备，默认挂载路径为 `path`。

    用法：
        @device("/dev/mydev")
        class MyDevice(VirtualDevice):
            def read(self, fd, size):
                return b"A" * size

    import 时不会自动挂载到 runtime；需显式调用 `register(runtime, [MyDevice])`
    或 `runtime.fs.map_device("/dev/mydev", MyDevice)`。
    """
    def decorator(cls):
        cls.__device_path__ = path
        return cls
    return decorator


def file_node(path: str, kind: str = "regular"):
    """类 decorator：声明此类为虚拟文件节点（动态 provider）。

    用法：
        @file_node("/proc/self/maps")
        class ProcSelfMaps:
            def bytes(self) -> bytes:
                return b"..."
    """
    def decorator(cls):
        cls.__file_path__ = path
        cls.__file_kind__ = kind
        return cls
    return decorator
