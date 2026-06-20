# rundroid Python Stub 层
#
# 本包是 rundroid 的 Python 侧入口，提供：
# - `Runtime` — 装配好的 Unicorn + Linux 运行时
# - `VirtFile` — 普通文件挂载来源（bytes / host）
# - `VirtualDevice` — 自定义虚拟设备基类
# - `@device` / `@file_node` — decorator 元数据声明
# - `register` — 批量注册设备/文件类
#
# 实际执行引擎由 `_rundroid` C 扩展提供（Rust 侧编译产物）。

from ._rundroid import Runtime, VirtFile
from .drivers import VirtualDevice, device, file_node
from .register import register
