"""JNI shim 显式注册。

methods 已由 ``JavaClass.__init_subclass__`` 在类创建时建好 ``__java_methods__``，
``register()`` 只负责收集 ``@java_field`` 标记的字段写入 ``__java_static_fields__``，
再调用 Rust bridge（经 ``emulator.avm``）注册到 JNI registry。

# 注册流程
1. 检查 class 是否有 ``__java_class_name__``（即被 @java_class 标记）
2. 复用 ``__java_methods__``（methods 不再在此收集）
3. 扫描 ``@java_field`` 标记的字段，收集 descriptor 和初始值
4. 将 field metadata 写入 ``__java_static_fields__`` 供 Rust bridge 读取
5. 调用 ``emulator.avm.register_java_class(cls)`` 完成注册

注册不注入类级 ``_avm``——avm 在对象构造时显式传入（``Cls(avm)``）。
"""

from __future__ import annotations

import inspect
from typing import TYPE_CHECKING, List, Tuple, Type

if TYPE_CHECKING:
    from ..emulator import Emulator
    from .base import JavaClass


def register(emulator: "Emulator", classes: "List[Type[JavaClass]]") -> None:
    """将 decorated JNI shim class 显式注册到 emulator。

    参数：
        emulator: rundroid ``Emulator`` 实例（wrapper）
        classes: 由 @java_class decorator 标记的 Python class 列表

    任何 descriptor 解析失败或注册冲突都会立即抛出异常（fail-fast）。
    """
    for cls in classes:
        if not hasattr(cls, "__java_class_name__"):
            raise ValueError(
                f"class {cls.__name__} 缺少 @java_class decorator，"
                f"请先使用 @java_class('full/class/Name') 标记"
            )

        # methods 复用 __init_subclass__ 已建的 __java_methods__，此处不重复收集。
        # 仅收集 @java_field 标记的字段。
        static_fields = _collect_static_fields(cls)
        cls.__java_static_fields__ = static_fields  # type: ignore[attr-defined]

        # 经 avm 门面注册到 Rust VM
        emulator.avm.register_java_class(cls)


def _collect_static_fields(
    cls: "Type[JavaClass]",
) -> "List[Tuple[str, str, bool, object]]":
    """扫描 class 上的 ``@java_field`` 标记，收集字段元数据。

    返回 ``(attr_name, descriptor, is_static, initial)`` 列表，供 Rust bridge 读取。
    """
    fields: "List[Tuple[str, str, bool, object]]" = []
    for attr_name, attr_val in inspect.getmembers(cls):
        if attr_name.startswith("__") and attr_name.endswith("__"):
            continue
        desc: str | None = getattr(attr_val, "__java_field_descriptor__", None)
        if desc is None:
            continue
        is_static = getattr(attr_val, "__java_field_static__", True)
        initial = getattr(attr_val, "__java_field_value__", None)
        # @java_field 未显式给 initial 时，若装饰对象本身是常量值则取之
        if initial is None and not callable(attr_val):
            if isinstance(attr_val, (int, float, bool)):
                initial = attr_val
        fields.append((attr_name, desc, is_static, initial))
    return fields
