# rundroid JNI shim 层
#
# 本包是 rundroid 的 Python JNI shim 入口，提供：
# - `JavaClass` — Java 对象蓝图基类（用户继承：`class Signature(JavaClass)`）
# - `JavaObject` — 实例类型（携带 VM 身份；由 `avm.new_object` 构造）
# - JNI 类型标注：`JBoolean`, `JByte`, `JChar`, `JShort`, `JInt`, `JLong`, `JFloat`, `JDouble`, `JObject`, `JVoid`
# - `@java_class` / `@java_method` / `@java_field` — metadata-only decorator
# - `register` — 显式注册 shim class 到 emulator
#
# # 使用方式
#
# ```python
# from rundroid.javashim import JavaClass, java_class, java_method, register
# from rundroid.javashim.types import JInt
# from rundroid import Emulator
#
# @java_class("android/content/pm/Signature")
# class Signature(JavaClass):
#     @java_method("hashCode()I")
#     def hashCode(self) -> JInt:
#         return 0x12345678
#
# emu = Emulator("arm64", "unicorn", 42)
# register(emu, [Signature])
# obj = Signature(emu.avm)   # 显式传 avm 构造
# ```

from .base import JavaClass, JavaObject
from .decorators import java_class, java_field, java_method
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
from .values import JavaByteArray, JavaString
from .verify import (
    annotation_to_jni_descriptor,
    extract_type_hints,
    get_param_types_jni,
    get_return_type_jni,
)

__all__ = [
    "JavaClass",
    "JavaObject",
    "JavaString",
    "JavaByteArray",
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
    "annotation_to_jni_descriptor",
    "extract_type_hints",
    "get_return_type_jni",
    "get_param_types_jni",
]
