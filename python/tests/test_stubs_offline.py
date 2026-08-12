# -*- coding: utf-8 -*-
"""类型存根与运行时的一致性：`py.typed` 是对外契约，漂了就得红。

包里声明了 `py.typed`，IDE 与类型检查器只信 `.pyi`。绑定新增一个 pyfunction
而忘了补存根时，运行时能用、静态面却报「没有这个属性」——2026-08-12 的
`aios_db.fixture` 就是这么漏的。这一档把两边的名字集合逐个对齐。
"""

from __future__ import annotations

import ast
from pathlib import Path

import pytest

pytestmark = pytest.mark.offline

PKG = Path(__file__).resolve().parents[1] / "pysrc" / "aios_db"
SUBMODULES = ["db", "fixture", "incr", "model", "parse", "room", "spatial", "sync"]


def _stub_functions(pyi: Path) -> set[str]:
    tree = ast.parse(pyi.read_text(encoding="utf-8"))
    return {node.name for node in tree.body if isinstance(node, ast.FunctionDef)}


def _runtime_functions(module) -> set[str]:
    return {
        name
        for name in dir(module)
        if not name.startswith("_") and callable(getattr(module, name))
    }


def test_py_typed_marker_present():
    assert (PKG / "py.typed").exists(), "缺 py.typed，存根对外不生效"


@pytest.mark.parametrize("name", SUBMODULES)
def test_submodule_stub_matches_runtime(configured, name):
    pyi = PKG / f"{name}.pyi"
    assert pyi.exists(), f"子模块 {name} 缺存根 {pyi.name}"
    runtime = _runtime_functions(getattr(configured, name))
    stub = _stub_functions(pyi)
    assert stub == runtime, (
        f"{name}.pyi 与运行时不一致——"
        f"存根缺 {sorted(runtime - stub)}，存根多 {sorted(stub - runtime)}"
    )


def test_toplevel_stub_matches_runtime(configured):
    stub = _stub_functions(PKG / "__init__.pyi")
    runtime = {
        name for name in configured.__all__ if callable(getattr(configured, name))
    }
    assert stub == runtime, f"缺 {sorted(runtime - stub)}，多 {sorted(stub - runtime)}"


def test_stub_reexports_every_submodule(configured):
    """`__init__.pyi` 的 `from . import X as X` 必须盖住 `__all__` 里全部子模块。"""
    tree = ast.parse((PKG / "__init__.pyi").read_text(encoding="utf-8"))
    reexported = {
        alias.asname or alias.name
        for node in tree.body
        if isinstance(node, ast.ImportFrom)
        for alias in node.names
    }
    exported = {
        name for name in configured.__all__ if not callable(getattr(configured, name))
    }
    assert exported == set(SUBMODULES), (
        f"运行时导出的子模块与本文件的清单不符: {sorted(exported)}"
    )
    assert exported <= reexported, f"存根漏了子模块 {sorted(exported - reexported)}"
