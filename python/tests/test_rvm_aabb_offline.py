# -*- coding: utf-8 -*-
"""RVM AABB 对拍纯函数：不连库。钉住与 rvm_gate_c_or_1r345_c.ps1 相同的容差口径。"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

pytestmark = pytest.mark.offline

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = REPO_ROOT / "python" / "scripts"
sys.path.insert(0, str(SCRIPTS))

import rvm_aabb_compare as rvm  # noqa: E402


def test_span_from_world_box():
    assert rvm.span([0.0, 1.0, 2.0, 10.0, 4.0, 8.0]) == (10.0, 3.0, 6.0)
    assert rvm.span(None) is None
    assert rvm.span([1, 2, 3]) is None


def test_equal_spans_ok():
    verdict, excess = rvm.compare_spans((1000.0, 50.0, 30.0), (1000.0, 50.0, 30.0))
    assert verdict == "OK"
    assert excess <= 0


def test_faceting_within_abs_tol_ok():
    """圆柱/FTUB 面片化 ~2–3mm，绝对值门限 3mm 应放过。"""
    verdict, _ = rvm.compare_spans((857.0, 50.0, 30.0), (854.8, 50.0, 30.0))
    assert verdict == "OK"


def test_coverage_defect_fails():
    """修复前薄饼覆盖：gen 一维只有 20mm、E3D 857mm。"""
    verdict, excess = rvm.compare_spans((857.0, 50.0, 30.0), (20.0, 50.0, 30.0))
    assert verdict == "FAIL"
    assert excess > 800


def test_rel_tol_absorbs_percent_error():
    rvm_span = (1000.0, 1000.0, 1000.0)
    gen_span = (1020.0, 1000.0, 1000.0)  # 2% < 3%
    verdict, _ = rvm.compare_spans(rvm_span, gen_span)
    assert verdict == "OK"
    verdict, _ = rvm.compare_spans(rvm_span, (1040.0, 1000.0, 1000.0))
    assert verdict == "FAIL"


def test_find_rvm_member_does_not_match_wall_10():
    members = [
        {"name": "WALL 10 of CWALL /X", "aabb_world_mm": [0, 0, 0, 1, 1, 1]},
        {"name": "WALL 1 of CWALL /X", "aabb_world_mm": [0, 0, 0, 2, 2, 2]},
    ]
    hit = rvm.find_rvm_member(members, "WALL 1")
    assert hit is not None
    assert hit["name"].startswith("WALL 1 ")


def test_pairs_from_children_sorts_by_refno():
    children = [
        {"refno": "17496_105930", "noun": "WALL"},
        {"refno": "17496_105812", "noun": "STWALL"},
        {"refno": "17496_105912", "noun": "WALL"},
        {"refno": "17496_105817", "noun": "GWALL"},
        {"refno": "17496_105813", "noun": "STWALL"},
    ]
    pairs = rvm.pairs_from_children(children, ("WALL", "STWALL"))
    assert [p.rvm for p in pairs] == ["WALL 1", "WALL 2", "STWALL 1", "STWALL 2"]
    assert [p.gen for p in pairs] == [
        "17496_105912",
        "17496_105930",
        "17496_105812",
        "17496_105813",
    ]


def test_c_or_snapshot_has_ftube1_aabb():
    snapshot = REPO_ROOT / "test_data" / "rvm" / "C-OR-1R345-C.rvm.json"
    if not snapshot.is_file():
        pytest.skip(f"缺 {snapshot}")
    data = json.loads(snapshot.read_text(encoding="utf-8"))
    member = rvm.find_rvm_member(data["members"], "FTUBE 1")
    assert member is not None
    box = member.get("aabb_world_mm")
    assert box and len(box) == 6
    dx, dy, dz = rvm.span(box)
    assert dx > 100
    assert dy > 100
    assert dz > 1
