"""Python 类型注解提取器，用于 JNI shim 严格校验。

从 Python 方法注解中提取返回值类型和参数类型，
转换为 JNI descriptor 字符串，供 Rust bridge 在注册时
通过 `PythonCallableAnnotations::verify()` 做 exact match 检查。

# 核心规则
- decorator 声明的 descriptor 必须与 Python type hint 完全一致
- 不匹配在 register() 阶段 fail-fast，不延迟到运行时
- 如果方法没有 type hint，跳过 verify（向后兼容）
"""

from __future__ import annotations

import inspect
import typing

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

# Python 类型标注 → JNI descriptor 字符 映射表
#
# JNI descriptor 语法：
#   V=void, Z=boolean, B=byte, C=char, S=short,
#   I=int, J=long, F=float, D=double,
#   Lfull/class/name; = object,  [type = array
_TYPE_TO_JNI: dict[type, str] = {
    JVoid: "V",
    JBoolean: "Z",
    JByte: "B",
    JChar: "C",
    JShort: "S",
    JInt: "I",
    JLong: "J",
    JFloat: "F",
    JDouble: "D",
    JObject: "Ljava/lang/Object;",
    # Python 原生类型也做映射（便捷写法）
    bool: "Z",
    int: "I",
    float: "D",
    str: "Ljava/lang/String;",
    bytes: "[B",  # Java byte[]
    type(None): "V",
}


def annotation_to_jni_descriptor(ann: object) -> str | None:
    """将 Python type annotation 转换为 JNI descriptor 字符串。

    参数：
        ann: Python 类型对象（如 JInt、int、bytes 等）。

    返回值：
        JNI descriptor 字符串（如 "I"、"[B"），
        如果类型无法映射则返回 None。
    """
    if isinstance(ann, type) and ann in _TYPE_TO_JNI:
        return _TYPE_TO_JNI[ann]
    # 检查是否是 JObject 的子类（携带 class name）
    if isinstance(ann, type) and issubclass(ann, JObject):
        return "Ljava/lang/Object;"
    return None


def extract_type_hints(func: object) -> dict[str, object]:
    """提取函数/方法的类型注解。

    使用 typing.get_type_hints() 解析前向引用，
    如果函数没有注解或解析失败则返回空 dict。

    返回值：
        dict 的 key 为参数名（"return" 表示返回值类型），
        value 为类型对象。
    """
    if not (inspect.isfunction(func) or inspect.ismethod(func)):
        return {}
    try:
        hints = typing.get_type_hints(func)
    except Exception:
        # 注解不可解析时 skip verify（向后兼容）
        return {}
    return hints


def get_return_type_jni(func: object) -> str | None:
    """获取函数返回值类型的 JNI descriptor。

    参数：
        func: 被 @java_method 标记的 Python 函数。

    返回值：
        JNI descriptor 字符串（如 "I"、"V"），
        如果返回值没有类型注解或无法映射则返回 None。
    """
    hints = extract_type_hints(func)
    ret_ann = hints.get("return")
    if ret_ann is None:
        return None
    return annotation_to_jni_descriptor(ret_ann)


def get_param_types_jni(func: object) -> list[str]:
    """获取函数参数类型的 JNI descriptor 列表。

    参数：
        func: 被 @java_method 标记的 Python 函数/方法。

    返回值：
        JNI descriptor 字符串列表，按参数顺序排列。
        排除 self 参数（方法第一个参数），
        无法映射的类型用空字符串占位（verify 阶段会失败）。
    """
    hints = extract_type_hints(func)
    params: list[str] = []
    sig = inspect.signature(func)
    for name, param in sig.parameters.items():
        if name == "return":
            continue
        # 跳过 self（instance method 的第一个参数）
        if name == "self":
            continue
        ann = hints.get(name)
        if ann is None:
            params.append("")
        else:
            jni = annotation_to_jni_descriptor(ann)
            params.append(jni if jni is not None else "")
    return params
