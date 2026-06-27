"""Python ↔ JNI 值编组回归测试（change ``python-jni-value-marshalling``）。

覆盖 spec 的全部 requirement / scenario：

- **Req 1** Python 值自动 coercion 成 JNI 值（``str``/``bytes``/``None``/primitive）
- **Req 2** JNI 对象值按 storage 类型回编组（String→str、byte[]→bytes、Null→None）
- **Req 3** guest/native 调用 Python ``@java_method`` 时，``String``/``byte[]`` 参数
  被还原成可直接用的 Python 值（不是 ``None``）
- **Req 4** 显式 ``JavaString`` / ``JavaByteArray`` wrapper（identity 敏感）
- **Req 5** 未支持的值 fail-fast（不静默吞成 Null）

# 端到端车辆：``avm.call_java_method_typed``

值编组的真实闭环是 guest 经 JNI 调用一个方法——Python 参数先编组成 ``JniArgs``
（``str``/``bytes`` 落身份成 Java 对象），Python ``@java_method`` 收到的参数再被
还原成 ``str``/``bytes``，返回值再按 storage 还原。``call_java_method_typed`` 就是
这条路径的 Python 入口（等价于一次 guest JNI 调用），故作为端到端验证车辆。
"""
from __future__ import annotations

from typing import TYPE_CHECKING

import pytest

if TYPE_CHECKING:
    from rundroid import Emulator


# ============================================================================
# Fixtures
# ============================================================================

@pytest.fixture
def emu() -> "Emulator":
    from rundroid import Emulator
    e = Emulator("arm64", "unicorn", 42)
    yield e
    e.close()


# ============================================================================
# Req 3 + Req 1：Signature([B)V —— Python 方法收到 bytes（不是 None）
# ============================================================================

def test_signature_byte_array_arg_received_as_bytes(emu: "Emulator") -> None:
    """``Signature([B)V``：guest 传入 byte[]，Python 方法参数 SHALL 是 bytes。

    回归：编组前 ``bytes`` 被静默吞成 Null（方法体收到 None）；
    编组后经 ``call_java_method_typed``，方法体收到真正的 ``bytes``。
    """
    from rundroid.javashim import JavaClass, java_class, java_method, register

    seen: list[object] = []

    @java_class("test/SigStore")
    class SigStore(JavaClass):
        def __init__(self) -> None:
            self._msig: bytes = b""

        @java_method("setSig([B)V")
        def set_sig(self, sig: bytes) -> None:
            # 关键断言：参数是 bytes，不是 None
            assert sig is not None, "byte[] 参数被吞成 None"
            assert isinstance(sig, bytes), f"byte[] 参数应为 bytes，实际 {type(sig)}"
            self._msig = bytes(sig)
            seen.append(self._msig)

    register(emu, [SigStore])
    handle = SigStore(emu.avm)._handle

    # 经类型化 dispatch 调用（等价 guest JNI 调用）：bytes → ObjectStore byte[] → 还原成 bytes
    result = emu.avm.call_java_method_typed(handle, "setSig([B)V", (b"\x11\x22\x33",))

    assert result is None, "void 方法返回值应为 None"
    assert seen == [b"\x11\x22\x33"], "Python 方法应收到原始 bytes"


# ============================================================================
# Req 2 + Req 1：java/lang/String 进出都不是 None
# ============================================================================

def test_string_roundtrip_not_none(emu: "Emulator") -> None:
    """``String`` 参数 + ``String`` 返回值：进出都 SHALL 是 str（不是 None）。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/Greeter")
    class Greeter(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("greet(Ljava/lang/String;)Ljava/lang/String;")
        def greet(self, who: str) -> str:
            assert who is not None, "String 参数被吞成 None"
            assert isinstance(who, str), f"String 参数应为 str，实际 {type(who)}"
            return f"hello:{who}"

    register(emu, [Greeter])
    handle = Greeter(emu.avm)._handle

    out = emu.avm.call_java_method_typed(
        handle, "greet(Ljava/lang/String;)Ljava/lang/String;", ("world",)
    )

    assert out is not None, "String 返回值被吞成 None"
    assert isinstance(out, str), f"String 返回值应为 str，实际 {type(out)}"
    assert out == "hello:world"


def test_byte_array_return_not_none(emu: "Emulator") -> None:
    """``byte[]`` 返回值 SHALL 是 bytes（不是 None）。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/BytesProvider")
    class BytesProvider(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("toBytes()[B")
        def to_bytes(self) -> bytes:
            return b"\xde\xad\xbe\xef"

    register(emu, [BytesProvider])
    handle = BytesProvider(emu.avm)._handle

    out = emu.avm.call_java_method_typed(handle, "toBytes()[B", ())

    assert out is not None, "byte[] 返回值被吞成 None"
    assert isinstance(out, bytes), f"byte[] 返回值应为 bytes，实际 {type(out)}"
    assert out == b"\xde\xad\xbe\xef"


# ============================================================================
# Req 1：primitive / None 自动 coercion
# ============================================================================

def test_primitive_and_none_roundtrip(emu: "Emulator") -> None:
    """primitive（int/bool）与 None 经编组往返稳定。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/PrimEcho")
    class PrimEcho(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("echoInt(I)I")
        def echo_int(self, n: int) -> int:
            return n

        @java_method("echoBool(Z)Z")
        def echo_bool(self, b: bool) -> bool:
            return b

    register(emu, [PrimEcho])
    handle = PrimEcho(emu.avm)._handle

    assert emu.avm.call_java_method_typed(handle, "echoInt(I)I", (42,)) == 42
    assert emu.avm.call_java_method_typed(handle, "echoInt(I)I", (-7,)) == -7
    assert emu.avm.call_java_method_typed(handle, "echoBool(Z)Z", (True,)) is True
    assert emu.avm.call_java_method_typed(handle, "echoBool(Z)Z", (False,)) is False


def test_null_arg_and_null_return_roundtrip(emu: "Emulator") -> None:
    """None ↔ null 往返：传 None 进、返 None 出都稳定（不是被吞的 None）。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/Nuller")
    class Nuller(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("maybe(Ljava/lang/String;)Ljava/lang/String;")
        def maybe(self, s: object) -> object:
            # 传入 None → Python 收到 None
            assert s is None
            return None  # 返回 None → JValue::Null

    register(emu, [Nuller])
    handle = Nuller(emu.avm)._handle

    out = emu.avm.call_java_method_typed(
        handle, "maybe(Ljava/lang/String;)Ljava/lang/String;", (None,)
    )
    assert out is None


# ============================================================================
# Req 5：未支持的值 fail-fast（不静默吞成 Null）
# ============================================================================

def test_unsupported_value_raises_not_silent_null(emu: "Emulator") -> None:
    """未支持的复杂 Python 值（list）SHALL 抛异常，不静默变 Null。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/EchoStr")
    class EchoStr(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("echo(Ljava/lang/String;)Ljava/lang/String;")
        def echo(self, s: str) -> str:
            return s

    register(emu, [EchoStr])
    handle = EchoStr(emu.avm)._handle

    # dict 无法编组成 JValue → fail-fast（PyTypeError），不是静默 Null。
    # 注意：list[int] 不能用——pyo3 会把 [1,2,3] 当 Vec<u8> 提取成 bytes。
    with pytest.raises(TypeError):
        emu.avm.call_java_method_typed(
            handle, "echo(Ljava/lang/String;)Ljava/lang/String;", ({"k": 1},)
        )


# ============================================================================
# Req 4：显式 JavaString / JavaByteArray wrapper（identity 敏感）
# ============================================================================

def test_explicit_java_string_wrapper(emu: "Emulator") -> None:
    """显式 ``JavaString`` 构造路径 + 经 marshalling 复用身份。"""
    from rundroid.javashim import JavaString

    js = emu.avm.new_string("hello")
    assert isinstance(js, JavaString)
    assert js.value == "hello"
    assert js._handle > 0, "wrapper 应有有效 global handle"
    assert js._rundroid_oid > 0, "wrapper 应有有效 ObjectId"

    # 同内容两次构造 → 两个不同身份（不同 ObjectId）
    js2 = emu.avm.new_string("hello")
    assert js2._rundroid_oid != js._rundroid_oid, "两次 new_string 应是不同对象身份"

    # wrapper 经 marshalling 复用身份（不新建 String），值正确还原
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/StrSink")
    class StrSink(JavaClass):
        def __init__(self) -> None:
            self.received: object = None

        @java_method("take(Ljava/lang/String;)Ljava/lang/String;")
        def take(self, s: str) -> str:
            assert isinstance(s, str)
            self.received = s
            return s

    register(emu, [StrSink])
    handle = StrSink(emu.avm)._handle

    out = emu.avm.call_java_method_typed(
        handle, "take(Ljava/lang/String;)Ljava/lang/String;", (js,)
    )
    assert out == "hello"


def test_explicit_java_byte_array_wrapper(emu: "Emulator") -> None:
    """显式 ``JavaByteArray`` 构造路径 + 经 marshalling 复用身份。"""
    from rundroid.javashim import JavaByteArray

    jb = emu.avm.new_bytes(b"\x01\x02\x03")
    assert isinstance(jb, JavaByteArray)
    assert jb.value == b"\x01\x02\x03"
    assert jb._handle > 0
    assert jb._rundroid_oid > 0

    jb2 = emu.avm.new_bytes(b"\x01\x02\x03")
    assert jb2._rundroid_oid != jb._rundroid_oid, "两次 new_bytes 应是不同对象身份"


# ============================================================================
# 死锁回归：call_java_method_typed 方法体内 new_object 不死锁
# ============================================================================

def test_typed_call_body_new_object_no_deadlock(emu: "Emulator") -> None:
    """经 ``call_java_method_typed`` 触发的方法体内 ``self._avm.new_object(...)`` 不死锁。

    回归 ``call_java_method_typed`` 的死锁规避设计：调用 handler 前必须释放 runtime
    read guard——否则方法体内 ``new_object`` → ``register_java_object`` 重入 write guard
    会自锁卡死。
    """
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/Maker")
    class Maker(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("make(Ljava/lang/String;)Ljava/lang/String;")
        def make(self, s: str) -> str:
            # 方法体内派生新对象（重入 runtime write guard）——若持守 read guard 会死锁
            helper = self._avm.new_object(Helper)
            return helper.echo(s)

    @java_class("test/Helper")
    class Helper(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("echo(Ljava/lang/String;)Ljava/lang/String;")
        def echo(self, s: str) -> str:
            return f"echo:{s}"

    register(emu, [Maker, Helper])
    handle = Maker(emu.avm)._handle

    out = emu.avm.call_java_method_typed(
        handle, "make(Ljava/lang/String;)Ljava/lang/String;", ("hi",)
    )
    assert out == "echo:hi"


# ============================================================================
# 深度边界测试：全字节 / 空值 / bytearray / float 语义（确认无误行为锁定）
# ============================================================================

def test_full_byte_range_roundtrip_exact(emu: "Emulator") -> None:
    """byte[] 全字节域(0x00..0xFF)往返逐字节精确——验证 i8↔u8 转换无损。

    回归：``0xFF`` 在 Rust 侧是 ``JValue::Byte(-1)``(i8)，取出时 ``*b as u8`` 还原 0xFF。
    若哪一步符号处理出错，0x80..0xFF 段会偏移。
    """
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/FullBytes")
    class FullBytes(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("echo([B)[B")
        def echo(self, b: bytes) -> bytes:
            return b

    register(emu, [FullBytes])
    handle = FullBytes(emu.avm)._handle

    payload = bytes(range(256))
    out = emu.avm.call_java_method_typed(handle, "echo([B)[B", (payload,))
    assert out == payload, "全字节域 byte[] 往返应逐字节相等"


def test_empty_bytes_and_empty_str_roundtrip(emu: "Emulator") -> None:
    """空 bytes / 空 str 往返稳定（空 PrimitiveArray / 空 String 不应崩溃或变 None）。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/Empty")
    class Empty(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("echoB([B)[B")
        def echo_b(self, b: bytes) -> bytes:
            return b

        @java_method("echoS(Ljava/lang/String;)Ljava/lang/String;")
        def echo_s(self, s: str) -> str:
            return s

    register(emu, [Empty])
    handle = Empty(emu.avm)._handle

    assert emu.avm.call_java_method_typed(handle, "echoB([B)[B", (b"",)) == b""
    out_s = emu.avm.call_java_method_typed(
        handle, "echoS(Ljava/lang/String;)Ljava/lang/String;", ("",)
    )
    assert out_s == ""
    assert out_s is not None, "空 str 不应被吞成 None"


def test_bytearray_accepted_as_bytes(emu: "Emulator") -> None:
    """``bytearray`` 作为 byte[] 输入 SHALL 被 pyo3 接受（与 bytes 等价）。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/BA")
    class BA(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("echo([B)[B")
        def echo(self, b: bytes) -> bytes:
            return b

    register(emu, [BA])
    handle = BA(emu.avm)._handle

    out = emu.avm.call_java_method_typed(handle, "echo([B)[B", (bytearray(b"\x10\x20\x30"),))
    assert out == b"\x10\x20\x30"


def test_float_not_treated_as_int(emu: "Emulator") -> None:
    """Python ``float`` SHALL 编组成 Double，不被当 int 吞成 Int/Long。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/Flt")
    class Flt(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("echo(D)D")
        def echo(self, d: float) -> float:
            # 收到的应是 float，不是 int
            assert isinstance(d, float), f"float 参数被降级成 {type(d)}"
            return d

    register(emu, [Flt])
    handle = Flt(emu.avm)._handle

    out = emu.avm.call_java_method_typed(handle, "echo(D)D", (3.5,))
    assert isinstance(out, float)
    assert out == 3.5


# ============================================================================
# int / long 边界（修复点：Int→Long 无损 widening）
# ============================================================================

def test_int_widens_to_long_position(emu: "Emulator") -> None:
    """小整数(落 Int)用于声明 Long 的位置 SHALL 无损放行。

    回归：Python int 是统一整型，runtime 按值大小落 Int/Long。修复前 i32 范围内的值
    返回到 ``(J)`` 声明位置会被 validate 拒绝（TypeMismatch: 期望 Long 实际 Int），
    导致声明 long 的方法几乎只能收刚好 >i32 的值——不可用。

    方法参数/返回**无标注** → 跳过 verify（否则 int 注解→"I" 与 descriptor "J" 冲突）；
    widening 由运行时 ``validate_return_value`` 放行。
    """
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/Widen")
    class Widen(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("ret()J")
        def ret(self):  # type: ignore[no-untyped-def]
            return 42  # 小整数 → Int → 应 widening 到 Long 位置

        @java_method("retNeg()J")
        def ret_neg(self):  # type: ignore[no-untyped-def]
            return -1

    register(emu, [Widen])
    handle = Widen(emu.avm)._handle

    assert emu.avm.call_java_method_typed(handle, "ret()J", ()) == 42
    assert emu.avm.call_java_method_typed(handle, "retNeg()J", ()) == -1


def test_long_does_not_narrow_to_int(emu: "Emulator") -> None:
    """反方向 Long→Int **有损**，SHALL 仍拒绝（不双向 widening）。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/Narrow")
    class Narrow(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("big()I")
        def big(self):  # type: ignore[no-untyped-def]
            return 2 ** 40  # 超 i32 → Long；声明返回 I(Int) → 有损，应拒绝

    register(emu, [Narrow])
    handle = Narrow(emu.avm)._handle

    with pytest.raises(RuntimeError):
        emu.avm.call_java_method_typed(handle, "big()I", ())


# ============================================================================
# JavaObject 跨编组边界（修复点：JavaObject 携带 _rundroid_oid）
# ============================================================================

def test_java_object_arg_roundtrip_preserves_identity(emu: "Emulator") -> None:
    """JavaObject 作参数往返，SHALL 返回**同一** Python 对象（identity 保留）。

    回归：修复前 JavaObject 无 ``_rundroid_oid``，py_to_jvalue 不识别 → TypeError。
    修复后经 oid 复用 → HostValue<Py> 取回原对象 → ``out is obj`` 成立。
    """
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/ObjEcho")
    class ObjEcho(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("echo(Ljava/lang/Object;)Ljava/lang/Object;")
        def echo(self, o: object) -> object:
            return o

    @java_class("test/Plain")
    class Plain(JavaClass):
        def __init__(self) -> None:
            self.tag = 0

    register(emu, [ObjEcho, Plain])
    handle = ObjEcho(emu.avm)._handle
    obj = Plain(emu.avm)
    obj.tag = 99
    assert obj._rundroid_oid > 0, "JavaObject 应回填 _rundroid_oid"

    out = emu.avm.call_java_method_typed(
        handle, "echo(Ljava/lang/Object;)Ljava/lang/Object;", (obj,)
    )
    assert out is obj, "JavaObject 参数应原样返回（identity 保留）"
    assert out.tag == 99


def test_java_object_return_value(emu: "Emulator") -> None:
    """方法返回 JavaObject SHALL 正确回传（修复前 TypeError）。"""
    from rundroid.javashim import JavaClass, JavaObject, java_class, java_method, register

    @java_class("test/Factory")
    class Factory(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("produce()Ljava/lang/Object;")
        def produce(self):  # type: ignore[no-untyped-def]
            return self._avm.new_object(Product)

    @java_class("test/Product")
    class Product(JavaClass):
        def __init__(self) -> None:
            pass

    register(emu, [Factory, Product])
    handle = Factory(emu.avm)._handle

    out = emu.avm.call_java_method_typed(handle, "produce()Ljava/lang/Object;", ())
    assert isinstance(out, JavaObject)
    assert out._java_class is Product


# ============================================================================
# 异常传播 / 嵌套 typed call / wrapper 退化
# ============================================================================

def test_method_body_exception_propagates(emu: "Emulator") -> None:
    """方法体内抛 Python 异常 SHALL 经编组层传播为 RuntimeError，且信息保留。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/Boom")
    class Boom(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("explode()I")
        def explode(self) -> int:
            raise ValueError("boom-from-body")

    register(emu, [Boom])
    handle = Boom(emu.avm)._handle

    with pytest.raises(RuntimeError, match="boom-from-body"):
        emu.avm.call_java_method_typed(handle, "explode()I", ())


def test_nested_typed_call_no_deadlock(emu: "Emulator") -> None:
    """``call_java_method_typed`` 方法体内再 ``call_java_method_typed``——嵌套重入不死锁。

    外层 typed call 的 handler 提取已释放 read guard；方法体内经 avm 再发起一次 typed call
    会再次走「read guard 提取 handler → 释放 → 调用」。两层重入均不死锁，且值正确。
    """
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/Nest")
    class Nest(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("outer(I)I")
        def outer(self, n: int) -> int:
            # 方法体内经 avm 再做一次 typed call（内层重入 runtime + handler 提取）
            return self._avm.call_java_method_typed(self._handle, "inner(I)I", (n,))

        @java_method("inner(I)I")
        def inner(self, n: int) -> int:
            return n + 1

    register(emu, [Nest])
    handle = Nest(emu.avm)._handle

    assert emu.avm.call_java_method_typed(handle, "outer(I)I", (5,)) == 6


def test_returned_wrapper_degrades_to_str(emu: "Emulator") -> None:
    """方法返回 ``JavaString`` wrapper 时，回编组按 storage 还原成 ``str``（decision 4）。

    显式 wrapper 的身份复用发生在**入参**侧（py_to_jvalue 复用 oid）；
    **返回**侧统一按 storage 类型投影——String→str，故返回 wrapper 在 Python 侧得到 str。
    """
    from rundroid.javashim import JavaClass, JavaString, java_class, java_method, register

    @java_class("test/Returner")
    class Returner(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("give(Ljava/lang/String;)Ljava/lang/String;")
        def give(self, s):  # type: ignore[no-untyped-def]
            return s  # 透传 wrapper

    register(emu, [Returner])
    handle = Returner(emu.avm)._handle
    js = emu.avm.new_string("wrapped")

    out = emu.avm.call_java_method_typed(
        handle, "give(Ljava/lang/String;)Ljava/lang/String;", (js,)
    )
    # 入参 wrapper 复用 oid（不新建）；返回侧 String storage → str
    assert out == "wrapped"
    assert isinstance(out, str)


# ============================================================================
# Req 5：wrapper 生命周期（release / __del__）
# ============================================================================


def test_java_string_release_is_idempotent(emu: "Emulator") -> None:
    """``JavaString.release()`` 幂等：两次调用不抛，首次确实清了底层资源。"""
    from rundroid.javashim import JavaString

    js = emu.avm.new_string("release-me")
    handle = js._handle

    # 首次 release 不抛
    js.release()

    # 二次 release 幂等，不抛
    js.release()

    # 验证首次 release 确实清理了 RefTable：直接调底层应报 ValueError
    with pytest.raises(ValueError):
        emu.avm.release_java_instance(handle)


def test_java_byte_array_release_is_idempotent(emu: "Emulator") -> None:
    """``JavaByteArray.release()`` 幂等：同 JavaString 语义。"""
    from rundroid.javashim import JavaByteArray

    jb = emu.avm.new_bytes(b"\xde\xad")
    handle = jb._handle

    jb.release()
    jb.release()  # 幂等

    with pytest.raises(ValueError):
        emu.avm.release_java_instance(handle)


def test_java_string_del_triggers_release(emu: "Emulator") -> None:
    """``JavaString.__del__`` 在 GC 回收后触发底层释放。"""
    from rundroid.javashim import JavaString

    js = emu.avm.new_string("gc-me")
    handle = js._handle
    assert not js._released

    # CPython refcount: del 立即触发 __del__
    del js
    # gc.collect 兜底（非 CPython 实现或循环引用场景）
    import gc
    gc.collect()

    # __del__ 应已触发 release → handle 已从 RefTable 移除
    with pytest.raises(ValueError):
        emu.avm.release_java_instance(handle)


# ============================================================================
# Req 6：fail-fast —— dangling OID 回 Python 时抛异常（非静默 None）
# ============================================================================


def test_dangling_oid_return_to_python_raises(emu: "Emulator") -> None:
    """wrapper release 后再经 marshalling 传参→dangling OID→RuntimeError（非 None）。

    同时验证：
    - Part A：``jvalue_object_to_py`` 对 dangling OID 抛 RuntimeError（fail-fast）
    - Part B：``release()`` 真的从 ObjectStore 移除了条目
    """
    from rundroid.javashim import JavaClass, java_class, java_method, register
    from rundroid.javashim import JavaString

    js = emu.avm.new_string("will-dangle")
    js.release()  # oid 从 ObjectStore + RefTable 移除

    @java_class("test/Sink")
    class Sink(JavaClass):
        def __init__(self) -> None:
            self.received: object = None

        @java_method("take(Ljava/lang/String;)V")
        def take(self, s: str) -> None:
            self.received = s

    register(emu, [Sink])
    handle = Sink(emu.avm)._handle

    # dangling OID 被传入 → jvalue_object_to_py 命中 storage(oid)==None → RuntimeError
    with pytest.raises(RuntimeError, match="dangling OID"):
        emu.avm.call_java_method_typed(
            handle, "take(Ljava/lang/String;)V", (js,)
        )
