"""Android VM 门面——收拢整个 JNI / VM 表面。

照搬既有 ``emu.fs`` 子对象模式（小写属性 + 大写类名）：
- ``emu.avm`` 是 property，返回 ``AVM`` 代理。
- 机器层操作（``load`` / ``call`` / ``write_guest`` / ``fs`` / ``seed`` / ``close``）
  留在 ``emu``，不在 ``avm`` 下。

``AVM`` 持有底层 ``_rundroid.Emulator`` engine，透传 flat Rust 方法，并在
``new_object`` 上做 Python 编排（构造 JavaObject、跑蓝图 ``__init__``、注册到 VM）。
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Type

if TYPE_CHECKING:
    from ._rundroid import Emulator as _Engine
    from .javashim.base import JavaClass, JavaObject


class AVM:
    """emulator 的 Android VM 门面：封装对象构造 + 透传 JNI flat 方法。

    经 ``emu.avm`` 取得。所有对象 → VM 注册统一经 ``register_java_object``，
    构造统一经 ``new_object``（不再有按 class_name 内部实例化的入口）。
    """

    def __init__(self, engine: "_Engine") -> None:
        # _rundroid.Emulator（flat Rust 方法在此）
        self._engine = engine

    # ------------------------------------------------------------------
    # 构造（Python 编排 + Rust 落身份）
    # ------------------------------------------------------------------

    def new_object(self, java_class: "Type[JavaClass]", *args: Any) -> "JavaObject":
        """构造 JavaObject 并注册到 VM，回填 handle。

        流程：
        1. 直接 ``JavaObject.__new__(JavaObject)`` 建实例——绕过 ``JavaClass.__new__``，
           避免回到 ``avm.new_object`` 形成递归。
        2. 挂 ``_java_class`` / ``_avm``。
        3. 跑蓝图 ``__init__(obj, *args)`` 填用户字段（avm 已剥离，不传给 ``__init__``）。
        4. 经 Rust ``register_java_object`` 注册：存 ObjectStore + 分配全局 handle
           （ObjectId 由 AVM 层 IdAllocator 分配）。
        5. 回填 ``_handle``，返回实例。
        """
        from .javashim.base import JavaObject

        # 1-2. 建实例并挂身份引用
        obj = JavaObject.__new__(JavaObject)
        obj._java_class = java_class
        obj._avm = self

        # 3. 跑蓝图 __init__ 填字段（self=obj；avm 不传入）
        java_class.__init__(obj, *args)

        # 4. 注册到 Rust VM（ObjectId 归 AVM 层 IdAllocator，handle 是 jobject 等价物）
        class_name = java_class.__java_class_name__
        handle = self._engine.register_java_object(class_name, obj)

        # 5. 仅回填 _handle（ObjectId 是 Rust 内部 ObjectStore key，不暴露给 Python）
        obj._handle = handle
        return obj

    # ------------------------------------------------------------------
    # flat Rust 方法透传（JNI / VM 表面）
    # ------------------------------------------------------------------
    # 显式声明每个方法，使 AVM 命名空间只含 JNI/VM 操作（机器层方法不在此泄漏）。

    def register_java_class(self, cls: Any) -> None:
        """注册 Java shim class 的 method / field 到 Rust VM。"""
        return self._engine.register_java_class(cls)

    def register_java_object(self, class_name: str, py_obj: Any) -> int:
        """注册已创建的 Python 对象（JavaObject）到 VM，返回全局 handle。"""
        return self._engine.register_java_object(class_name, py_obj)

    def call_java_method(self, handle: int, method_desc: str, args: tuple) -> Any:
        """调用已注册 Java 实例的方法（过渡/调试 API）。"""
        return self._engine.call_java_method(handle, method_desc, args)

    def read_java_field(self, class_name: str, field_desc: str) -> Any:
        """读取已注册的 static field（过渡/调试 API）。"""
        return self._engine.read_java_field(class_name, field_desc)

    def read_instance_field(self, handle: int, field_name: str) -> Any:
        """读取实例的 Python 属性（field 值）。"""
        return self._engine.read_instance_field(handle, field_name)

    def register_framework_stub(self, class_name: str, methods: Any) -> None:
        """注册 framework stub class（纯 Rust-native handler）。"""
        return self._engine.register_framework_stub(class_name, methods)

    def release_java_instance(self, handle: int) -> None:
        """释放 Java 实例——清理 ObjectStore + RefTable。"""
        return self._engine.release_java_instance(handle)

    def java_instance(self, handle: int) -> Any:
        """获取 handle 对应的 Python 实例对象。"""
        return self._engine.java_instance(handle)
