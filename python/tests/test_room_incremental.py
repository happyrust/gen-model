# -*- coding: utf-8 -*-
"""房间增量的 Python 对拍测试（口径与 Rust room_fixture live 轨一致）。

硬标准（ADR-010 §9 / 测试计划 RI-12）：增量收敛后的规范化边集合等于同数据上
全量重建的结果，**逐边比较**，禁止只比 count；且搬家/删除本身必须可见——只比
「增量 == 全量」的话，两边同时算错成空集也会相等。

边的规范形态：(panel, element, room_num, inside_count, center_dist)，
center_dist 按 0.01mm 容差取整比较（测试计划 §3 数值容差）。
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


def room_pending_ids(db) -> list[str]:
    rows = db.query("SELECT record::id(id) AS id FROM model_update_pending;")[0]
    return sorted(
        row["id"] for row in rows if str(row["id"]).startswith("room_recalc_")
    )


# ── 用例 ─────────────────────────────────────────────────────────────────────


def test_element_move_parity(binding, room_fixture):
    """RS2：构件 A→B 搬家，增量收敛 == 全量重建，且搬家可见。"""
    fx = room_fixture
    baseline(binding, fx)
    straddler = norm(fx["straddler"])

    # 跨界件（原骑在 A/B 重叠区）整体搬进 B 的独占区。
    binding.fixture.move_body(fx["seqs"]["straddler"], (1450, 450, 450), (1550, 550, 550))
    changes = binding.model.update_aabbs([straddler], replace=True)
    assert [norm(change["refno"]) for change in changes] == [straddler], (
        f"变更集应恰为搬动的跨界件: {changes}"
    )

    assert binding.room.enqueue(changes) == 1
    report = binding.room.drain()
    assert not report["failures"], f"房间消费不该有失败: {report}"

    incremental = edges(binding.db)
    pairs = membership_pairs(incremental)
    assert (fx["pane_b"], straddler) in pairs, f"B 房应收下搬家件: {incremental}"
    assert (fx["pane_a"], straddler) not in pairs, f"A 房应放走搬家件: {incremental}"

    binding.room.build_all()
    full = edges(binding.db)
    assert incremental == full, f"增量 != 全量:\n增量 {incremental}\n全量 {full}"


def test_panel_move_parity(binding, room_fixture):
    """RS3：面板移动走整间分支，跨界件掉出该房，与全量逐边一致。"""
    fx = room_fixture
    baseline(binding, fx)
    straddler = norm(fx["straddler"])

    # B 面板整体右移：22/23 仍在其内，跨界件掉出。
    binding.fixture.move_body(fx["seqs"]["pane_b"], (1400, 0, 0), (2400, 1000, 1000))
    changes = binding.model.update_aabbs([fx["pane_b"]], replace=True)
    assert len(changes) == 1 and changes[0]["noun"] == "PANE", changes

    binding.room.enqueue(changes)
    report = binding.room.drain()
    assert not report["failures"], f"整间分支不该失败: {report}"

    incremental = edges(binding.db)
    pairs = membership_pairs(incremental)
    assert (fx["pane_b"], straddler) not in pairs, f"跨界件应掉出 B: {incremental}"
    assert (fx["pane_a"], straddler) in pairs, f"A 房归属不该被牵连: {incremental}"
    for part in fx["in_b"]:
        assert (fx["pane_b"], norm(part)) in pairs, f"B 房存量成员不该丢: {incremental}"

    binding.room.build_all()
    full = edges(binding.db)
    assert incremental == full, f"增量 != 全量:\n增量 {incremental}\n全量 {full}"


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


def test_durable_update_publishes_room_task_and_bumps_epoch(binding, room_fixture):
    """durable 直写：AABB 指针、room_recalc 任务、spatial epoch 同事务
    （生产 TransformOnly / 定向 regen 的路径）；随后 drain 收敛到与全量一致。"""
    fx = room_fixture
    baseline(binding, fx)
    straddler = norm(fx["straddler"])

    epoch_before = spatial_epoch(binding.db)
    # 搬进 A 的独占区（真变化）。
    binding.fixture.move_body(fx["seqs"]["straddler"], (300, 450, 450), (400, 550, 550))
    changes = binding.model.update_aabbs([straddler], replace=True, durable=True)
    assert [norm(change["refno"]) for change in changes] == [straddler], changes

    assert spatial_epoch(binding.db) > epoch_before, "durable 直写必须 bump epoch"
    pending = room_pending_ids(binding.db)
    assert f"room_recalc_element_{straddler}" in pending, (
        f"room_recalc 任务应随直写事务发布（room_incremental=true）: {pending}"
    )

    report = binding.room.drain()
    assert not report["failures"], report
    assert room_pending_ids(binding.db) == [], "消费后队列应清空"

    incremental = edges(binding.db)
    pairs = membership_pairs(incremental)
    assert (fx["pane_a"], straddler) in pairs
    assert (fx["pane_b"], straddler) not in pairs

    binding.room.build_all()
    full = edges(binding.db)
    assert incremental == full, f"增量 != 全量:\n增量 {incremental}\n全量 {full}"
