"""rundroid JNI shim foundation E2E 测试。

验证：
1. @java_class / @java_method / @java_field decorator metadata-only 行为
2. 实例化（avm.new_object）+ 构造函数 + method + field 联动
3. Signature 类的完整 JNI shim 流程

注意：本 change 起，蓝图基类为 ``JavaClass``，实例类型为 ``JavaObject``（两者分离）。
对象构造经 ``Cls(emu.avm)`` 或 ``emu.avm.new_object(Cls)``；flat JNI 调用经 ``emu.avm.*``。
"""
from __future__ import annotations

import os
import pytest

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SMOKE_SO = os.path.join(REPO_ROOT, "resources", "smoke", "build", "libsmoke.so")


# ============================================================================
# Unit tests（纯 Python metadata，无需 Emulator）
# ============================================================================

def test_java_class_decorator_attaches_name() -> None:
    from rundroid.javashim.base import JavaClass
    from rundroid.javashim.decorators import java_class

    @java_class("android/content/pm/Signature")
    class Signature(JavaClass):
        pass

    assert Signature.__java_class_name__ == "android/content/pm/Signature"


def test_java_method_decorator() -> None:
    from rundroid.javashim.decorators import java_method

    @java_method("hashCode()I")
    def hashCode(self) -> int:
        return 42

    assert hashCode.__java_method_descriptor__ == "hashCode()I"


def test_java_field_decorator_with_name_sig() -> None:
    from rundroid.javashim.decorators import java_field

    @java_field(name="mSignature", sig="[B")
    def _field_sig() -> None:
        pass

    assert _field_sig.__java_field_descriptor__ == "mSignature:[B"


def test_java_field_with_initial_value() -> None:
    """验证 @java_field 的 initial 参数正确传递。"""
    from rundroid.javashim.decorators import java_field

    @java_field(name="count", sig="I", initial=100)
    def _field_count() -> None:
        pass

    assert _field_count.__java_field_descriptor__ == "count:I"
    assert _field_count.__java_field_value__ == 100


def test_import_does_not_register() -> None:
    from rundroid.javashim.base import JavaClass
    from rundroid.javashim.decorators import java_class, java_method

    @java_class("test/ImportClass")
    class ImportClass(JavaClass):
        @java_method("test()I")
        def test(self) -> int:
            return 0

    assert ImportClass.__java_class_name__ == "test/ImportClass"
    # 类创建即建好分派表（不依赖 register / emulator）
    assert "test" in ImportClass.__java_dispatch__


def test_register_without_java_class_fails() -> None:
    """验证缺少 @java_class 的 class 注册时 register() 抛出 ValueError。"""
    from rundroid.javashim.base import JavaClass
    from rundroid.javashim.registry import register
    from rundroid import Emulator

    class NoDecorator(JavaClass):
        pass

    emu = Emulator("arm64", "unicorn", 42)
    try:
        with pytest.raises(ValueError, match="@java_class"):
            register(emu, [NoDecorator])
    finally:
        emu.close()


def test_staticmethod_method_collection() -> None:
    """验证 @staticmethod + @java_method 的方法被正确收集为 static。"""
    from rundroid.javashim.base import JavaClass
    from rundroid.javashim.decorators import java_class, java_method
    from rundroid.javashim.registry import register
    from rundroid import Emulator

    @java_class("test/StaticMethodClass")
    class StaticMethodClass(JavaClass):
        @java_method("staticHashCode()I")
        @staticmethod
        def static_hash_code() -> int:
            return 0x1234

    emu = Emulator("arm64", "unicorn", 42)
    try:
        register(emu, [StaticMethodClass])
        # 验证方法已注册（不会抛异常）
    finally:
        emu.close()

    # __java_methods__ 由 __init_subclass__ 在类创建时建好
    assert len(StaticMethodClass.__java_methods__) == 1  # type: ignore[attr-defined]
    assert StaticMethodClass.__java_methods__[0][3] is True  # type: ignore[attr-defined]  # is_static
    # 静态方法不进 Python 侧分派表（走 guest→Python 方向 A）
    assert "staticHashCode" not in StaticMethodClass.__java_dispatch__


# ============================================================================
# Fixtures
# ============================================================================

@pytest.fixture
def emu() -> "Emulator":
    from rundroid import Emulator
    e = Emulator("arm64", "unicorn", 42)
    yield e
    e.close()


@pytest.fixture
def emu_with_smoke() -> "Emulator":
    from rundroid import Emulator
    e = Emulator("arm64", "unicorn", 42)
    e.load("smoke", open(SMOKE_SO, "rb").read())
    yield e
    e.close()


# ============================================================================
# E2E 测试
# ============================================================================

def test_pure_export_call(emu_with_smoke: "Emulator") -> None:
    assert emu_with_smoke.call("rd_add", 1, 2) == 3


def test_signature_full_jni_flow(emu: "Emulator") -> None:
    """完整 Signature JNI shim 流程。

    模拟 android/content/pm/Signature：
    1. 注册 class
    2. 实例化（avm.new_object）
    3. 调用构造函数 Signature([B)V
    4. 调用 hashCode()I
    5. 读取 field mSignature
    """
    from rundroid.javashim.base import JavaClass
    from rundroid.javashim.decorators import java_class, java_method, java_field
    from rundroid.javashim.registry import register

    @java_class("android/content/pm/Signature")
    class Signature(JavaClass):

        def __init__(self) -> None:
            self.mSignature = bytes([])  # Java field: mSignature

        @java_method("Signature([B)V")
        def signature_init(self, sig: bytes) -> None:
            self.mSignature = bytes(sig)

        @java_method("hashCode()I")
        def hash_code(self) -> int:
            h = 0
            for b in self.mSignature:
                h = (h * 31 + b) & 0xFFFFFFFF
            return h

        @java_method("describeContents()I")
        def describe_contents(self) -> int:
            return 0

        @java_method("getSignature()[B")
        def get_signature(self) -> bytes:
            return self.mSignature

        @java_field(name="mSignature", sig="[B")
        def _field_signature() -> None:
            pass

    register(emu, [Signature])

    # 实例化经 avm.new_object（或等价的 Signature(emu.avm)）
    obj = emu.avm.new_object(Signature)
    handle = obj._handle
    assert handle > 0

    test_sig = b"\x01\x02\x03\x04"
    emu.avm.call_java_method(handle, "Signature([B)V", (test_sig,))

    r = emu.avm.call_java_method(handle, "describeContents()I", ())
    assert r == 0

    expected_hash = ((((0 * 31 + 1) * 31 + 2) * 31 + 3) * 31 + 4) & 0xFFFFFFFF
    r = emu.avm.call_java_method(handle, "hashCode()I", ())
    assert r == expected_hash

    # 通过 Java field 名 mSignature 读取（与 @java_field 声明的 name 一致）
    sig_bytes = emu.avm.read_instance_field(handle, "mSignature")
    assert sig_bytes == test_sig

    returned_sig = emu.avm.call_java_method(handle, "getSignature()[B", ())
    assert returned_sig == test_sig


def test_counter_instance_flow(emu: "Emulator") -> None:
    """Counter 实例：field count + method increment。

    模拟 java/util/concurrent/atomic/AtomicInteger：
    - __init__ 设 count = 0
    - getAndIncrement()I 返回当前值并 +1
    - get()I 返回当前值
    """
    from rundroid.javashim.base import JavaClass
    from rundroid.javashim.decorators import java_class, java_method, java_field
    from rundroid.javashim.registry import register

    @java_class("java/util/concurrent/atomic/AtomicInteger")
    class AtomicInteger(JavaClass):
        def __init__(self) -> None:
            self.count = 0  # Java field: count

        @java_method("AtomicInteger(I)V")
        def init_with_value(self, initial: int) -> None:
            self.count = initial

        @java_method("getAndIncrement()I")
        def get_and_increment(self) -> int:
            val = self.count
            self.count = val + 1
            return val

        @java_method("get()I")
        def get(self) -> int:
            return self.count

        @java_field(name="count", sig="I", initial=0)
        def _field_count() -> None:
            pass

    register(emu, [AtomicInteger])

    obj = AtomicInteger(emu.avm)
    h = obj._handle

    assert emu.avm.call_java_method(h, "getAndIncrement()I", ()) == 0
    assert emu.avm.call_java_method(h, "getAndIncrement()I", ()) == 1
    assert emu.avm.call_java_method(h, "getAndIncrement()I", ()) == 2
    assert emu.avm.call_java_method(h, "get()I", ()) == 3
    # 通过 Java field 名 count 读取
    assert emu.avm.read_instance_field(h, "count") == 3


def test_multiple_instances(emu: "Emulator") -> None:
    """多个同一 class 的实例独立运行。"""
    from rundroid.javashim.base import JavaClass
    from rundroid.javashim.decorators import java_class, java_method
    from rundroid.javashim.registry import register

    @java_class("test/Counter")
    class Counter(JavaClass):
        def __init__(self) -> None:
            self.count = 0

        @java_method("increment()I")
        def increment(self) -> int:
            self.count += 1
            return self.count

        @java_method("get()I")
        def get(self) -> int:
            return self.count

    register(emu, [Counter])

    h1 = Counter(emu.avm)._handle
    h2 = Counter(emu.avm)._handle

    for expected in [1, 2, 3]:
        assert emu.avm.call_java_method(h1, "increment()I", ()) == expected

    assert emu.avm.call_java_method(h2, "increment()I", ()) == 1
    assert emu.avm.call_java_method(h1, "get()I", ()) == 3
    assert emu.avm.call_java_method(h2, "get()I", ()) == 1


def test_release_java_instance(emu: "Emulator") -> None:
    """验证 release_java_instance 清理实例。"""
    from rundroid.javashim.base import JavaClass
    from rundroid.javashim.decorators import java_class, java_method
    from rundroid.javashim.registry import register

    @java_class("test/CloseTest")
    class CloseTest(JavaClass):
        def __init__(self) -> None:
            self.val = 0

        @java_method("get()I")
        def get(self) -> int:
            return self.val

    register(emu, [CloseTest])

    h = CloseTest(emu.avm)._handle
    assert emu.avm.call_java_method(h, "get()I", ()) == 0

    emu.avm.release_java_instance(h)

    with pytest.raises(Exception):
        emu.avm.call_java_method(h, "get()I", ())


# ============================================================================
# Override 与 strictness 测试
# ============================================================================


def test_python_override_beats_framework_stub(emu: "Emulator") -> None:
    """Python shim override 覆盖已注册的 framework stub。

    优先级验证：
    1. 先注册 framework stub（模拟 Rust builtin）
    2. 再注册 Python shim（覆盖 frameworkValue，新增 pythonOnly）
    3. 验证 Python override 优先，framework stub 回落
    """
    from rundroid.javashim.base import JavaClass
    from rundroid.javashim.decorators import java_class, java_method
    from rundroid.javashim.registry import register

    # Step 1: 注册 framework stub class
    emu.avm.register_framework_stub("test/SharedClass", {
        "frameworkValue()I": 100,
        "frameworkOnly()I": 200,
    })

    # Step 2: 注册 Python shim（覆盖 frameworkValue，添加 pythonOnly）
    @java_class("test/SharedClass")
    class OverrideClass(JavaClass):
        def __init__(self) -> None:
            pass

        @java_method("frameworkValue()I")
        def framework_value(self) -> int:
            return 999  # Python override 值

        @java_method("pythonOnly()I")
        def python_only(self) -> int:
            return 300

    register(emu, [OverrideClass])
    handle = OverrideClass(emu.avm)._handle

    # Python override 生效
    assert emu.avm.call_java_method(handle, "frameworkValue()I", ()) == 999

    # Framework-only method 仍然可用（回落）
    assert emu.avm.call_java_method(handle, "frameworkOnly()I", ()) == 200

    # Python-only method 可用
    assert emu.avm.call_java_method(handle, "pythonOnly()I", ()) == 300


def test_bad_annotation_fails_at_registration(emu: "Emulator") -> None:
    """注解与 descriptor 不匹配在注册阶段 fail-fast。

    descriptor 声明返回值 I(int)，但 Python type hint 声明 str →
    register() 抛出 ValueError。
    """
    from rundroid.javashim.base import JavaClass
    from rundroid.javashim.decorators import java_class, java_method
    from rundroid.javashim.registry import register

    @java_class("test/BadAnnotation")
    class BadClass(JavaClass):
        def __init__(self) -> None:
            pass

        # descriptor 说返回值 I(int)，但注解是 str → 不匹配
        @java_method("badMethod()I")
        def bad_method(self) -> str:
            return "hello"

    with pytest.raises((ValueError, TypeError)):
        register(emu, [BadClass])
