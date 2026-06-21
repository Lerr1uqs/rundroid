"""JNI shim 显式注册。

遍历 decorated class 的 metadata，收集 method / field 定义，
解析 descriptor 并调用 Rust bridge 注册到 emulator 的 JNI registry。

# 注册流程
1. 检查 class 是否有 `__java_class_name__`（即被 @java_class 标记）
2. 扫描 class 的方法，收集被 @java_method 标记的方法
3. 扫描 @java_field 标记的字段，收集 descriptor 和初始值
4. 将 metadata 写入 `__java_methods__` / `__java_static_fields__` 供 Rust bridge 读取
5. 调用 Rust bridge 的 `register_java_class` 完成注册
"""

from __future__ import annotations

import inspect
from typing import TYPE_CHECKING, List, Tuple, Type

if TYPE_CHECKING:
    from .._rundroid import Emulator
    from .base import JavaObject


def register(emulator: Emulator, classes: List[Type[JavaObject]]) -> None:
    """将 decorated JNI shim class 显式注册到 emulator。

    参数：
        emulator: rundroid Emulator 实例
        classes: 由 @java_class decorator 标记的 Python class 列表

    任何 descriptor 解析失败或注册冲突都会立即抛出异常（fail-fast）。
    """
    for cls in classes:
        if not hasattr(cls, "__java_class_name__"):
            raise ValueError(
                f"class {cls.__name__} 缺少 @java_class decorator，"
                f"请先使用 @java_class('full/class/Name') 标记"
            )

        # —— 收集 method ——
        # 遍历 class __dict__ + getmembers，避免遗漏 @staticmethod / @classmethod
        methods: List[Tuple[str, str, object, bool]] = []
        for attr_name, attr_val in inspect.getmembers(cls):
            if attr_name.startswith("__") and attr_name.endswith("__"):
                continue
            desc: str | None = getattr(attr_val, "__java_method_descriptor__", None)
            if desc is not None:
                is_static = not inspect.isfunction(attr_val) or isinstance(
                    inspect.getattr_static(cls, attr_name, None), staticmethod
                )
                methods.append((attr_name, desc, attr_val, is_static))

        # 检查 __dict__ 中 @staticmethod / @classmethod 包装的函数
        for attr_name in cls.__dict__:
            if attr_name.startswith("__") and attr_name.endswith("__"):
                continue
            raw = cls.__dict__[attr_name]
            if isinstance(raw, staticmethod):
                func = raw.__func__
                desc = getattr(raw, "__java_method_descriptor__", None) or getattr(
                    func, "__java_method_descriptor__", None
                )
                if desc is not None and not any(m[0] == attr_name for m in methods):
                    methods.append((attr_name, desc, func, True))
            elif isinstance(raw, classmethod):
                func = raw.__func__
                desc = getattr(raw, "__java_method_descriptor__", None) or getattr(
                    func, "__java_method_descriptor__", None
                )
                if desc is not None and not any(m[0] == attr_name for m in methods):
                    methods.append((attr_name, desc, func, False))

        # —— 收集 @java_field 标记的字段 ——
        static_fields: List[Tuple[str, str, bool, object]] = []
        for attr_name, attr_val in inspect.getmembers(cls):
            if attr_name.startswith("__") and attr_name.endswith("__"):
                continue
            desc: str | None = getattr(attr_val, "__java_field_descriptor__", None)
            if desc is not None:
                is_static = getattr(attr_val, "__java_field_static__", True)
                initial = getattr(attr_val, "__java_field_value__", None)
                if initial is None and not callable(attr_val):
                    if isinstance(attr_val, (int, float, bool)):
                        initial = attr_val
                static_fields.append((attr_name, desc, is_static, initial))

        # —— 写入 metadata + 调用 Rust bridge 注册 ——
        cls.__java_methods__ = methods  # type: ignore[attr-defined]
        cls.__java_static_fields__ = static_fields  # type: ignore[attr-defined]
        emulator.register_java_class(cls)  # type: ignore[attr-defined]
