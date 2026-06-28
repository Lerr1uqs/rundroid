"""python-jni-execution 最小 JNI fixture 端到端测试（Phase 2）。

验证 Python 绑定层 JNI guest execution surface 打通——把故障面收窄到
「ABI 表映射 / hook 安装 / env-vm 指针传递 / 基础 dispatch」，覆盖最小主链：

    Emulator → register(Counter) → load(jnimin) → init_jni() → jni_onload()
             → call("Java_com_jnimin_Native_run", env_ptr)

guest 侧 ``Java_com_jnimin_Native_run`` 经 JNI 函数表回调：
``FindClass → GetMethodID → NewObject → GetMethodID → CallIntMethod``，
最终命中 Python 注册的 ``com/jnimin/Counter.getValue`` override。

# 关于 guest 创建对象与 ``self=None``

guest 经 ``(*env)->NewObject`` 创建的对象在 Rust 侧是 ``StubInstance``（无 Python
backing）——单线程仿真下不会自动产出 Python ``JavaObject``。故 instance override
经 unbound 调用触发（无 ``self`` 绑定）。本 fixture 的 override 为**纯计算**（不依赖
``self``），以 ``self=None`` 默认值容忍 unbound 调用。这是 guest-created 对象的已知
边界（同 RELRO/TLS 等 bootstrap 遗留），rich scene（Phase 4）另行压测更复杂语义。
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
    """构造一个 arm64/unicorn Emulator，测试结束 close。"""
    from rundroid import Emulator

    e = Emulator("arm64", "unicorn", 42)
    yield e
    e.close()


def _jnimin_bytes() -> bytes:
    """读取 NDK 编译的 libjnimin.so（clone 后需重编，见 src/jnimin.c 文件头）。"""
    # tests/ → python/ → repo root → resources/jnimin/build/libjnimin.so
    p = (
        Path(__file__).resolve().parent.parent.parent
        / "resources"
        / "jnimin"
        / "build"
        / "libjnimin.so"
    )
    assert p.exists(), f"libjnimin.so 未编译，请按 src/jnimin.c 文件头命令用 NDK 重编: {p}"
    return p.read_bytes()


def _smoke_bytes() -> bytes:
    """读取 libsmoke.so，供多模块共存回归测试使用。"""
    p = (
        Path(__file__).resolve().parent.parent.parent
        / "resources"
        / "smoke"
        / "build"
        / "libsmoke.so"
    )
    assert p.exists(), f"libsmoke.so 未编译，请按 src/smoke.c 文件头命令用 NDK 重编: {p}"
    return p.read_bytes()


def _register_counter(emu: object) -> None:
    """注册 com/jnimin/Counter（<init> 构造器 + getValue()I 纯计算 override）。"""
    from rundroid.javashim import JavaClass, java_class, java_method, register

    @java_class("com/jnimin/Counter")
    class Counter(JavaClass):
        # guest NewObject 产出 StubInstance（无 Python backing）→ unbound 调用；
        # 纯计算 override 以 self=None 容忍 unbound（见模块 docstring）。
        @java_method("<init>()V")
        def py_init(self: object = None) -> None:
            return None

        @java_method("getValue()I")
        def get_value(self: object = None) -> int:  # type: ignore[assignment]
            return 42

    register(emu, [Counter])


# ============================================================================
# 主链：JNI_OnLoad → FindClass → GetMethodID → NewObject → CallIntMethod
# ============================================================================


def test_minimal_jni_loop(emu: object) -> None:
    """最小 JNI 主链端到端：guest 回调 Python getValue 返回 42。"""
    _register_counter(emu)

    module_id = emu.load("libjnimin.so", _jnimin_bytes())  # type: ignore[attr-defined]
    assert isinstance(module_id, int)

    # init_jni：映射 JNIEnv + JavaVM ABI 表、安装 trampoline hook、缓存指针。
    emu.init_jni()  # type: ignore[attr-defined]
    env_ptr = emu.jni_env_pointer  # type: ignore[attr-defined]
    vm_ptr = emu.java_vm_pointer  # type: ignore[attr-defined]
    assert env_ptr is not None, "init_jni 后 jni_env_pointer 必须非空"
    assert vm_ptr is not None, "init_jni 后 java_vm_pointer 必须非空"

    # JNI_OnLoad：遍历模块调 JNI_OnLoad(JavaVM*, 0)，校验返回合法 version。
    onload = emu.jni_onload()  # type: ignore[attr-defined]
    assert len(onload) == 1, f"应恰好 1 个模块导出 JNI_OnLoad，实际 {len(onload)}"
    _name, version = onload[0]
    assert version == 0x00010006, f"JNI_OnLoad 应返回 JNI_VERSION_1_6，实际 {version:#x}"

    # 主链：env_ptr 作 x0 调 Java_com_jnimin_Native_run。
    #   FindClass("com/jnimin/Counter") → GetMethodID(<init>) → NewObject
    #   → GetMethodID(getValue) → CallIntMethod → Python getValue 返回 42。
    result = emu.call("Java_com_jnimin_Native_run", env_ptr)  # type: ignore[attr-defined]
    assert result == 42, f"最小 JNI 主链应返回 42（getValue），实际 {result}"


def test_multiple_loaded_modules_keep_distinct_ids(emu: object) -> None:
    """同一 Emulator 连续 load 两个不同 so 时，两个模块必须共存且导出都可见。"""
    _register_counter(emu)

    smoke_id = emu.load("libsmoke.so", _smoke_bytes())  # type: ignore[attr-defined]
    jni_id = emu.load("libjnimin.so", _jnimin_bytes())  # type: ignore[attr-defined]

    assert smoke_id > 0, f"libsmoke.so module_id 必须为正数，实际 {smoke_id}"
    assert jni_id > 0, f"libjnimin.so module_id 必须为正数，实际 {jni_id}"
    assert smoke_id != jni_id, (
        f"两个不同模块必须拿到不同 ModuleId，实际 smoke={smoke_id}, jnimin={jni_id}"
    )

    emu.init_jni()  # type: ignore[attr-defined]
    onload = emu.jni_onload()  # type: ignore[attr-defined]
    assert len(onload) == 1, f"只有 libjnimin.so 应导出 JNI_OnLoad，实际 {len(onload)}"

    assert emu.call("rd_add", 1, 2) == 3  # type: ignore[attr-defined]
    assert emu.call(  # type: ignore[attr-defined]
        "Java_com_jnimin_Native_run", emu.jni_env_pointer  # type: ignore[attr-defined]
    ) == 42


# ============================================================================
# verbose trace（unidbg 式）
# ----------------------------------------------------------------------------
# 关键：Rust 端 `println!` 写到 fd 级 stdout（不经 Python sys.stdout），
# 故必须用 capfd（fd 级捕获）而非 capsys（sys.stdout 级）——capsys 抓不到 Rust 输出。
# ============================================================================


def test_jni_verbose_trace(emu: object, capfd: "pytest.CaptureFixture[str]") -> None:
    """verbose 开启后，guest JNI 调用打印 unidbg 式 trace，capfd 可捕获断言。"""
    _register_counter(emu)
    emu.load("libjnimin.so", _jnimin_bytes())  # type: ignore[attr-defined]
    emu.init_jni()  # type: ignore[attr-defined]
    emu.set_jni_verbose(True)  # type: ignore[attr-defined]
    emu.jni_onload()  # type: ignore[attr-defined]
    emu.call("Java_com_jnimin_Native_run", emu.jni_env_pointer)  # type: ignore[attr-defined]

    out = capfd.readouterr().out
    # verbose 行形如：[I] JNIEnv->FindClass(name="com/jnimin/Counter") => 0x...
    assert "JNIEnv->FindClass" in out, f"trace 缺 FindClass 行:\n{out}"
    assert "GetMethodID" in out, f"trace 缺 GetMethodID 行:\n{out}"
    assert "CallIntMethod" in out, f"trace 缺 CallIntMethod 行:\n{out}"
    assert "com/jnimin/Counter" in out, f"trace 缺 class 名:\n{out}"
