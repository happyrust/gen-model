# -*- coding: utf-8 -*-
"""房间归属的 Python 对拍测试（口径与 Rust room_fixture live 轨一致）。

硬标准（ADR-010 §9 / 测试计划 RI-12）：直写路径收敛后的规范化边集合等于同数据上
全量重建的结果，**逐边比较**，禁止只比 count；且删除本身必须可见——只比
「直写 == 全量」的话，两边同时算错成空集也会相等。

边的规范形态：(panel, element, room_num, inside_count, center_dist)，
center_dist 按 0.01mm 容差取整比较（测试计划 §3 数值容差）。

搬家两例（RS2 构件搬家 / RS3 面板搬家）与「durable 直写发布房间任务」一例随
逐目标的房间待办行退役（ADR-072 D4 / D5：房间归属由该库 dbnum 任务的房间段整库算，
绑定侧没有 `room.enqueue → room.drain` 这条触发路了）；对拍口径由 Rust 侧
`room_fixture.rs` 的直调元素分支用例接着守。
"""

from __future__ import annotations


def norm(refno) -> str:
    """refno 宽容归一到 a_b 形态（与库内 record id 一致）。"""
    return str(refno).lstrip("=").replace("/", "_")


def edges(db) -> list[tuple]:
    rows = db.query(
        "SELECT record::id(in) AS panel, record::id(out) AS element, "
        "room_num, inside_count, center_dist "
        "FROM room_relate ORDER BY panel, element;"
    )[0]
    return sorted(
        (
            row["panel"],
            row["element"],
            row["room_num"],
            row["inside_count"],
            round(float(row["center_dist"]), 2),
        )
        for row in rows
    )


def membership_pairs(edge_rows) -> set[tuple]:
    return {(panel, element) for (panel, element, *_rest) in edge_rows}


def spatial_epoch(db) -> int:
    # `value` 是 SurrealQL 保留字，必须反引号（aabb_tree.rs 同款教训）。
    rows = db.query("SELECT `value` FROM spatial_epoch:current;")[0]
    return int(rows[0]["value"]) if rows else 0


def all_refnos(fx) -> list[str]:
    return list(fx["in_a"]) + list(fx["in_b"]) + [
        fx["straddler"],
        fx["pane_a"],
        fx["pane_b"],
    ]


def baseline(aios_db, fx) -> list[tuple]:
    """包围盒进树 + 全量重建基线（6 条边：A×3、B×3，跨界件两室都收）。

    replace=True 是双保险：既覆盖「行里已有指针」的过滤，也把上一条用例可能
    留在进程级 GLOBAL_AABB_TREE 里的旧条目同步掉（pytest 单进程共享一棵树，
    与 Rust 夹具 fixture_baseline 同一前提）。
    """
    aios_db.model.update_aabbs(all_refnos(fx), replace=True)
    aios_db.room.build_all()
    base = edges(aios_db.db)
    assert len(base) == 6, f"基线应有 6 条成员边: {base}"
    pairs = membership_pairs(base)
    assert (fx["pane_a"], norm(fx["straddler"])) in pairs
    assert (fx["pane_b"], norm(fx["straddler"])) in pairs, "跨界件应两室都收"
    return base


# ── 用例 ─────────────────────────────────────────────────────────────────────


def test_noop_refresh_produces_no_changes(binding, room_fixture):
    """RF9 负例：没动几何的重刷不算变——差异信号是房间增量的触发源，误报
    等于给根下每个元素白排一次房间任务。"""
    fx = room_fixture
    baseline(binding, fx)

    changes = binding.model.update_aabbs(all_refnos(fx), replace=True)
    assert changes == [], f"逐位相等的重刷不该产生变更: {changes}"


def test_delete_clears_membership_and_bumps_epoch(binding, room_fixture):
    """RF4 + 直写删除留痕（2026-08-12 改动的首个行为级覆盖）：删除立即清边、
    摘树，且在库侧留下 spatial epoch 痕迹——不 bump 的话，落盘前崩溃的重启会
    按指纹相等复用陈旧树，被删构件借启动全量房间重建还魂（ADR-010 D4）。"""
    fx = room_fixture
    baseline(binding, fx)
    victim = norm(fx["in_a"][0])

    epoch_before = spatial_epoch(binding.db)
    entries_before = binding.spatial.tree_status()["entries"]

    binding.model.delete_subtree([victim])

    remaining = edges(binding.db)
    assert all(element != victim for (_p, element, *_r) in remaining), (
        f"被删构件的归属边必须当场清掉: {remaining}"
    )
    tree_after = binding.spatial.tree_status()
    assert tree_after["entries"] < entries_before, (
        f"被删构件必须从空间树摘掉: {entries_before} -> {tree_after['entries']}"
    )
    assert spatial_epoch(binding.db) > epoch_before, (
        "直写删除必须在同事务里递增 spatial epoch（无痕迹 = 崩溃后静默漂移）"
    )

    # 删除后的边集与全量重建一致（5 条：A×2、B×3）。
    binding.room.build_all()
    full = edges(binding.db)
    assert remaining == full, f"删除后增量 != 全量:\n{remaining}\n{full}"
    assert len(full) == 5, full


def test_durable_update_bumps_epoch_without_a_room_queue(binding, room_fixture):
    """durable 直写：AABB 指针与 spatial epoch 同事务（生产 TransformOnly / 定向 regen
    的路径）；库里不再长出逐目标的房间待办行（ADR-072 D5）。"""
    fx = room_fixture
    baseline(binding, fx)
    straddler = norm(fx["straddler"])

    epoch_before = spatial_epoch(binding.db)
    # 搬进 A 的独占区（真变化）。
    binding.fixture.move_body(fx["seqs"]["straddler"], (300, 450, 450), (400, 550, 550))
    changes = binding.model.update_aabbs([straddler], replace=True, durable=True)
    assert [norm(change["refno"]) for change in changes] == [straddler], changes

    assert spatial_epoch(binding.db) > epoch_before, "durable 直写必须 bump epoch"
    tables = binding.db.query("INFO FOR DB;")[0]["tables"]
    assert "model_update_pending" not in tables, (
        f"退役的模型意图队列表不许被直写路径重新建出来: {sorted(tables)}"
    )

    # 归属由全量路给出，且搬家可见。
    binding.room.build_all()
    full = edges(binding.db)
    pairs = membership_pairs(full)
    assert (fx["pane_a"], straddler) in pairs
    assert (fx["pane_b"], straddler) not in pairs
