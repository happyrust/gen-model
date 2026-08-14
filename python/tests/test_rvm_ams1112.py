# -*- coding: utf-8 -*-
"""AMS dbnum 1112 SweepSolid（WALL/STWALL）对拍 E3D RVM。

默认 skip：会连 8009 生产验证库，且会和房间档 conftest 抢 `set_config`。
单独跑：

    $env:AIOS_RVM_LIVE = '1'
    python\\.venv\\Scripts\\python.exe -m pytest tests\\test_rvm_ams1112.py -q --noconftest
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = REPO_ROOT / "python" / "scripts"
sys.path.insert(0, str(SCRIPTS))

import rvm_aabb_compare as rvm  # noqa: E402

LIVE = os.environ.get("AIOS_RVM_LIVE") == "1"

pytestmark = [
    pytest.mark.live_8009,
    pytest.mark.skipif(not LIVE, reason="live 8009 RVM 对拍；设 AIOS_RVM_LIVE=1 且单独跑本文件"),
]


def test_1112_cwall_rr001_rvm_aabb():
    fixture = rvm.FIXTURES["1rs-wf03-w-c-rr001"]
    code = rvm.run_fixture(fixture, require_snapshot=True)
    assert code == 0, "WALL/STWALL 相对 E3D RVM 的 AABB 跨度超 max(3mm, 3%)"
