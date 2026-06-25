"""JNI 对象蓝图与实例类型。

把 Python shim 的「类」与「实例」建模为两个真实类型：

- ``JavaClass`` —— 蓝图基类（用户写 ``class Signature(JavaClass)``）。
  类创建时（``__init_subclass__``）扫描 ``@java_method`` 元数据，产出两份结构：
    * ``__java_dispatch__`` —— dict[java_name → list[_Entry]]，供实例 ``__getattr__`` 分派
    * ``__java_methods__``  —— list[(py_name, desc, fn, is_static)]，供 ``register()`` 喂 Rust
  构造时显式传 avm 作首参（``Signature(avm, ...)``），委托给 ``avm.new_object``。

- ``JavaObject`` —— 实例类型（**非** ``JavaClass`` 别名）。
  携带 ``_java_class``（蓝图）/``_avm``（构造时传入）/``_handle``（VM 句柄）+ 用户字段。
  ``__getattr__`` 按 Java 方法名分派到蓝图方法体，重载按实参个数（argc）解析。
"""

from __future__ import annotations

import inspect
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Callable, Dict, List, Tuple

if TYPE_CHECKING:
    from ..avm import AVM


# ============================================================================
# 辅助函数
# ============================================================================


def _java_name_from_descriptor(descriptor: str) -> str:
    """从 method descriptor 提取 Java 方法名（``(`` 之前的部分）。

    示例：
    - ``"hashCode()I"``      → ``"hashCode"``
    - ``"Signature([B)V"``   → ``"Signature"``
    - ``"a/b/C.foo()V"``     → ``"foo"``（带 class 前缀时取最后一个 ``.`` 之后）

    无 ``(`` 的非法 descriptor 直接返回原文，交由后续 register 阶段 fail-fast。
    """
    paren = descriptor.find("(")
    name = descriptor if paren < 0 else descriptor[:paren]
    dot = name.rfind(".")
    if dot >= 0:
        name = name[dot + 1:]
    return name


def _resolve_method_descriptor(raw: Any, fn: Callable[..., Any]) -> str | None:
    """取出函数上的 ``@java_method`` descriptor。

    ``@java_method`` 与 ``@staticmethod`` 叠加时，descriptor 落在 staticmethod
    对象上（而非底层 ``__func__``）；普通 instance method 则落在函数本身。
    故先查 ``raw``（类字典里的原始对象），再回落到底层 ``fn``。
    """
    return getattr(raw, "__java_method_descriptor__", None) \
        or getattr(fn, "__java_method_descriptor__", None)


def _method_argc(fn: Callable[..., Any]) -> int:
    """计算 instance method 的用户参数个数（剥离 ``self``）。

    分派时 ``self`` 经 ``_bind`` 闭包注入，用户调用 ``len(a)`` 即用户参数个数，
    因此存入 ``_Entry.argc`` 时减去一个 ``self``，使 ``e.argc == len(a)`` 成立。
    """
    code = getattr(fn, "__code__", None)
    if code is None:
        return 0
    # co_argcount 含 self；instance method 至少有 1 个 self 参数
    return max(code.co_argcount - 1, 0)


# ============================================================================
# _Entry —— 分派表条目
# ============================================================================


@dataclass(frozen=True)
class _Entry:
    """一个 Java 方法名对应的某个 Python 方法重载。

    重载（同名不同参数）会有多个 ``_Entry`` 共享同一 java_name。
    """
    py_name: str   # 蓝图上的 Python 方法名
    desc: str      # Java method descriptor
    argc: int      # 该重载的用户参数个数（不含 self）


def _bind(java_class: type, py_name: str, self_obj: "JavaObject") -> Callable[..., Any]:
    """把蓝图上的函数绑定到指定 ``self_obj``（JavaObject），返回可调用对象。

    分派时显式传入 ``self_obj`` 作 ``self``，其余参数透传。
    """
    fn = getattr(java_class, py_name)  # Py3 普通函数（未绑定）
    return lambda *a, **k: fn(self_obj, *a, **k)


# ============================================================================
# JavaClass —— 蓝图基类
# ============================================================================


class JavaClass:
    """蓝图基类。

    用户写 ``class Signature(JavaClass)``。类创建时 ``__init_subclass__`` 扫描
    MRO 的 ``@java_method`` 元数据，构建分派表与 Rust 注册用的方法列表。

    构造必须显式传 avm 作首参：``Signature(avm, ...)`` 等价于
    ``avm.new_object(Signature, ...)``。不传 avm → ``__new__`` 签名不匹配 →
    Python 直接 ``TypeError``（天然 fail-fast）。
    """

    # 类级结构由 __init_subclass__ 在子类创建时填充（此处仅声明类型）。
    __java_dispatch__: Dict[str, List[_Entry]]
    __java_methods__: List[Tuple[str, str, Callable[..., Any], bool]]

    def __init_subclass__(cls, **kwargs: Any) -> None:
        super().__init_subclass__(**kwargs)
        dispatch: Dict[str, List[_Entry]] = {}
        methods: List[Tuple[str, str, Callable[..., Any], bool]] = []
        seen_descs: set[str] = set()  # descriptor 去重：同一 Java 方法签名只保留最近一层

        # 扫 MRO（跳过 JavaClass / object），逐层收集 @java_method 函数。
        # 从派生类向基类遍历，配合 descriptor 去重保证最近一层定义胜出（Python MRO 语义）。
        # 注意这里不能按 Python attr_name 去重：
        # 子类完全可能用不同 Python 名去承载与父类相同的 Java descriptor，
        # 此时覆写语义应由 descriptor 决定，而不是由 Python 名决定。
        for klass in cls.__mro__:
            if klass is JavaClass or klass is object:
                continue
            for attr_name, raw in vars(klass).items():
                # 解出底层函数 + 判定是否 static（classmethod 视同 instance）
                if isinstance(raw, staticmethod):
                    fn = raw.__func__
                    is_static = True
                elif isinstance(raw, classmethod):
                    fn = raw.__func__
                    is_static = False
                elif inspect.isfunction(raw):
                    fn = raw
                    is_static = False
                else:
                    continue

                desc = _resolve_method_descriptor(raw, fn)
                if desc is None:
                    continue  # 非 @java_method 方法，跳过
                if desc in seen_descs:
                    continue  # 同一 descriptor 已被更近一层类占用，父类实现跳过

                seen_descs.add(desc)
                methods.append((attr_name, desc, fn, is_static))

                # 静态方法不进 Python 侧分派表：静态方法走 guest→Python 方向 A
                # （Non-Goal：不做静态方法的 Python 侧 __getattr__ 分派）。
                if not is_static:
                    java_name = _java_name_from_descriptor(desc)
                    dispatch.setdefault(java_name, []).append(
                        _Entry(py_name=attr_name, desc=desc, argc=_method_argc(fn))
                    )

        cls.__java_dispatch__ = dispatch
        cls.__java_methods__ = methods

    def __new__(cls, avm: "AVM", *args: Any, **kwargs: Any) -> "JavaObject":
        # avm 是必填首参；不传 → Python TypeError（签名不匹配），天然 fail-fast。
        # 委托给 avm.new_object，返回 JavaObject（非 cls 实例）→ Python 跳过 cls.__init__。
        return avm.new_object(cls, *args, **kwargs)


# ============================================================================
# JavaObject —— 实例类型
# ============================================================================


class JavaObject:
    """实例类型。

    由 ``avm.new_object`` 构造（直接 ``JavaObject.__new__(JavaObject)``，绕过
    ``JavaClass.__new__`` 避免递归）。携带：

    - ``_java_class`` —— 蓝图（JavaClass 子类），提供分派表
    - ``_avm``        —— 构造时传入的 avm，供方法体内派生相关对象
    - ``_handle``     —— Rust VM 分配的全局句柄（JNI ``jobject`` 等价物）

    ``__getattr__`` 按 Java 方法名分派：单条直返绑定方法，多条按 argc 选重载，
    未知名抛 ``AttributeError``（仅拦截分派表内的名，防 ``__getattr__`` 递归）。
    """

    _java_class: "type[JavaClass]"
    _avm: "AVM"
    _handle: int

    def __getattr__(self, name: str) -> Any:
        # 直接读实例 __dict__，避免访问未初始化属性时触发 __getattr__ 递归。
        java_class = self.__dict__.get("_java_class")
        if java_class is None:
            raise AttributeError(
                f"{type(self).__name__!r} 实例未初始化（缺少 _java_class），"
                f"无法访问 {name!r}；请经 avm.new_object 构造"
            )

        dispatch = getattr(java_class, "__java_dispatch__", {})
        entries = dispatch.get(name)
        if not entries:
            raise AttributeError(
                f"{java_class.__name__} 实例无 Java 方法 {name!r}"
            )

        if len(entries) == 1:
            return _bind(java_class, entries[0].py_name, self)

        # 重载：按实参个数选。首版限制：同 argc 不同类型会歧义，取首个匹配。
        def _dispatcher(*a: Any, **k: Any) -> Any:
            for entry in entries:
                if entry.argc == len(a):
                    return _bind(java_class, entry.py_name, self)(*a, **k)
            raise TypeError(
                f"{name!r} 无匹配 argc={len(a)} 的重载"
                f"（候选 argc: {[e.argc for e in entries]}）"
            )

        return _dispatcher
