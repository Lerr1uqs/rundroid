"""python-jni-execution Phase 4 —— rich scene 端到端测试。

压测真实 Android 逆向场景下 JNI dispatch / 继承 / primitive 参数 marshalling / syscall / verbose
的交叉正确性。驱动 guest fixture ``libscene.so``：

    Emulator → register(Signer/Verifier/Crypto/Scene) → load(libscene)
             → init_jni() → jni_onload()（GetEnv + RegisterNatives）
             → call("Java_com_scene_Native_run", env_ptr, input)

guest 侧 ``Java_com_scene_Native_run`` 经 JNI 函数表 + svc syscall 完成全部覆盖项：
RegisterNatives、跨 class FindClass/GetMethodID/CallIntMethod（instance）+
GetStaticMethodID/CallStaticIntMethod（static）+ 继承（Verifier extends Signer）+
交叉依赖（Crypto.mix）+ syscall（mmap/getrandom/openat/read）+ checksum 算法。

# 确定性断言

guest 成功返回 27-bit 正 int hash；任一 syscall 失败或继承解析失败返回 0x400000xx 哨兵。
测试用同款算法在 Python 侧复算 expected——相等即转证 syscall + 继承 + 交叉调用全绿。
"""
from __future__ import annotations

from pathlib import Path
from typing import Iterator

import pytest


# ============================================================================
# Fixtures
# ============================================================================


@pytest.fixture
def emu() -> "Iterator[object]":
    """构造一个 arm64/unicorn Emulator（seed 固定 → syscall 随机源可复现），结束 close。"""
    from rundroid import Emulator

    e = Emulator("arm64", "unicorn", 42)
    yield e
    e.close()


def _scene_bytes() -> bytes:
    """读取 NDK 编译的 libscene.so（clone 后需重编，见 src/scene.c 文件头）。"""
    p = (
        Path(__file__).resolve().parent.parent.parent
        / "resources"
        / "scene"
        / "build"
        / "libscene.so"
    )
    assert p.exists(), f"libscene.so 未编译，请按 src/scene.c 文件头命令用 NDK 重编: {p}"
    return p.read_bytes()


def _register_scene_classes(emu: object) -> None:
    """注册 scene 的 4 个 guest 可见 Java 类。

    - Signer：hash(I)I instance（27-bit hash finalizer）
    - Verifier extends Signer：**不重定义 hash**（继承）+ 自有 nonce()I
    - Crypto：mix(II)I static（交叉混合）
    - Scene：verifyNative(I)I native（RegisterNatives 绑定目标，placeholder 不被分派）
    """
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("com/scene/Signer")
    class Signer(JavaClass):
        @java_method("<init>()V")
        def py_init(self: object) -> None:
            return None

        # guest 经 NewObject 产出的对象是 StubInstance（无 Python backing）——
        # wrap_python_method 对 guest instance 注入 self=None，故 override 不依赖 self。
        # 不写 ``self=None`` 默认值：注入由 Rust 侧统一完成，缺则 TypeError fail-fast。
        # descriptor 必须含 Java 方法名前缀（``methodName(args)return``），与 scene.c 的
        # GetMethodID("hash","(I)I") 对齐——dispatch 表按 Java 名分派。
        @java_method("hash(I)I")
        def hash(self: object, x: int) -> int:
            h = x & 0xFFFFFFFF
            h ^= h >> 16
            h = (h * 0x45D9F3B) & 0xFFFFFFFF
            h ^= h >> 16
            return h & 0x07FFFFFF

    @java_class("com/scene/Verifier", superclass="com/scene/Signer")
    class Verifier(JavaClass):
        # 不定义 hash —— 继承自 Signer，靠 Rust 侧 superclass 链解析覆盖。
        @java_method("<init>()V")
        def py_init(self: object) -> None:
            return None

        @java_method("nonce()I")
        def nonce(self: object) -> int:
            return 0x100

    @java_class("com/scene/Crypto")
    class Crypto(JavaClass):
        @java_method("mix(II)I")
        @staticmethod
        def mix(a: int, b: int) -> int:
            return (a ^ b ^ 0x5A5A5A5A) & 0x07FFFFFF

    @java_class("com/scene/Scene")
    class Scene(JavaClass):
        # verifyNative 的真实实现是 guest native（RegisterNatives 绑定）。bootstrap 不支持
        # 经 JNI 表分派 guest native，故此 placeholder 仅提供 MethodId 供绑定，从不被分派。
        @java_method("verifyNative(I)I")
        def verify_native(self: object, x: int) -> int:
            return 0

    # 注册顺序：Signer 先于 Verifier（superclass 名在 GetMethodID 时按名解析，先后均可，
    # 但保持父类在前便于阅读）。
    register(emu, [Signer, Verifier, Crypto, Scene])


# ============================================================================
# 确定性 hash 复算（与 scene.c 的算法严格对齐）
# ============================================================================


def _scene_hash(x: int) -> int:
    """Signer.hash 的 Python 复算（27-bit finalizer）。"""
    h = x & 0xFFFFFFFF
    h ^= h >> 16
    h = (h * 0x45D9F3B) & 0xFFFFFFFF
    h ^= h >> 16
    return h & 0x07FFFFFF


def _expected_scene(input_val: int) -> int:
    """复算 Java_com_scene_Native_run 的成功返回值。

    链路：sig_hash=Signer.hash(input)；inh_hash=继承的 hash= sig_hash；
    vnonce=Verifier.nonce()=0x100；mixed=Crypto.mix(sig_hash, vnonce)；
    combine=mixed ^ (inh_hash*31) ^ input（32-bit 无符号），& 0x07FFFFFF。
    """
    sig_hash = _scene_hash(input_val)
    inh_hash = sig_hash  # 继承 → 与 Signer.hash 同结果
    vnonce = 0x100
    mixed = (sig_hash ^ vnonce ^ 0x5A5A5A5A) & 0x07FFFFFF
    combine = (mixed ^ ((inh_hash * 31) & 0xFFFFFFFF) ^ input_val) & 0xFFFFFFFF
    return combine & 0x07FFFFFF


# ============================================================================
# 主场景：RegisterNatives + 跨 class（instance/static/继承/交叉）+ syscall + checksum
# ============================================================================


@pytest.mark.parametrize("input_val", [0, 1, 12345, 0x6789AB, 0x7FFFFFFF])
def test_scene_rich_integration(emu: object, input_val: int) -> None:
    """rich scene 端到端：确定性 hash 相等即转证 syscall + 继承 + 交叉调用全绿。"""
    _register_scene_classes(emu)
    emu.load("libscene.so", _scene_bytes())  # type: ignore[attr-defined]
    emu.init_jni()  # type: ignore[attr-defined]

    # JNI_OnLoad：GetEnv + RegisterNatives(com/scene/Scene.verifyNative)
    onload = emu.jni_onload()  # type: ignore[attr-defined]
    assert len(onload) == 1, f"应恰好 1 个模块导出 JNI_OnLoad，实际 {len(onload)}"
    _name, version = onload[0]
    assert version == 0x00010006, f"JNI_OnLoad 应返回 JNI_VERSION_1_6，实际 {version:#x}"

    result = emu.call(  # type: ignore[attr-defined]
        "Java_com_scene_Native_run", emu.jni_env_pointer, input_val  # type: ignore[attr-defined]
    )
    expected = _expected_scene(input_val)
    assert result == expected, (
        f"input={input_val:#x}: scene 应返回确定性 hash {expected:#x}，实际 {result:#x}"
        f"（若为 0x400000xx 见 scene.c 哨兵定义：syscall/继承失败）"
    )


# ============================================================================
# verbose trace（unidbg 式）—— 关键 JNI 调用经 capfd 可观测
# ----------------------------------------------------------------------------
# Rust 端 println! 写 fd 级 stdout，必须用 capfd（fd 级）而非 capsys。
# ============================================================================


def test_scene_verbose_trace(emu: object, capfd: "pytest.CaptureFixture[str]") -> None:
    """verbose 开启后，guest 的关键 JNI 调用 + RegisterNatives 在 trace 中可观测。"""
    _register_scene_classes(emu)
    emu.load("libscene.so", _scene_bytes())  # type: ignore[attr-defined]
    emu.init_jni()  # type: ignore[attr-defined]
    emu.set_jni_verbose(True)  # type: ignore[attr-defined]
    emu.jni_onload()  # type: ignore[attr-defined]
    emu.call("Java_com_scene_Native_run", emu.jni_env_pointer, 12345)  # type: ignore[attr-defined]

    out = capfd.readouterr().out

    # RegisterNatives（JNI_OnLoad 内）
    assert "RegisterNatives" in out, f"trace 缺 RegisterNatives:\n{out}"
    # 跨 class JNI 主链
    assert "JNIEnv->FindClass" in out, f"trace 缺 FindClass:\n{out}"
    assert "GetMethodID" in out, f"trace 缺 GetMethodID:\n{out}"
    assert "GetStaticMethodID" in out, f"trace 缺 GetStaticMethodID:\n{out}"
    assert "CallIntMethod" in out, f"trace 缺 CallIntMethod:\n{out}"
    assert "CallStaticIntMethod" in out, f"trace 缺 CallStaticIntMethod:\n{out}"
    # 关键 class 名出现在 trace（证明 FindClass 命中）
    assert "com/scene/Signer" in out, f"trace 缺 Signer class:\n{out}"
    assert "com/scene/Verifier" in out, f"trace 缺 Verifier class:\n{out}"
    assert "com/scene/Crypto" in out, f"trace 缺 Crypto class:\n{out}"
