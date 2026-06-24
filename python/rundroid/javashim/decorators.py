"""JNI shim decorator。

这些 decorator 只附加 metadata 属性到 class / method 上，
不做即时 runtime 注册。import 模块不会污染全局 registry。
真正生效必须显式调用 `register(emulator, [MyClass])`。

# 核心规则
- decorator 只挂 metadata，不自动注册
- import 模块不污染全局 JNI registry
- 注解不匹配在 register() 阶段 fail-fast
"""

from __future__ import annotations

from typing import Callable, TypeVar, Union
from .base import JavaClass

_JT = TypeVar("_JT", bound=type[JavaClass])
_F = TypeVar("_F", bound=Callable[..., object])


def java_class(name: str) -> Callable[[_JT], _JT]:
    """声明一个 Python class 是 Java class 的 shim。

    参数：
        name: Java class 的 slash-separated 全限定名（如 ``"android/content/pm/Signature"``）。
    """
    def decorator(cls: _JT) -> _JT:
        cls.__java_class_name__ = name  # type: ignore[attr-defined]
        if not hasattr(cls, "__java_methods__"):
            cls.__java_methods__ = []  # type: ignore[attr-defined]
        if not hasattr(cls, "__java_static_fields__"):
            cls.__java_static_fields__ = []  # type: ignore[attr-defined]
        return cls
    return decorator


def java_method(descriptor: str) -> Callable[[_F], _F]:
    """声明一个 method 对应 Java method。

    参数：
        descriptor: method descriptor 字符串。
                   格式为 ``methodName(argTypes)returnType``。
                   示例: ``"hashCode()I"``, ``"Signature([B)V"``。
                   class name 由外层的 @java_class 提供。
    """
    def decorator(func: _F) -> _F:
        func.__java_method_descriptor__ = descriptor  # type: ignore[attr-defined]
        return func
    return decorator


def java_field(
    *,
    name: str,
    sig: str,
    initial: Union[int, float, bool, bytes, None] = None,
) -> Callable[[_F], _F]:
    """声明一个 field 对应 Java field。

    参数：
        name: field 名称（如 ``"mSignature"``）
        sig: field 类型 descriptor（如 ``"[B"``、``"I"``）
        initial: 初始值（可选）
    """
    def decorator(func: _F) -> _F:
        func.__java_field_descriptor__ = f"{name}:{sig}"  # type: ignore[attr-defined]
        func.__java_field_name__ = name  # type: ignore[attr-defined]
        func.__java_field_value__ = initial  # type: ignore[attr-defined]
        return func
    return decorator
