"""轻量 ``Emulator`` wrapper——包 ``_rundroid.Emulator`` engine。

- ``__getattr__`` 透传机器层方法（``load`` / ``call`` / ``write_guest`` / ``fs`` /
  ``seed`` / ``close``）到 engine。
- ``avm`` property 返回 ``AVM`` 门面（JNI / VM 表面）。

这样 Python 脚本经 ``rundroid.Emulator`` 拿到统一的入口，机器层直接 ``emu.*``，
Android VM 层统一 ``emu.avm.*``。
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from . import _rundroid
from .avm import AVM

if TYPE_CHECKING:
    pass


class Emulator:
    """rundroid Python 侧主入口（wrapper）。

    构造参数与底层 ``_rundroid.Emulator`` 一致：
    - ``arch``：仅支持 ``"arm64"``
    - ``backend``：仅支持 ``"unicorn"``
    - ``seed``：syscall runtime 随机种子

    机器层方法（未在本类显式定义的）经 ``__getattr__`` 透传到 engine；
    Android VM 表面经 ``avm`` property 取得。
    """

    def __init__(self, arch: str, backend: str, seed: int) -> None:
        # 底层 Rust engine（_rundroid.Emulator）
        self._engine = _rundroid.Emulator(arch, backend, seed)

    @property
    def avm(self) -> AVM:
        """Android VM 门面：JNI / VM 表面（对象构造、方法调用、field 读写等）。"""
        return AVM(self._engine)

    def __getattr__(self, name: str) -> Any:
        # TODO: 这个属于过度兜底 未来可以移除
        # 仅在正常属性查找失败时触发：透传机器层方法到 engine。
        # ``avm`` property / ``_engine`` 等已在 __dict__ / 类上，不会进到这里。
        return getattr(self._engine, name)
