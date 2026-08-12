# -*- coding: utf-8 -*-
"""「两处声明必须一致」的看守档：对不上就红，别等运行时才发现。

- 类型存根 vs 运行时：包里声明了 `py.typed`，IDE 与类型检查器只信 `.pyi`。绑定
  新增一个 pyfunction 而忘了补存根时，运行时能用、静态面却报「没有这个属性」
  ——2026-08-12 的 `aios_db.fixture` 就是这么漏的。
- conftest 的树产物清单 vs Rust 侧的文件名：漏一个就会拿测试产物顶掉真项目的
  空间树快照（V2 迁移时差点发生）。
"""

from __future__ import annotations

import ast
import re
from pathlib import Path

import pytest

pytestmark = pytest.mark.offline

REPO_ROOT = Path(__file__).resolve().parents[2]
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


def test_conftest_shelves_every_tree_artifact_rust_can_write():
    """conftest 的搬挪清单必须盖住 Rust 侧会写出的每一种空间树产物。

    房间档跑在一次性内存库上，但空间树落盘写的是**仓库根**、文件名与真项目
    同款。conftest 会在 session 前后把它们挪开再还原——清单漏一项，那一项就会
    被测试产物顶掉。V2 迁移（`.bin` + `.meta.json` → `.snapshot`）时这张表差点
    没跟上，所以改成从源码反查而不是靠人记。
    """
    source = (REPO_ROOT / "src" / "fast_model" / "aabb_tree.rs").read_text(
        encoding="utf-8"
    )
    written = set(re.findall(r'accel_tree_\{\}(\.[A-Za-z0-9.]+)"', source))
    assert written, "源码里找不到 accel_tree_{} 文件名——正则或文件名约定变了"

    from conftest import TREE_ARTIFACTS

    shelved = {
        # `.meta.json` 是双段后缀，`Path.suffix` 只给 `.json`。
        name[name.index(".") :]
        for name in (path.name for path in TREE_ARTIFACTS)
    }
    assert written <= shelved, (
        f"conftest 的 TREE_ARTIFACTS 漏了 {sorted(written - shelved)}"
        "——不补的话测试会把它写在仓库根、顶掉真项目的同名产物"
    )
