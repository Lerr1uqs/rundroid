#!/usr/bin/env python3
"""rundroid 资源 URI 解析与最小管理工具。

资源 URI 形如 ``resource:<pack>/<path>``，例如 ``resource:smoke/build/libsmoke.so``。

为什么强制使用 resource URI：
- 让 case manifest 不依赖绝对路径，保证跨主机可移植
- 让 fixture 与 manifest 解耦：fixture 可以从仓库内 / 缓存 / 远端 pack 任意来源解析

bootstrap 阶段实现：
- 解析：``resource:smoke/build/libsmoke.so`` → ``<repo>/resources/smoke/build/libsmoke.so``
- 列举：列出 ``resources/`` 下所有 pack 与 pack 内文件
- 校验：检查 case.toml 引用的 URI 是否存在

使用示例::

    python tools/resources.py resolve resource:smoke/build/libsmoke.so
    python tools/resources.py list smoke
    python tools/resources.py check tests/cases/01-pure-export-call/case.toml
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import Iterable

try:
    import tomllib  # Python 3.11+
except ModuleNotFoundError:  # pragma: no cover - 兼容老版本
    import tomli as tomllib  # type: ignore[no-redef]

RESOURCE_URI_RE = re.compile(r"^resource:(?P<pack>[^/]+)/(?P<path>.+)$")


def find_repo_root(start: Path | None = None) -> Path:
    """从 start 起向上找包含 resources/ 目录的祖先。

    bootstrap 阶段不强求 git 仓库标记，只要找到 resources/ 即视为仓库根。
    """
    cur = (start or Path(__file__)).resolve()
    if cur.is_file():
        cur = cur.parent
    while True:
        if (cur / "resources").is_dir():
            return cur
        if cur.parent == cur:
            raise RuntimeError("could not locate repo root (no resources/ found)")
        cur = cur.parent


def resources_root() -> Path:
    return find_repo_root() / "resources"


def parse_uri(uri: str) -> tuple[str, str]:
    """把 ``resource:<pack>/<path>`` 拆成 (pack, path)。非法时抛 ValueError。"""
    m = RESOURCE_URI_RE.match(uri)
    if not m:
        raise ValueError(f"not a resource URI: {uri}")
    return m.group("pack"), m.group("path")


def resolve(uri: str) -> Path:
    """把 resource URI 解析成本地文件路径。文件不存在时抛 FileNotFoundError。"""
    pack, path = parse_uri(uri)
    full = resources_root() / pack / path
    if not full.is_file():
        raise FileNotFoundError(f"resource not found: {uri} (looked at {full})")
    return full


def list_pack(pack: str) -> Iterable[Path]:
    """列出指定 pack 下的所有文件（递归）。pack 不存在抛 FileNotFoundError。"""
    root = resources_root() / pack
    if not root.is_dir():
        raise FileNotFoundError(f"unknown pack: {pack}")
    yield from sorted(root.rglob("*"))


def uris_in_case(case_toml: Path) -> Iterable[str]:
    """从 case.toml 中提取所有形如 ``resource:`` 的字符串值。"""
    if not case_toml.is_file():
        raise FileNotFoundError(case_toml)
    data = tomllib.loads(case_toml.read_text(encoding="utf-8"))
    # 简化：递归扫描所有字符串值。
    seen: list[str] = []
    _collect_strings(data, seen)
    return [s for s in seen if s.startswith("resource:")]


def _collect_strings(node, out: list[str]) -> None:
    if isinstance(node, str):
        out.append(node)
    elif isinstance(node, dict):
        for v in node.values():
            _collect_strings(v, out)
    elif isinstance(node, list):
        for v in node:
            _collect_strings(v, out)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="rundroid resource URI tool")
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_resolve = sub.add_parser("resolve", help="resolve a resource URI to a local path")
    p_resolve.add_argument("uri")

    p_list = sub.add_parser("list", help="list files in a resource pack")
    p_list.add_argument("pack")

    p_check = sub.add_parser("check", help="check all URIs referenced by a case.toml")
    p_check.add_argument("case_toml")

    args = parser.parse_args(argv)

    if args.cmd == "resolve":
        print(resolve(args.uri))
        return 0
    if args.cmd == "list":
        for p in list_pack(args.pack):
            if p.is_file():
                print(p.relative_to(resources_root()))
        return 0
    if args.cmd == "check":
        ok = True
        for uri in uris_in_case(Path(args.case_toml)):
            try:
                resolve(uri)
                print(f"OK   {uri}")
            except (FileNotFoundError, ValueError) as e:
                print(f"FAIL {uri}  ({e})", file=sys.stderr)
                ok = False
        return 0 if ok else 1
    return 2  # unreachable


if __name__ == "__main__":
    sys.exit(main())
