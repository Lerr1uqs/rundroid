"""JNI 类型标注。

这些类型用于 Python 方法注解，标记 JNI method 的参数和返回值类型。
它们不携带运行时值，仅用于 verify 阶段的严格类型匹配。
"""


class JBoolean:
    """JNI boolean 类型标注。对应 Java `boolean` / JNI `Z`。"""
    pass


class JByte:
    """JNI byte 类型标注。对应 Java `byte` / JNI `B`。"""
    pass


class JChar:
    """JNI char 类型标注。对应 Java `char` / JNI `C`。"""
    pass


class JShort:
    """JNI short 类型标注。对应 Java `short` / JNI `S`。"""
    pass


class JInt:
    """JNI int 类型标注。对应 Java `int` / JNI `I`。"""
    pass


class JLong:
    """JNI long 类型标注。对应 Java `long` / JNI `J`。"""
    pass


class JFloat:
    """JNI float 类型标注。对应 Java `float` / JNI `F`。"""
    pass


class JDouble:
    """JNI double 类型标注。对应 Java `double` / JNI `D`。"""
    pass


class JObject:
    """JNI object 类型标注。对应 Java `Object` 或任何引用类型 / JNI `L...;`。"""
    pass


class JVoid:
    """JNI void 类型标注。对应 Java `void` / JNI `V`。仅用于返回值标注。"""
    pass
