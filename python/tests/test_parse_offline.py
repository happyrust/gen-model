# -*- coding: utf-8 -*-
"""解析层离线用例：只吃仓内 issue-019 会话快照夹具。

这是 CI 唯一跑得动的一档——解析层不连 SurrealDB、不碰 E3D 装机、不扫项目目录，
夹具（db8000 的 sesno 24 / 25 / 26 三份快照）就躺在 `tests/fixtures/issues` 里。
断言与 Rust 侧 `tests/db8000_two_delete_fixture.rs` 同源：同一份数据，两条链路
必须读出同一串删除序列，谁漂移了都会在这里红。
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


def test_collect_changes_reports_both_deletes(configured, snapshots, manifest):
    """窗口 25..26：子件在 25 删、父件在 26 删——issue-019 的病灶序列本体。"""
    window = manifest["window"]
    start, end = window["start_sesno"], window["end_sesno"]
    changes = configured.parse.collect_changes(
        str(snapshots["parent_deleted"]), start, end
    )
    assert set(changes) == {str(start), str(end)}, changes.keys()

    deleted_at = {
        sesno: {op["refno"] for op in ops if op["op"] == "deleted"}
        for sesno, ops in changes.items()
    }
    assert manifest["refs"]["child"] in deleted_at[str(start)]
    assert manifest["refs"]["parent_equi"] in deleted_at[str(end)]

    for sesno, ops in changes.items():
        for op in ops:
            assert op["sesno"] == int(sesno), f"操作要落在自己的会话分区: {op}"


def test_collect_changes_detail_expands_attributes(configured, snapshots, manifest):
    """detail=False 只给属性名列表，detail=True 给具体值——两种形态都得成立。"""
    window = manifest["window"]
    path = str(snapshots["parent_deleted"])
    brief = configured.parse.collect_changes(path, window["start_sesno"], window["end_sesno"])
    full = configured.parse.collect_changes(
        path, window["start_sesno"], window["end_sesno"], detail=True
    )
    assert set(brief) == set(full)
    for sesno in brief:
        assert len(brief[sesno]) == len(full[sesno])
    modified = [
        op for ops in full.values() for op in ops if op["op"] == "modified"
    ]
    for op in modified:
        assert isinstance(op["added"], dict), "detail=True 的 added 应是 {名: 值}"


def test_element_dump_and_history_replay(configured, snapshots, manifest):
    """被删元素在最新索引里读不到，但给 sesno 能读回历史版本（M4 记过的坑：
    「最新索引里没有」不等于「从未存在」）。"""
    child = manifest["refs"]["child"]
    baseline_sesno = manifest["window"]["baseline_sesno"]

    dump = configured.parse.element(str(snapshots["baseline"]), child)
    assert dump["refno"] == child
    assert dump["noun"].strip().upper() == "BOX"
    assert dump["found_sesno"] <= baseline_sesno
    assert dump["owner"] == manifest["refs"]["parent_equi"]

    final = str(snapshots["parent_deleted"])
    with pytest.raises(RuntimeError):
        configured.parse.element(final, child)
    revived = configured.parse.element(final, child, sesno=baseline_sesno)
    assert revived["refno"] == child
    assert revived["noun"].strip().upper() == "BOX"


def test_element_rejects_unparsable_refno(configured, snapshots):
    with pytest.raises(RuntimeError):
        configured.parse.element(str(snapshots["baseline"]), "not-a-refno")


def test_attmap_and_subtree_read_zone_without_database(configured, snapshots, manifest):
    """生成期属性视图及父子闭包均由 PdmsIO 直读，且不依赖数据库连接。"""
    zone = manifest["refs"]["zone"]
    one = configured.parse.attmap(str(snapshots["baseline"]), zone)
    assert one["refno"] == zone
    assert isinstance(one["attrs"], dict)
    tree = configured.parse.subtree(str(snapshots["baseline"]), zone)
    assert tree["root"] == zone
    assert tree["count"] == len(tree["elements"])
    assert tree["count"] >= 1
    assert tree["elements"][0]["refno"] == zone
    # direct 解析同时产出生成器可消费的 PdmsGeoParam；ZONE 自身通常无实体，
    # 其后代至少应有一个带几何参数的基本体。
    primitive_models = [item for item in tree["elements"] if item["geo_valid"]]
    assert primitive_models
    assert all(item["geo_param"] is not None for item in primitive_models)
    assert any(item["mesh"] is not None for item in primitive_models)


def test_generate_model_writes_direct_zone_artifact(configured, snapshots, manifest, tmp_path):
    zone = manifest["refs"]["zone"]
    output = tmp_path / "zone-model.json"
    result = configured.parse.generate_model(str(snapshots["baseline"]), zone, str(output))
    assert result["format"] == "direct-model-v1"
    assert result["root"] == zone
    assert result["count"] > 0
    assert result["mesh_count"] > 0
    assert output.exists()
    payload = json.loads(output.read_text(encoding="utf-8"))
    assert payload["root"] == zone
    assert payload["count"] == len(payload["elements"])
    assert any(item["geo_valid"] for item in payload["elements"])
    assert sum(1 for item in payload["elements"] if item["rvm_primitive"]) > 0


def test_generate_obj_writes_direct_zone_mesh(configured, snapshots, manifest, tmp_path):
    zone = manifest["refs"]["zone"]
    output = tmp_path / "zone-model.obj"
    result = configured.parse.generate_obj(str(snapshots["baseline"]), zone, str(output))
    assert result["format"] == "obj"
    assert result["mesh_count"] > 0
    text = output.read_text(encoding="utf-8")
    assert text.startswith("# aios direct-model-v1 OBJ")
    assert "\nv " in text and "\nf " in text


def test_generate_rvm_writes_direct_zone_smoke(configured, snapshots, manifest, tmp_path):
    zone = manifest["refs"]["zone"]
    output = tmp_path / "zone-model.rvm"
    result = configured.parse.generate_rvm(str(snapshots["baseline"]), zone, str(output))
    assert result["format"] == "rvm-direct-v1"
    assert result["geometry_count"] > 0
    assert output.stat().st_size == result["bytes"]


def _net_classes(result: dict) -> dict[str, str]:
    return {
        entry["refno"]: kind
        for kind in ("added", "deleted", "modified")
        for entry in result[kind]
    }


def test_net_changes_reports_the_recorded_net_tristate(configured, snapshots, manifest):
    """窗口 25..26 的净三态（会话索引差分，不逐会话解析）：ZONE=modified、
    EQUI 与子件=deleted——与 Rust 侧 db8000_session_pairs 性质 h 同一份 ground
    truth。with_noun 时删除条目从旧记录（不可变页）解出类型名。"""
    window = manifest["window"]
    refs = manifest["refs"]
    result = configured.parse.net_changes(
        str(snapshots["parent_deleted"]),
        window["start_sesno"],
        window["end_sesno"],
        with_noun=True,
    )

    assert result["base_sesno"] == window["baseline_sesno"]
    assert result["target_sesno"] == window["end_sesno"]
    assert _net_classes(result) == {
        refs["zone"]: "modified",
        refs["parent_equi"]: "deleted",
        refs["child"]: "deleted",
    }

    nouns = {
        entry["refno"]: (entry["noun"] or "").strip().upper()
        for kind in ("added", "deleted", "modified")
        for entry in result[kind]
    }
    assert nouns[refs["zone"]] == "ZONE"
    assert nouns[refs["parent_equi"]] == "EQUI"
    assert nouns[refs["child"]] == "BOX"


def test_net_changes_bases_each_window_on_the_previous_session(
    configured, snapshots, manifest
):
    """单会话窗口 26..26：base 落在 25，子件（25 删、两端都不在场）不出现——
    净口径「窗口内自我抵消不出现」的直接检验。"""
    window = manifest["window"]
    refs = manifest["refs"]
    result = configured.parse.net_changes(
        str(snapshots["parent_deleted"]), window["end_sesno"], window["end_sesno"]
    )

    assert result["base_sesno"] == window["start_sesno"]
    assert _net_classes(result) == {
        refs["zone"]: "modified",
        refs["parent_equi"]: "deleted",
    }


def test_net_changes_refuses_windows_beyond_the_latest_session(
    configured, snapshots, manifest
):
    """窗口终点超出文件最新会话必须响亮报错（窗口与文件对不上，不猜）。"""
    with pytest.raises(RuntimeError):
        configured.parse.net_changes(
            str(snapshots["parent_deleted"]), 1, manifest["window"]["end_sesno"] + 10
        )


def test_net_window_returns_semantic_operations_without_a_database(
    configured, snapshots, manifest
):
    """公开语义入口必须接到生产净窗口合成器，而不是只回传记录位置触达集。"""
    window = manifest["window"]
    refs = manifest["refs"]
    result = configured.parse.net_window(
        str(snapshots["parent_deleted"]),
        window["start_sesno"],
        window["end_sesno"],
        detail=True,
    )

    operations = [
        operation
        for session in result["window"].values()
        for operation in session
    ]
    by_refno = {operation["refno"]: operation for operation in operations}
    assert result["counts"] == {"added": 0, "deleted": 2, "modified": 1}
    assert by_refno[refs["zone"]]["op"] == "modified"
    assert by_refno[refs["parent_equi"]]["op"] == "deleted"
    assert by_refno[refs["child"]]["op"] == "deleted"
    assert result["unparseable_finals"] == 0
