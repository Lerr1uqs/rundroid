"""显式 Java 内置值 wrapper——identity 敏感场景的 opt-in 身份层。

默认场景下 Python 原生 ``str`` / ``bytes`` 会自动 coercion 成 ``java/lang/String`` /
``byte[]``（见 change ``python-jni-value-marshalling``，决策 1）。但当需要**复用同一
对象身份**（同一 handle / ObjectId）时——例如同一个 String 被多次传递、或需与 guest
侧持有的 jobject 指向同一对象——应显式构造 wrapper（决策 2）。

wrapper 在构造时即落入 Rust ``ObjectStore``（分配 ObjectId + global handle）；
marshalling 见到 wrapper 时**复用其 ObjectId**（读取 ``_rundroid_oid``），不再新建对象，
从而保留身份。这与自动 coercion（每次 ``str`` 都新建一个 String）形成互补。

构造风格与 ``JavaClass`` / ``JavaObject`` 一致：首参显式传 avm——
``JavaString(avm, "hello")``，等价于 ``avm.new_string("hello")``。
"""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from ..avm import AVM


class JavaString:
    """``java/lang/String`` 的显式 wrapper（identity 敏感）。

    携带：
    - ``_handle``        —— Rust VM 分配的全局句柄（JNI ``jobject`` 等价物）
    - ``_rundroid_oid``  —— Rust ``ObjectStore`` 内部 key，marshalling 据此复用身份
    - ``value``          —— 包装的 Python ``str``（便于调试 / 相等比较）

    marshalling 规则（见 Rust 侧 ``javashim::py_to_jvalue``）：见到 ``JavaString``
    实例时直接取 ``_rundroid_oid`` → ``JValue::Object(oid)``，不新建 String 对象。
    """

    _handle: int
    _rundroid_oid: int

    def __init__(self, avm: "AVM", value: str) -> None:
        handle, oid = avm.register_java_string(value)
        self._handle = handle
        self._rundroid_oid = oid
        self._value = value

    @property
    def value(self) -> str:
        """wrapper 包装的 Python ``str`` 值。"""
        return self._value

    def __repr__(self) -> str:
        return f"JavaString({self._value!r}, oid={self._rundroid_oid})"


class JavaByteArray:
    """``byte[]`` 的显式 wrapper（identity 敏感）。语义同 :class:`JavaString`。"""

    _handle: int
    _rundroid_oid: int

    def __init__(self, avm: "AVM", value: bytes) -> None:
        handle, oid = avm.register_java_bytes(value)
        self._handle = handle
        self._rundroid_oid = oid
        self._value = bytes(value)

    @property
    def value(self) -> bytes:
        """wrapper 包装的 Python ``bytes`` 值。"""
        return self._value

    def __repr__(self) -> str:
        return f"JavaByteArray({self._value!r}, oid={self._rundroid_oid})"
