"""设备 / 文件节点注册主线。

提供 `register()` 函数，把带 decorator 元数据的类批量挂载到 runtime。
"""

from typing import List, Type


def register(runtime, classes: List[Type]) -> None:
    """把一组设备/文件类按 decorator 声明的路径挂载到 runtime。

    - 带 `__device_path__` 的类通过 `runtime.fs.map_device()` 挂载
    - 带 `__file_path__` 的类（暂不自动处理，需手动调用 map_file）
    """
    for cls in classes:
        path = getattr(cls, "__device_path__", None)
        if path is not None:
            runtime.fs.map_device(path, cls)
