# rundroid Python Stub 层
#
# 本包是 rundroid 的 Python 侧入口，提供：
# - `Emulator` — 装配好的 Unicorn + Linux 运行时（轻量 wrapper，包 _rundroid.Emulator）
# - `VirtFile` — 普通文件挂载来源（bytes / host）
# - `VirtualDevice` — 自定义虚拟设备基类
# - `@device` / `@file_node` — decorator 元数据声明
# - `register` — 批量注册设备/文件类

from ._rundroid import VirtFile
from .avm import AVM
from .drivers import VirtualDevice, device, file_node
from .emulator import Emulator
from .register import register

# JNI shim 子包（按需导入，不强制依赖）
from . import javashim  # noqa: F401
