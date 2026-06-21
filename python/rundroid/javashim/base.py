"""JNI 对象基类。

所有 use@java_class decorator 的 Python class 都应继承 `JavaObject`。
"""


class JavaObject:
    """Java 对象基类。

    这是 Python 侧 JNI shim class 的公共基类。
    继承此类表示该 Python class 是一个 Java class 的 shim 实现。

    当前阶段（foundation）仅提供身份标记，不包含字段存储或方法表等复杂逻辑。
    """

    pass
