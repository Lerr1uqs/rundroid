# rundroid JNI shim 层
#
# 本包是 rundroid 的 Python JNI shim 入口，提供：
# - `JavaObject` — Java 对象基类
# - JNI 类型标注：`JBoolean`, `JByte`, `JChar`, `JShort`, `JInt`, `JLong`, `JFloat`, `JDouble`, `JObject`, `JVoid`
# - `@java_class` / `@java_method` / `@java_field` — metadata-only decorator
# - `register` — 显式注册 shim class 到 emulator
#
# # 使用方式
#
# ```python
# from rundroid.javashim import JavaObject, java_class, java_method, register
# from rundroid.javashim.types import JInt
# from rundroid import Emulator
#
# @java_class("android/content/pm/Signature")
# class Signature(JavaObject):
#     @java_method("hashCode()I")
#     def hashCode(self) -> JInt:
#         return 0x12345678
#
# emu = Emulator("arm64", "unicorn")
# register(emu, [Signature])
# ```

from .base import JavaObject
from .decorators import java_class, java_method, java_field
from .registry import register
from .types import (
    JBoolean,
    JByte,
    JChar,
    JDouble,
    JFloat,
    JInt,
    JLong,
    JObject,
    JShort,
    JVoid,
)

__all__ = [
    "JavaObject",
    "java_class",
    "java_method",
    "java_field",
    "register",
    "JBoolean",
    "JByte",
    "JChar",
    "JShort",
    "JInt",
    "JLong",
    "JFloat",
    "JDouble",
    "JObject",
    "JVoid",
]
