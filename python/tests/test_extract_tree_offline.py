# -*- coding: utf-8 -*-
"""抽取树叠加（ADR-028）的 Python 解析层用例。

离线档只吃临时文件名 + 纯函数归并，不连库。本机若有 AMS `ams7355` /
`ams7355_0001`，额外用 `parse.header` / `parent_gap_refno_count` 钉住实文件
探针（缺文件则 skip，CI `-m offline` 不红）。
"""

from __future__ import annotations

from pathlib import Path

import pytest

pytestmark = pytest.mark.offline

AMS000 = Path(r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000")
AMS_PARENT = AMS000 / "ams7355"
AMS_LEAF = AMS000 / "ams7355_0001"

needs_ams7355 = pytest.mark.skipif(
    not (AMS_PARENT.is_file() and AMS_LEAF.is_file()),
    reason="本机没有 AMS ams7355 / ams7355_0001",
)


def _touch(dir_path: Path, name: str) -> Path:
    path = dir_path / name
    path.write_bytes(b"stub")
    return path


def test_master_and_unique_extract_are_candidate_db_files(configured, tmp_path):
    master = _touch(tmp_path, "ams7355")
    leaf = _touch(tmp_path, "ams7355_0001")
    copy = _touch(tmp_path, "ams7355_0001 copy")
    bak = _touch(tmp_path, "ams7355.bak")
    assert configured.parse.is_db_file(str(master))
    assert configured.parse.is_db_file(str(leaf))
    assert not configured.parse.is_db_file(str(copy))
    assert not configured.parse.is_db_file(str(bak))


def test_collapse_master_and_leaf_selects_the_leaf(configured, tmp_path):
    parent = _touch(tmp_path, "ams7355")
    leaf = _touch(tmp_path, "ams7355_0001")
    result = configured.parse.collapse_extract_files(
        [
            ("AMS", 7355, str(parent)),
            ("AMS", 7355, str(leaf)),
        ]
    )
    assert result["duplicate_keys"] == []
    assert result["mismatches"] == []
    assert len(result["selected"]) == 1
    selected = result["selected"][0]
    assert selected["project"] == "AMS"
    assert selected["dbnum"] == 7355
    assert Path(selected["leaf_path"]) == leaf
    assert Path(selected["parent_path"]) == parent
    assert [Path(path) for path in result["shadowed_parents"]] == [parent]


def test_collapse_leaf_only_stays_the_candidate(configured, tmp_path):
    leaf = _touch(tmp_path, "ams7322_0001")
    result = configured.parse.collapse_extract_files([("AMS", 7322, str(leaf))])
    assert result["duplicate_keys"] == []
    assert result["shadowed_parents"] == []
    assert Path(result["selected"][0]["leaf_path"]) == leaf
    assert result["selected"][0]["parent_path"] is None


def test_collapse_sibling_extracts_are_duplicate(configured, tmp_path):
    first = _touch(tmp_path, "ams9990_0001")
    second = _touch(tmp_path, "ams9990_0002")
    result = configured.parse.collapse_extract_files(
        [
            ("AMS", 9990, str(first)),
            ("AMS", 9990, str(second)),
        ]
    )
    assert result["duplicate_keys"] == [["AMS", 9990]]
    assert result["selected"] == []


def test_collapse_copy_next_to_extract_is_duplicate(configured, tmp_path):
    leaf = _touch(tmp_path, "ams1112_0001")
    copy = _touch(tmp_path, "ams1112_0001 copy")
    result = configured.parse.collapse_extract_files(
        [
            ("AMS", 1112, str(leaf)),
            ("AMS", 1112, str(copy)),
        ]
    )
    assert result["duplicate_keys"] == [["AMS", 1112]]


def test_collapse_filename_header_mismatch_blocks(configured, tmp_path):
    leaf = _touch(tmp_path, "ams7355_0001")
    result = configured.parse.collapse_extract_files([("AMS", 8000, str(leaf))])
    assert result["duplicate_keys"] == [["AMS", 8000]]
    assert result["selected"] == []
    assert result["mismatches"] == [
        {
            "path": str(leaf),
            "filename_dbnum": 7355,
            "header_dbnum": 8000,
        }
    ]


@needs_ams7355
def test_ams7355_headers_same_dbnum_leaf_is_later(configured):
    parent = configured.parse.header(str(AMS_PARENT))
    leaf = configured.parse.header(str(AMS_LEAF))
    assert parent["dbnum"] == 7355
    assert leaf["dbnum"] == 7355
    assert parent["latest_sesno"] == 13
    assert leaf["latest_sesno"] == 15
    assert leaf["file_size"] > parent["file_size"]

    parent_sesnos = [row["sesno"] for row in configured.parse.sessions(str(AMS_PARENT))]
    leaf_sesnos = [row["sesno"] for row in configured.parse.sessions(str(AMS_LEAF))]
    assert parent_sesnos[-1] == 13
    assert leaf_sesnos[-1] == 15


@needs_ams7355
def test_ams7355_collapse_selects_leaf_and_parent_gap_is_zero(configured):
    parent_h = configured.parse.header(str(AMS_PARENT))
    leaf_h = configured.parse.header(str(AMS_LEAF))
    result = configured.parse.collapse_extract_files(
        [
            ("AMS", parent_h["dbnum"], str(AMS_PARENT)),
            ("AMS", leaf_h["dbnum"], str(AMS_LEAF)),
        ]
    )
    assert result["duplicate_keys"] == []
    assert Path(result["selected"][0]["leaf_path"]) == AMS_LEAF
    assert Path(result["selected"][0]["parent_path"]) == AMS_PARENT
    assert configured.parse.parent_gap_refno_count(str(AMS_LEAF), str(AMS_PARENT)) == 0
