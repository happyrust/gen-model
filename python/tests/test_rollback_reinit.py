# -*- coding: utf-8 -*-
"""回退与幽灵水位默认整库重建（ADR-021）的 Python 闭环。

走的是与服务完全相同的机器：`incr.execute_manual`（扫描 + 入队 + worker 消费到
队列空）。三条用例对应 ADR-021 的三条承诺：

- **回退**（`file_latest < applied`）：不阻断——入队一条重建批次，worker 冻结点
  复核后整库清空（幸存行也不留）再按首次导入重新解析当前文件；
- **幽灵水位**（`applied > 0` 而 `pe` 零行）：路由到基线而不是增量窗口（增量只会
  把 `1..applied` 永远漏掉）；
- **身份歧义**（类型变更）：照旧阻断，绝不自动清库。

跑在 conftest 自起的一次性内存 SurrealDB @8071（房间增量档同款），不碰
8009 / 8019 / 9099。靶库是 testbed 副本里最小的设计库 7998（本模块会对它做
真实的基线 / 清空 / 重建，session 结束内存库整体丢弃，零残留）。

引导成本：模块级 fixture 先跑一遍 `execute_manual()` 让 bootstrap 范围解析出
SYS meta（MDB /ALL 由此可解），再按子集把 7998 建成基线——只付一次。
"""

from __future__ import annotations

import time

import pytest

DBNUM = 7998
# testbed ams7998 的 Ref0（今日 live 实测：唯一元素 pe:16190_1）。标记行借用同一
# Ref0 段的保留高位 idx，真实工程不会用到；整库清空按 Ref0 区间删，正好把它们
# 一并收走——这就是「全删而不是只删高于文件水位」的证据面。
REF0 = 16190
SURVIVOR = f"pe:{REF0}_999999901"
GHOST = f"pe:{REF0}_999999902"


def _pe_count(binding) -> int:
    rows = binding.db.query(
        f"SELECT count() AS count FROM pe WHERE dbnum = {DBNUM} GROUP ALL;"
    )[0]
    return rows[0]["count"] if rows else 0


def _cleanup_markers(binding) -> None:
    binding.db.query(f"DELETE {SURVIVOR}; DELETE {GHOST};")


@pytest.fixture(scope="module")
def reinit_baseline(binding):
    """一次引导：SYS meta 解析（撑起 MDB 范围）→ 7998 首次导入基线。

    返回 `{file_latest, full_count}`：`file_latest` 是文件最新会话号（后续用例的
    对齐判据），`full_count` 是完整基线的 pe 行数——「重建后行数回到 full_count」
    是区分基线与增量重放的证据（增量窗口重放不出建库早期的元素）。
    """
    started = time.monotonic()
    bootstrap = binding.incr.execute_manual()
    receipt = binding.incr.execute_manual(dbnums=[DBNUM])["receipt"]
    rejected = [w for w in receipt["warnings"] if f"dbnum={DBNUM}" in w]
    assert not rejected, (
        f"7998 应在 MDB 范围内（SYS 引导后），却被拒: {rejected}；"
        f"bootstrap 回执: {bootstrap['receipt']['warnings']}"
    )

    file_latest = binding.db.query(
        f"SELECT VALUE file_latest_sesno FROM dbnum_watermark:{DBNUM};"
    )[0][0]
    applied = binding.db.watermark(DBNUM)
    full_count = _pe_count(binding)
    print(
        f"引导完成 {time.monotonic() - started:.1f}s："
        f"applied={applied} file_latest={file_latest} pe={full_count}"
    )
    assert applied == file_latest > 0, "引导基线必须把水位推到文件水位"
    assert full_count >= 1, "引导基线必须留下数据"
    return {"file_latest": file_latest, "full_count": full_count}


def test_rollback_wipes_and_rebuilds_from_the_current_file(binding, reinit_baseline):
    """回退：入队不删数据 → worker 整库清空（幸存行也不留）→ 首次导入重建。"""
    file_latest = reinit_baseline["file_latest"]
    full_count = reinit_baseline["full_count"]
    _cleanup_markers(binding)
    # 两枚标记行：幸存位（sesno=1，缝合式对齐会保留它）与幽灵位（高于文件水位）。
    # 整库重建后两枚都必须物理消失——这正是 ADR-021 与旧 prune 方案的分界线。
    binding.db.query(
        f"CREATE {SURVIVOR} SET dbnum = {DBNUM}, sesno = 1, noun = 'ZZWM', "
        f"name = '/zz-py-reinit-survivor'; "
        f"CREATE {GHOST} SET dbnum = {DBNUM}, sesno = {file_latest + 3}, "
        f"noun = 'ZZWM', name = '/zz-py-reinit-ghost'; "
        f"UPDATE dbnum_watermark:{DBNUM} SET applied_sesno = {file_latest + 7}, "
        f"sesno = {file_latest + 7};"
    )
    try:
        assert binding.db.watermark(DBNUM) == file_latest + 7, "回退夹具必须先成立"

        outcome = binding.incr.execute_manual(dbnums=[DBNUM])
        receipt = outcome["receipt"]

        assert not receipt["blocked"], f"回退不再阻断: {receipt['blocked']}"
        assert any(
            "整库重建" in warning for warning in receipt["warnings"]
        ), f"入队回执必须点名重建: {receipt['warnings']}"
        assert outcome["drained"] >= 1, "重建批次必须被 worker 消费"

        assert binding.db.watermark(DBNUM) == file_latest, "重建后水位对齐文件"
        exists = binding.db.query(
            f"RETURN [record::exists({SURVIVOR}), record::exists({GHOST})];"
        )[0]
        assert exists == [False, False], (
            "整库清空：幸存位与幽灵位都必须物理消失（只删幽灵 = 退回缝合式对齐）"
        )
        assert _pe_count(binding) == full_count, "重建后内容回到完整基线"
    finally:
        _cleanup_markers(binding)


def test_ghost_watermark_reroutes_to_the_baseline(binding, reinit_baseline):
    """幽灵水位（applied>0、pe 零行、file>applied）：走基线，不走增量窗口。"""
    file_latest = reinit_baseline["file_latest"]
    full_count = reinit_baseline["full_count"]
    ghost_applied = max(file_latest - 2, 1)
    binding.db.query(
        f"DELETE pe WHERE dbnum = {DBNUM}; "
        f"UPDATE dbnum_watermark:{DBNUM} SET applied_sesno = {ghost_applied}, "
        f"sesno = {ghost_applied};"
    )
    assert _pe_count(binding) == 0, "幽灵水位夹具要求 pe 零行"

    outcome = binding.incr.execute_manual(dbnums=[DBNUM])
    receipt = outcome["receipt"]

    assert not receipt["blocked"], f"幽灵水位不是异常，不得阻断: {receipt['blocked']}"
    assert outcome["drained"] >= 1, "批次必须被 worker 消费"
    assert binding.db.watermark(DBNUM) == file_latest, "重建后水位对齐文件"
    # 证据面：增量窗口只重放 ghost_applied+1..file_latest，重放不出建库早期的
    # 元素，行数到不了 full_count；只有首次导入基线能把全量内容找回来。
    assert _pe_count(binding) == full_count, (
        "幽灵水位必须按首次导入重建全量内容，而不是增量重放最后两个会话"
    )


def test_identity_anomalies_still_block(binding, reinit_baseline):
    """身份歧义（类型变更）照旧阻断：不入队、不清库、水位纹丝不动。"""
    file_latest = reinit_baseline["file_latest"]
    full_count = reinit_baseline["full_count"]
    binding.db.query(f"UPDATE dbnum_watermark:{DBNUM} SET db_type = 'SYST';")
    try:
        outcome = binding.incr.execute_manual(dbnums=[DBNUM])
        receipt = outcome["receipt"]

        blocked = [row for row in receipt["blocked"] if row["dbnum"] == DBNUM]
        assert blocked and "类型变更" in blocked[0]["reason"], (
            f"类型变更必须阻断并说明理由: {receipt['blocked']}"
        )
        assert binding.db.watermark(DBNUM) == file_latest, "阻断时水位纹丝不动"
        assert _pe_count(binding) == full_count, "阻断时数据一行不动（绝不自动清库）"
    finally:
        binding.db.query(f"UPDATE dbnum_watermark:{DBNUM} SET db_type = 'DESI';")
