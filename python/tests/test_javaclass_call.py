"""JavaClass / JavaObject 调用模型测试（python-javaclass-call change）。

覆盖：
- issue #1 链式调用 ``Signature(emu.avm).builder().signature(b).build().sign("hello")``
- 重载（同名不同 argc 按 argc 选中）
- 未知方法名抛 ``AttributeError``（防 ``__getattr__`` 递归）
- 构造不传 avm 抛 ``TypeError``（fail-fast）
- import 不修改 runtime；``avm.new_object`` 产出 ``JavaObject`` 且 ``_handle > 0``
- 方法体内 ``self._avm.new_object(Other)`` 派生独立对象
"""
from __future__ import annotations

import pytest


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
# issue #1 链式调用复刻
# ============================================================================

def test_issue1_signature_builder_chain(emu: "Emulator") -> None:
    """复刻 issue #1：Signature(emu.avm).builder().signature(b).build().sign("hello")。

    所有调用按 Java 方法名分派；方法体内借 ``self._avm`` 派生相关对象。
    """
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("com/example/Signature")
    class Signature(JavaClass):
        def __init__(self) -> None:
            self._msig = b""

        @java_method("Signature([B)V")
        def py_init_with_sig(self, sig: bytes) -> None:
            self._msig = bytes(sig)

        @java_method("sign(Ljava/lang/String;)Ljava/lang/String;")
        def py_sign(self, text: str) -> str:
            return f"signed:{text}:{self._msig.hex()}"

        @java_method("builder()Lcom/example/SignatureBuilder;")
        def py_builder(self) -> object:
            # 方法体内派生新对象：复用本实例的 _avm
            return self._avm.new_object(SignatureBuilder)

    @java_class("com/example/SignatureBuilder")
    class SignatureBuilder(JavaClass):
        def __init__(self) -> None:
            self._sig = b""

        @java_method("signature([B)Lcom/example/SignatureBuilder;")
        def py_signature(self, sig: bytes) -> object:
            self._sig = bytes(sig)
            return self

        @java_method("build()Lcom/example/Signature;")
        def py_build(self) -> object:
            s = self._avm.new_object(Signature)
            s.Signature(self._sig)  # 按 Java 名调用构造
            return s

    register(emu, [Signature, SignatureBuilder])

    out = Signature(emu.avm).builder().signature(b"\x11\x22").build().sign("hello")
    assert out == "signed:hello:1122"


# ============================================================================
# 重载（argc 解析）
# ============================================================================

def test_overload_resolved_by_argc(emu: "Emulator") -> None:
    """同名 Java 方法多个重载，按实参个数选中。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/Adder")
    class Adder(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("add(I)I")
        def add_one(self, a: int) -> int:
            return a

        @java_method("add(II)I")
        def add_two(self, a: int, b: int) -> int:
            return a + b

    register(emu, [Adder])
    obj = Adder(emu.avm)

    assert obj.add(5) == 5          # argc=1 → add_one
    assert obj.add(3, 4) == 7       # argc=2 → add_two


def test_overload_no_matching_argc_raises(emu: "Emulator") -> None:
    """实参个数不匹配任何重载 → TypeError。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/Adder2")
    class Adder(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("add(I)I")
        def add_one(self, a: int) -> int:
            return a

    register(emu, [Adder])
    obj = Adder(emu.avm)

    with pytest.raises(TypeError):
        obj.add(1, 2, 3)  # 无 argc=3 的重载


def test_overload_ambiguous_arity_picks_first(emu: "Emulator") -> None:
    """同 argc 的两个重载（首版 argc 策略限制）：选中首个匹配。

    这是首版按 argc 解析的已知限制（同 argc 不同类型会歧义），文档标注。
    两个重载参数类型不同（I vs D）但 argc 相同，dispatch 表中前者胜出。
    """
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/Ambiguous")
    class Ambiguous(JavaClass):
        def __init__(self) -> None:
            pass

        # 两个重载 argc 相同（1），参数类型不同（I vs D）；首版取首个
        @java_method("foo(I)I")
        def foo_int(self, a: int) -> int:
            return 111

        @java_method("foo(D)I")
        def foo_double(self, a: float) -> int:
            return 222  # 不会被选中（dispatch 表里 foo_int 在前）

    register(emu, [Ambiguous])
    obj = Ambiguous(emu.avm)

    # 首个匹配胜出（argc 都是 1，无法按类型区分 → 取首个）
    assert obj.foo(1) == 111


# ============================================================================
# 未知方法名 / fail-fast 构造
# ============================================================================

def test_unknown_method_raises_attribute_error(emu: "Emulator") -> None:
    """访问不在分派表里的名 → AttributeError（且不触发 __getattr__ 无限递归）。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/NoMethods")
    class NoMethods(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("known()I")
        def known(self) -> int:
            return 1

    register(emu, [NoMethods])
    obj = NoMethods(emu.avm)

    assert obj.known() == 1
    with pytest.raises(AttributeError):
        obj.does_not_exist  # noqa: B018


def test_construct_without_avm_raises_type_error(emu: "Emulator") -> None:
    """构造不传 avm → __new__ 签名不匹配 → TypeError，且不创建 VM 对象。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/NeedAvm")
    class NeedAvm(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("ping()I")
        def ping(self) -> int:
            return 1

    register(emu, [NeedAvm])

    with pytest.raises(TypeError):
        NeedAvm()  # 未传 avm


def test_avm_not_forwarded_to_init(emu: "Emulator") -> None:
    """构造时 avm 被 __new__ 剥离，不传给蓝图 __init__。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    seen_args: list[int] = []

    @java_class("test/InitArgs")
    class InitArgs(JavaClass):
        def __init__(self, sig: int) -> None:
            seen_args.append(sig)

        @java_method("ping()I")
        def ping(self) -> int:
            return 0

    register(emu, [InitArgs])
    InitArgs(emu.avm, 42)  # avm 首参，42 作为 __init__ 的 sig

    assert seen_args == [42]


# ============================================================================
# VM-backed 构造 / 身份
# ============================================================================

def test_new_object_produces_javaobject_with_handle(emu: "Emulator") -> None:
    """avm.new_object(Cls) 产出 type is JavaObject 且 _handle > 0。"""
    from rundroid.javashim import JavaClass, JavaObject, java_class, java_method, register

    @java_class("test/Plain")
    class Plain(JavaClass):
        def __init__(self) -> None:
            self._n = 7

        @java_method("n()I")
        def n(self) -> int:
            return self._n

    register(emu, [Plain])

    obj = emu.avm.new_object(Plain)
    assert type(obj) is JavaObject
    assert obj._handle > 0
    assert obj._java_class is Plain
    assert obj.n() == 7


def test_method_body_spawns_independent_object(emu: "Emulator") -> None:
    """方法体内 self._avm.new_object(Other) 派生独立对象（独立 _handle，同一 _avm）。"""
    from rundroid.javashim import JavaClass, JavaObject, java_class, java_method, register

    @java_class("test/Spawner")
    class Spawner(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("spawn()Ltest/Spawned;")
        def py_spawn(self) -> object:
            return self._avm.new_object(Spawned)

    @java_class("test/Spawned")
    class Spawned(JavaClass):
        def __init__(self) -> None:
            self.v = 0

        @java_method("set(I)V")
        def set(self, v: int) -> None:
            self.v = v

        @java_method("get()I")
        def get(self) -> int:
            return self.v

    register(emu, [Spawner, Spawned])

    parent = Spawner(emu.avm)
    child = parent.spawn()
    assert type(child) is JavaObject
    assert child._handle != parent._handle        # 独立 handle
    assert child._avm is parent._avm              # 同一 avm 引用
    assert child._java_class is Spawned

    # 两个对象状态独立
    child.set(99)
    assert child.get() == 99


def test_each_instance_has_distinct_handle(emu: "Emulator") -> None:
    """多次构造产出不同 handle（VM 每次分配新 ObjectId + global ref）。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/Distinct")
    class Distinct(JavaClass):
        def __init__(self) -> None:
            self.c = 0

        @java_method("inc()I")
        def inc(self) -> int:
            self.c += 1
            return self.c

    register(emu, [Distinct])

    a = Distinct(emu.avm)
    b = Distinct(emu.avm)
    c = Distinct(emu.avm)

    handles = {a._handle, b._handle, c._handle}
    assert len(handles) == 3  # 三个互不相同的 handle

    # 实例状态独立
    a.inc()
    a.inc()
    assert a.inc() == 3
    assert b.inc() == 1


# ============================================================================
# import 不修改 runtime
# ============================================================================

def test_class_definition_does_not_touch_runtime() -> None:
    """定义 JavaClass 子类（未 register）不触碰任何 emulator runtime state。

    __init_subclass__ 只在类上建分派表，不依赖 / 不调用任何 emulator。
    """
    from rundroid.javashim import JavaClass, java_class, java_method

    @java_class("test/MetadataOnly")
    class MetadataOnly(JavaClass):
        @java_method("foo()I")
        def foo(self) -> int:
            return 1

    # 类创建即建好分派表（纯 Python，无 emulator）
    assert "foo" in MetadataOnly.__java_dispatch__
    assert len(MetadataOnly.__java_methods__) == 1
    # 该方法对应的 _Entry 的 argc 应为 0（foo(self) 无用户参数）
    entry = MetadataOnly.__java_dispatch__["foo"][0]
    assert entry.argc == 0


# ============================================================================
# 方向 A（经 Rust bridge）回归：方法体内 new_object 不死锁
# ============================================================================

def test_method_body_new_object_via_rust_bridge_no_deadlock(emu: "Emulator") -> None:
    """经 Rust bridge（call_java_method）触发的方法体内再 new_object——死锁回归。

    修复前：``call_java_method`` 持有 ObjectStore 锁进 Python 调用，方法体内
    ``self._avm.new_object(...)`` → ``register_java_object`` 重入同一把锁 → 自锁卡死。
    修复后：锁内只 clone 出 instance 引用并立即释放锁，方法体可安全构造新对象。
    """
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/Holder")
    class Holder(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("make()I")
        def py_make(self) -> int:
            # 方法体内派生新对象（重入 objects 锁）——修复前会在此死锁
            inner = self._avm.new_object(Inner)
            inner.set(42)
            return inner.get()

    @java_class("test/Inner")
    class Inner(JavaClass):
        def __init__(self) -> None:
            self._v = 0

        @java_method("set(I)V")
        def set(self, v: int) -> None:
            self._v = v

        @java_method("get()I")
        def get(self) -> int:
            return self._v

    register(emu, [Holder, Inner])
    handle = Holder(emu.avm)._handle

    # 经 Rust bridge 触发 make()；方法体内 new_object + set + get 全部正常
    assert emu.avm.call_java_method(handle, "make()I", ()) == 42


# ============================================================================
# 同名不同签名：Python override 与 framework fallback 按 argc 分流
# ============================================================================

def test_same_name_different_signature_override_and_fallback(emu: "Emulator") -> None:
    """同名不同签名：Python override ``foo()I`` + framework stub ``foo(I)I``。

    - ``foo()I``（argc 0）：命中 Python override → 999
    - ``foo(I)I``（argc 1）：无 Python override（argc 不匹配）→ 回落 framework stub → 100

    回归 override 缓存按 ``(class, java_name, argc)`` 命中，而非仅 java_name——
    否则 ``foo(I)I`` 会被 java_name 错误拦截到 Python 路径再因 argc 不匹配抛 TypeError。
    """
    from rundroid.javashim import JavaClass, java_class, java_method, register

    # framework stub 只提供 foo(I)I
    emu.avm.register_framework_stub("test/Mix", {"foo(I)I": 100})

    # Python 只 override foo()I（同名不同签名）
    @java_class("test/Mix")
    class Mix(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("foo()I")
        def foo_noarg(self) -> int:
            return 999

    register(emu, [Mix])
    handle = Mix(emu.avm)._handle

    # foo()I：argc 0 命中 Python override
    assert emu.avm.call_java_method(handle, "foo()I", ()) == 999
    # foo(I)I：argc 1 无 Python override → 回落 framework stub
    assert emu.avm.call_java_method(handle, "foo(I)I", (5,)) == 100


def test_pure_python_chain_new_object_no_deadlock(emu: "Emulator") -> None:
    """纯 Python 方向 B 的链式 new_object 也不死锁（方向 B 不经 Rust 持锁路径）。

    与上一条互补：方向 B（obj.method() 经 __getattr__）天然不持 Rust objects 锁，
    方法体内 new_object 多层嵌套也应正常。
    """
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("test/A")
    class A(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("makeB()I")
        def py_make_b(self) -> int:
            b = self._avm.new_object(B)
            return b.makeC()

    @java_class("test/B")
    class B(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("makeC()I")
        def py_make_c(self) -> int:
            c = self._avm.new_object(C)
            return c.value()

    @java_class("test/C")
    class C(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("value()I")
        def py_value(self) -> int:
            return 7

    register(emu, [A, B, C])

    a = A(emu.avm)
    assert a.makeB() == 7  # A.makeB → new B → B.makeC → new C → C.value

