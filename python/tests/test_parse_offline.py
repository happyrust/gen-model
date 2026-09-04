# -*- coding: utf-8 -*-
"""解析层离线用例：只吃仓内 issue-019 会话快照夹具。

这是 CI 唯一跑得动的一档——解析层不连 SurrealDB、不碰 E3D 装机、不扫项目目录，
夹具（db8000 的 sesno 24 / 25 / 26 三份快照）就躺在 `tests/fixtures/issues` 里。
断言与 Rust 侧 `tests/db8000_two_delete_fixture.rs` 同源：同一份数据，两条链路
必须读出同一串删除序列，谁漂移了都会在这里红。

判据只有一个：这三份快照是 E3D TTY 真做过的两次删除（子件在 25 删、父件在 26 删，
manifest 里记着），不是另一套解析器的读数。old-pdms-io 底座的 `collect_changes` /
`net_changes` / `element` / `attmap` / `subtree` / `generate_*` 及其用例 2026-09-04 已随
old-pdms-io 一起删除；窗口口径只剩生产收集器的 `net_window`。
"""

from __future__ import annotations

import hashlib
import json
import zipfile
from pathlib import Path

import pytest

pytestmark = pytest.mark.offline

REPO_ROOT = Path(__file__).resolve().parents[2]
FIXTURE_DIR = (
    REPO_ROOT
    / "tests"
    / "fixtures"
    / "issues"
    / "issue-019-cross-session-parent-child-delete"
)


@pytest.fixture(scope="session")
def manifest() -> dict:
    if not FIXTURE_DIR.exists():
        pytest.skip(f"缺 issue-019 夹具目录: {FIXTURE_DIR}")
    return json.loads((FIXTURE_DIR / "manifest.json").read_text(encoding="utf-8"))


@pytest.fixture(scope="session")
def snapshots(manifest, tmp_path_factory) -> dict[str, Path]:
    """解压三份快照，返回 role -> 文件路径（session 内解一次）。"""
    archive = FIXTURE_DIR / manifest["archive"]["path"]
    out = tmp_path_factory.mktemp("issue019")
    with zipfile.ZipFile(archive) as zf:
        zf.extractall(out)
    return {snap["role"]: out / snap["path"] for snap in manifest["snapshots"]}


@pytest.fixture(scope="session")
def by_role(manifest) -> dict[str, dict]:
    return {snap["role"]: snap for snap in manifest["snapshots"]}


def test_archive_integrity(manifest):
    """夹具没被换行转换 / LFS 占位符弄坏——后面全部断言以此为前提。"""
    archive = FIXTURE_DIR / manifest["archive"]["path"]
    assert archive.stat().st_size == manifest["archive"]["bytes"]
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    assert digest == manifest["archive"]["sha256"]


def test_snapshots_recognized_as_db_files(configured, snapshots):
    for role, path in snapshots.items():
        assert configured.parse.is_db_file(str(path)), f"{role} 应被认成候选库文件"


def test_header_matches_manifest(configured, snapshots, by_role, manifest):
    for role, path in snapshots.items():
        header = configured.parse.header(str(path))
        assert header["dbnum"] == manifest["dbnum"], role
        assert header["latest_sesno"] == by_role[role]["sesno"], role
        assert header["file_size"] > 0, role
        assert path.stat().st_size == by_role[role]["bytes"], role


def test_sessions_ascend_to_snapshot_sesno(configured, snapshots, by_role):
    """会话页升序且末位 == 该快照被切到的会话（切割正确性的最小证据）。"""
    for role, path in snapshots.items():
        sesnos = [row["sesno"] for row in configured.parse.sessions(str(path))]
        assert sesnos, role
        assert sesnos == sorted(sesnos), f"{role} 会话页应升序: {sesnos}"
        assert sesnos[-1] == by_role[role]["sesno"], role


def test_parse_exposes_exactly_one_window_entry(configured):
    """old-pdms-io 底座的绑定不许再回来：窗口只剩 `net_window` 一个口径，单元素直读只剩
    e3d-io 底座的 `element`。"""
    for retired in (
        "collect_changes",
        "net_changes",
        "attmap",
        "subtree",
        "generate_all_model",
        "generate_model",
        "generate_obj",
        "generate_rvm",
        "noun_dict",
    ):
        assert not hasattr(configured.parse, retired), f"parse.{retired} 是 old-pdms-io 的绑定，2026-09-04 已删"
    assert callable(configured.parse.net_window)
    assert callable(configured.parse.element)


def test_element_reads_the_snapshot_tty_left_behind(configured, snapshots, manifest):
    """被删元素在最新索引里读不到，但把文件钉回 baseline 会话就能读回历史版本
    （M4 记过的坑：「最新索引里没有」不等于「从未存在」）。三份快照都是 TTY 真做过的删除留下的。"""
    child = manifest["refs"]["child"]
    baseline_sesno = manifest["window"]["baseline_sesno"]

    dump = configured.parse.element(str(snapshots["baseline"]), child)
    assert dump["refno"] == child
    assert dump["sesno"] == baseline_sesno
    assert dump["noun"].strip().upper() == "BOX"
    assert dump["owner"] == manifest["refs"]["parent_equi"]
    assert isinstance(dump["attrs"], dict) and dump["attrs"]

    final = str(snapshots["parent_deleted"])
    with pytest.raises(RuntimeError, match="找不到元素"):
        configured.parse.element(final, child)
    revived = configured.parse.element(final, child, sesno=baseline_sesno)
    assert revived["refno"] == child
    assert revived["sesno"] == baseline_sesno
    assert revived["noun"].strip().upper() == "BOX"
    assert revived["attrs"] == dump["attrs"], "同一会话时点从两份快照读出的持久属性必须逐字相同"

    zone = manifest["refs"]["zone"]
    parent = configured.parse.element(str(snapshots["baseline"]), manifest["refs"]["parent_equi"])
    assert parent["owner"] == zone
    assert child in parent["children"]


def test_element_rejects_unparsable_refno(configured, snapshots):
    with pytest.raises(RuntimeError):
        configured.parse.element(str(snapshots["baseline"]), "not-a-refno")


def _operations(result: dict) -> list[dict]:
    return [operation for session in result["window"].values() for operation in session]


def _net_classes(result: dict) -> dict[str, str]:
    return {operation["refno"]: operation["op"] for operation in _operations(result)}


def test_net_window_returns_semantic_operations_without_a_database(
    configured, snapshots, manifest
):
    """窗口 25..26 的净三态：ZONE=modified、EQUI 与子件=deleted——TTY 在这两个会话里
    真做过的两次删除，与 Rust 侧 db8000_session_pairs 夹具是同一份 ground truth。"""
    window = manifest["window"]
    refs = manifest["refs"]
    result = configured.parse.net_window(
        str(snapshots["parent_deleted"]),
        window["start_sesno"],
        window["end_sesno"],
        detail=True,
    )

    assert result["requested_start"] == window["start_sesno"]
    assert result["requested_end"] == window["end_sesno"]
    assert result["counts"] == {"added": 0, "deleted": 2, "modified": 1}
    assert _net_classes(result) == {
        refs["zone"]: "modified",
        refs["parent_equi"]: "deleted",
        refs["child"]: "deleted",
    }
    by_refno = {operation["refno"]: operation for operation in _operations(result)}
    assert by_refno[refs["zone"]]["noun"].strip().upper() == "ZONE"
    assert result["unparseable_finals"] == 0


def test_net_window_keys_operations_inside_the_window_and_tombstones_at_its_end(
    configured, snapshots, manifest
):
    """子件在 25 删、父件在 26 删（issue-019 的病灶序列本体）。净口径**不**复刻逐会话回放的
    「每会话一条」分区：每 refno 恰一条，分区键落在窗口内且等于操作自己的 `sesno`；
    墓碑在终点索引里没有记录，一律挂在窗口终点——两次删除都在 26 这一格里出现。"""
    window = manifest["window"]
    refs = manifest["refs"]
    start, end = window["start_sesno"], window["end_sesno"]
    result = configured.parse.net_window(str(snapshots["parent_deleted"]), start, end)

    assert result["window"], "窗口不该为空"
    for sesno, ops in result["window"].items():
        assert start <= int(sesno) <= end, f"分区键跑出窗口: {sesno}"
        for op in ops:
            assert op["sesno"] == int(sesno), f"操作要落在自己的会话分区: {op}"
    deleted = {op["refno"]: op["sesno"] for op in _operations(result) if op["op"] == "deleted"}
    assert deleted == {refs["child"]: end, refs["parent_equi"]: end}
    refnos = [op["refno"] for op in _operations(result)]
    assert len(refnos) == len(set(refnos)), "净窗口每 refno 恰一条"


def test_net_window_detail_expands_attributes(configured, snapshots, manifest):
    """detail=False 只给属性名列表，detail=True 给具体值——两种形态都得成立。"""
    window = manifest["window"]
    path = str(snapshots["parent_deleted"])
    brief = configured.parse.net_window(path, window["start_sesno"], window["end_sesno"])
    full = configured.parse.net_window(
        path, window["start_sesno"], window["end_sesno"], detail=True
    )
    assert set(brief["window"]) == set(full["window"])
    for sesno in brief["window"]:
        assert len(brief["window"][sesno]) == len(full["window"][sesno])
    modified_brief = [op for op in _operations(brief) if op["op"] == "modified"]
    modified_full = [op for op in _operations(full) if op["op"] == "modified"]
    assert modified_brief and len(modified_brief) == len(modified_full)
    for op in modified_brief:
        assert isinstance(op["added"], list), "detail=False 的 added 应是属性名列表"
    for op in modified_full:
        assert isinstance(op["added"], dict), "detail=True 的 added 应是 {名: 值}"


def test_net_window_bases_a_single_session_window_on_the_previous_session(
    configured, snapshots, manifest
):
    """单会话窗口 26..26：base 落在 25，子件（25 删、两端都不在场）不出现——
    净口径「窗口内自我抵消不出现」的直接检验。"""
    window = manifest["window"]
    refs = manifest["refs"]
    result = configured.parse.net_window(
        str(snapshots["parent_deleted"]), window["end_sesno"], window["end_sesno"]
    )

    assert result["counts"] == {"added": 0, "deleted": 1, "modified": 1}
    assert _net_classes(result) == {
        refs["zone"]: "modified",
        refs["parent_equi"]: "deleted",
    }


def test_net_window_refuses_windows_beyond_the_latest_session(
    configured, snapshots, manifest
):
    """窗口终点超出文件最新会话必须响亮报错（窗口与文件对不上，不猜）。"""
    with pytest.raises(RuntimeError):
        configured.parse.net_window(
            str(snapshots["parent_deleted"]), 1, manifest["window"]["end_sesno"] + 10
        )
