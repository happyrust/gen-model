# -*- coding: utf-8 -*-
"""连接层只读入口的行为用例（对着合成夹具，不需要真实项目库）。

重点盯 `db.inst` 的三段式取边：`anc CONTAINS` 索引查询 → 元素自己那一跳的图跳
回落 → 两条都空时按 `anc = NONE` 探针响亮报错。夹具是直接 `RELATE` 出来的、
**不写 `anc`**（生产的 anc 由解析/回填链路填），所以它恰好是回落路径与未回填
告警的现成靶子——真实库上走的是第一段。
"""

from __future__ import annotations

import pytest

ROOM_FRMW_NAME = "/ZZ-R-K100"


@pytest.fixture()
def frmw(binding, room_fixture) -> str:
    """夹具房间节点（FRMW，自己 own 自己）的 refno。"""
    hits = binding.db.by_name(ROOM_FRMW_NAME)
    assert len(hits) == 1, f"夹具房间应唯一: {hits}"
    return hits[0]


@pytest.fixture()
def cwall(binding, frmw) -> str:
    """房间下的 CWALL——几何体真正挂靠的 owner（夹具是 FRMW → CWALL → 几何体）。"""
    members = [row for row in binding.db.members(frmw) if row["refno"] != frmw]
    assert [row["noun"] for row in members] == ["CWALL"], members
    return members[0]["refno"]


def _bodies(fx) -> set[str]:
    return set(fx["in_a"]) | set(fx["in_b"]) | {
        fx["straddler"],
        fx["pane_a"],
        fx["pane_b"],
    }


def test_by_name_and_pe_row(binding, room_fixture, frmw):
    row = binding.db.pe(frmw)
    assert row is not None
    assert row["noun"] == "FRMW"
    assert row["name"] == ROOM_FRMW_NAME
    assert binding.db.pe("4000000001_999999") is None, "不存在的元素应给 None"


def test_members_and_owner_chain(binding, room_fixture, frmw, cwall):
    members = {row["refno"] for row in binding.db.members(cwall)}
    assert _bodies(room_fixture) <= members, f"7 个几何体都挂在 CWALL 下: {sorted(members)}"

    chain = [row["refno"] for row in binding.db.owner_chain(room_fixture["pane_a"])]
    # 自己 → CWALL → FRMW；FRMW 自己 own 自己，链到它就该停（64 跳防环不该被跑满）。
    assert chain == [room_fixture["pane_a"], cwall, frmw], chain


def test_inst_falls_back_to_graph_hop_when_anc_unfilled(binding, room_fixture):
    """夹具行没有 anc：第一段查不到，必须靠图跳把元素自己的实例边取回来。"""
    for refno in (room_fixture["pane_a"], room_fixture["straddler"]):
        edges = binding.db.inst(refno)
        assert len(edges) == 1, f"{refno} 应有且只有一条实例边: {edges}"
        edge = edges[0]
        assert edge["aabb"], "FETCH 应把 aabb 展开成对象而不是留 record 链接"
        assert edge["world_trans"], "world_trans 同样要展开"


def test_inst_reports_unfilled_anc_instead_of_empty(binding, room_fixture, frmw):
    """房间节点自己没有实例边，而库里 anc 全没回填——此时「查不全」不能被读成
    「没有」，必须响亮报错并给自愈指引。"""
    with pytest.raises(RuntimeError) as caught:
        binding.db.inst(frmw)
    message = str(caught.value)
    assert "anc" in message
    assert "gen-model" in message, f"报错要给修法: {message}"


def test_inst_rejects_unparsable_refno(binding, room_fixture):
    """解析不出来的 refno 曾静默变成 0，谓词永不命中、结果是个空集。"""
    with pytest.raises(RuntimeError) as caught:
        binding.db.inst("not-a-refno")
    assert "解析失败" in str(caught.value)


def test_watermark_and_query_passthrough(binding, room_fixture):
    assert binding.db.watermark(4000000001) == 0, "夹具库号从未登记水位"
    rows = binding.db.query(
        "SELECT count() FROM pe WHERE deleted = false GROUP ALL;"
    )[0]
    assert rows and rows[0]["count"] >= 8, rows


def test_room_lookups_after_full_build(binding, room_fixture, cwall):
    """`fn::get_room_nodes` / `get_room_names` 问的是「**我名下**的东西穿过哪些
    房间」（`$id<-pe_owner.in<-room_relate`），所以要拿容器问，不是拿叶子问——
    对 BOX 自己调恒为空，这是语义不是 bug。"""
    binding.model.update_aabbs(sorted(_bodies(room_fixture)), replace=True)
    binding.room.build_all()

    assert room_fixture["room_num"] in binding.room.names(cwall)
    assert binding.room.names(room_fixture["in_a"][0]) == [], "叶子件名下无物"

    # `get_room_nodes` 取的是面板 `pe` 行上的 `refno` **字段**（`in.refno`），
    # 而合成夹具的 pe 行只写了 noun/owner/name/deleted——所以这里只能验通路，
    # 验不了内容（真实库上才有 refno 字段）。
    assert isinstance(binding.room.nodes(cwall), list)

    code = binding.room.code(room_fixture["in_a"][0])
    assert code is None or isinstance(code, str)


def test_spatial_tree_status_shape(binding, room_fixture):
    """只钉「跨形状都在」的稳定核。

    完整键面是 /health `spatial_tree` 渲染半边的对外承诺，权威的形状钉在 Rust
    侧（G-02 契约迁移期间它正从九键走向十五键）。Python 面跟着钉全集只会在迁移
    途中两头打架，所以这里只保证绑定确实把那份渲染原样透出来了、且判漂移要用的
    几个键在场。等迁移落定再收紧成全集。
    """
    status = binding.spatial.tree_status()
    assert {"entries", "file_epoch", "db_epoch", "drift", "startup_verdict"} <= set(
        status
    ), sorted(status)
    assert isinstance(status["entries"], int)
    assert isinstance(status["drift"], bool)


def test_queue_status_shape(binding, room_fixture):
    status = binding.incr.queue_status()
    assert isinstance(status["paused"], bool)
    assert isinstance(status["rows"], list)
