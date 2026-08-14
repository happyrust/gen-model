# -*- coding: utf-8 -*-
"""净窗口 vs 逐会话回放的 live A/B 全链路执行（ADR-022 验收 3，切默认值前的证据）。

窗口执行走与服务完全相同的机器（`incr.execute_manual`：扫描 + 入队 + worker
冻结 + ADR-017 暂存窗口 + 窗口内模型生成 + 提交 + 水位收口），在 conftest 自起
的一次性内存 SurrealDB @8071 上对**同一起始库态、同一增量窗口**各执行一遍，断言
库终态等价。靶库 8000（testbed 副本，会话链 209，真实删除/批量修改史）。

每臂序列（两臂只差执行窗口时的 `AIOS_NET_WINDOW`，Rust 侧每次调用现读 env）：

a. **臂重置**：按 `fast_delete::render_delete_phases` 的三阶段语句逐字镜像清库
   （Ref0 区间寻址——连 dbnum 字段缺失的墓碑/幻影残留一并收走），再走生产基线
   入口 `aios_db.sync.baseline`（`initialize_project_dbnum_baseline`，与 watcher/
   手动更新同一入口）首次导入重建。**为什么不直接走 ADR-021 回退批次**：那条路
   的重建半边会把基线登记的全部交付根（本库 2,229 个）在批次内当场生成，debug
   构建单臂 30 分钟起（2026-08-13 run1 实测日志 `.scratch/net-ab-run.log`：
   `ModelRefreshPolicy: 生成模型，根数量: 2229` 后 25 分钟未完被杀）——基线生成
   不是本用例的被测对象，两臂等价所需的只是「起点是文件的确定函数」，由断言 0
   （两臂重置后签名逐维相等）直接钉住；
b. **拨回水位**造增量窗口（只动水位不动数据，窗口重放幂等，ADR-001）：K 取
   `file_latest // 2`（`AIOS_AB_K` 可覆盖）；
c. 设本臂口径 env → 预览探口径自报（预览与执行共用 `collect_window` 唯一入口，
   ADR-011 同谓词；执行体的口径标注只进批次回执，Python 侧拿不到）→
   `execute_manual` 执行窗口 → 水位必须回到 `file_latest`；
d. 终态签名（**按 Ref0 record-id 区间取 pe**，与 `fast_delete` 同一寻址——
   `WHERE dbnum` 会漏掉 `UPDATE pe:{id} SET deleted=true` 在缺行时造出的
   无 dbnum 墓碑/幻影行，而那正是两套口径分歧的所在）。

等价判据：**逐维度严格相等**，例外必须逐条归因到 ADR-022 §5 的明示行为变化或
其背景一节立案的回放盲区家族，仲裁者是文件本身（`parse.net_changes` 的净三态与
生产 B+ 点查逐字对齐 + `parse.element` 两端内容比对）。任何归因不了的差异都是
断言失败——静默跳过 = 缺陷。

**opt-in**：设 `AIOS_NET_AB=1` 才跑（每臂一次 8000 基线重建 + 窗口内真实模型
生成，全程分钟级；全量绑定档的 80 绿秒级基线不能被它拖爆）。跑法：

    cd python
    $env:AIOS_NET_AB = '1'
    .venv\\Scripts\\python.exe -m pytest tests/test_net_window_ab.py -q -s

**并发禁忌**：T11b（`test_net_and_replay_agree_on_a_stock_deletion`）会就地**原子替换**
固定的 `DB_FILE`（同卷临时文件 + `os.replace`）并切换进程级 `AIOS_NET_WINDOW` env——
**禁止 pytest-xdist 并行**跑本文件（`-n>1` 会让两用例互相踩文件/env）；全链 A/B 与
T11b 亦不可并行。切换全走原子替换：进程即便被 kill 也只会看到「旧文件」或「新文件」，
绝不留下截断的源库。
"""

from __future__ import annotations

import json
import os
import shutil
import struct
import tempfile
import time
from pathlib import Path

import pytest

import _session_snapshot as snap

# live A/B 证据采集腿：分钟级，只在显式 AIOS_NET_AB=1 时跑（不进常规批）。
# 不用 module 级 pytestmark——本文件另有一条纯离线的切割镜像单测（T11b 的地基）
# 要在 offline 档常跑，module 级 skip 会把它一起挡掉。
LIVE = pytest.mark.skipif(
    os.environ.get("AIOS_NET_AB") != "1",
    reason="live A/B 证据采集用例：设 AIOS_NET_AB=1 显式开跑（分钟级，不进常规批）",
)

DBNUM = 8000
REPO_ROOT = Path(__file__).resolve().parents[2]
DB_FILE = (
    Path(__file__).resolve().parents[1]
    / "testbed" / "projects" / "AvevaMarineSample" / "ams000" / "ams8000_0001"
)
# 与 fast_delete::RANGE_END 同一形制：ref1 < 2^32 < 9999999999，区间必然盖满。
RANGE_END = 9999999999
# 与 fast_delete::RANGE_TABLES 同序（pe 最后删，owner 边先于一切）。
RANGE_TABLES = [
    "inst_relate",
    "tubi_relate",
    "room_relate",
    "room_panel_relate",
    "ref_rev",
    "geo_relate",
]
CHUNK = 400
NET_ENV = "AIOS_NET_WINDOW"


# ── T11b 地基：Python 历史快照切割镜像的离线不变量 ──────────────────────────

def _synthetic_two_sessions() -> bytes:
    """与 `session_cut.rs::synthetic_two_sessions` 同构：page0 头，page1=sesno7
    (latest_page=1)，page2 数据页，page3=sesno8(previous=1, latest_page=3)，头指针=3。"""
    data = bytearray(snap.PAGE_SIZE * 4)
    struct.pack_into(">I", data, snap.HEADER_SESSION_PAGE_OFFSET, 3)
    p1 = snap.PAGE_SIZE
    struct.pack_into(">I", data, p1 + 4, 0)  # previous
    struct.pack_into(">I", data, p1 + 12, 7)  # sesno
    struct.pack_into(">I", data, p1 + 20, 1)  # latest_page
    p3 = snap.PAGE_SIZE * 3
    struct.pack_into(">I", data, p3 + 4, 1)
    struct.pack_into(">I", data, p3 + 12, 8)
    struct.pack_into(">I", data, p3 + 20, 3)
    return bytes(data)


@pytest.mark.offline
def test_python_session_cut_mirrors_the_rust_authority_on_a_synthetic_file():
    """Python 切割镜像与 Rust `session_cut` 同算法（离线常跑，T11b 存量切片的地基）。

    断言集与 `session_cut.rs` 的 `walks_the_chain_and_reports_latest` +
    `snapshot_truncates_and_rewrites_the_header_pointer` 逐条对齐：任一 offset /
    截断规则漂移这里立刻红。真实文件上与 Rust `inspect` 的对拍在 T11b live 用例里做。
    """
    data = _synthetic_two_sessions()

    latest, cuts = snap.session_chain(data)
    assert latest == 8
    assert set(cuts) == {7, 8}
    assert cuts[7] == (1, 1)  # (session_page, latest_page)
    assert cuts[8] == (3, 3)

    snap7 = snap.cut_bytes(data, 7)
    assert len(snap7) == snap.PAGE_SIZE * 2, "sesno7 截断到 latest_page+1=2 页"
    assert (
        snap._be_u32(snap7, snap.HEADER_SESSION_PAGE_OFFSET) == 1
    ), "头指针回写到 sesno7 的 session page"
    re_latest, re_cuts = snap.session_chain(snap7)
    assert re_latest == 7 and set(re_cuts) == {7}, "切出的快照本身是合法的 @7 文件"

    assert snap.cut_bytes(data, 8) == data, "切最新会话 == 原文件逐字节相等"

    for bad in (9, 0):
        with pytest.raises(ValueError):
            snap.cut_bytes(data, bad)


def _query(binding, sql: str):
    rows = binding.db.query(sql)
    return rows[0] if rows else []


def _watermark(binding) -> int:
    return binding.db.watermark(DBNUM)


def _pe_count(binding) -> int:
    rows = _query(
        binding, f"SELECT count() AS count FROM pe WHERE dbnum = {DBNUM} GROUP ALL;"
    )
    return rows[0]["count"] if rows else 0


def _chunks(items: list, size: int):
    for start in range(0, len(items), size):
        yield items[start : start + size]


def _normalize(value):
    """pythonize 出来的行归一成可哈希形态（dict 键排序、list 保序）。"""
    if isinstance(value, dict):
        return tuple(sorted((key, _normalize(item)) for key, item in value.items()))
    if isinstance(value, list):
        return tuple(_normalize(item) for item in value)
    return value


def _wipe_mirroring_reinit(binding, ref0s: list[int]) -> None:
    """`fast_delete::render_delete_phases(ResetForReinit)` 的逐字镜像。

    三阶段同序：owner 边 → Ref0 区间（关系表 + noun 表 + pe 收尾）→ 元数据
    （队列残留 + 统计 + spatial epoch 递增 + 水位清值不删行）。区别仅两处，都比
    生产口径**多删不少删**：info 行额外按 ref0 记录 id 点名（墓碑创建事件会把
    info 行的 dbnum MERGE 成 NONE，`WHERE dbnum` 够不着这种被打脏的行）；noun
    表按当前店内 GROUP BY noun 现查（与生产同源）。
    """
    nouns = sorted(
        {
            str(row["noun"])
            for row in _query(
                binding, f"SELECT noun FROM pe WHERE dbnum = {DBNUM} GROUP BY noun;"
            )
            if row.get("noun")
        }
    )
    for noun in nouns:
        assert noun.replace("_", "").isalnum(), f"noun 表名不合法，拒绝拼语句: {noun}"

    relations = []
    ranges = []
    for ref0 in ref0s:
        pe_range = f"pe:{ref0}_0..{ref0}_{RANGE_END}"
        relations.append(
            f"DELETE array::flatten(SELECT VALUE ->pe_owner FROM {pe_range});"
        )
        relations.append(
            f"DELETE array::flatten(SELECT VALUE <-pe_owner FROM {pe_range});"
        )
        for table in RANGE_TABLES + nouns:
            ranges.append(f"DELETE {table}:{ref0}_0..{ref0}_{RANGE_END};")
        ranges.append(f"DELETE {pe_range};")
    metadata = [
        f"DELETE model_update_pending WHERE dbnum = {DBNUM};",
        f"DELETE increment_update_attempt WHERE dbnum = {DBNUM};",
        f"DELETE incr_side_effect_pending WHERE dbnum = {DBNUM};",
        f"DELETE dbnum_info_table WHERE dbnum = {DBNUM};",
    ]
    metadata.extend(f"DELETE dbnum_info_table:{ref0};" for ref0 in ref0s)
    metadata.append(
        "UPSERT spatial_epoch:current SET value = (value?:0) + 1, updated_at = time::now();"
    )
    metadata.append(
        f"UPDATE dbnum_watermark:{DBNUM} SET applied_sesno = 0, sesno = 0, "
        f"applied_sesno_time = NONE;"
    )
    binding.db.query("\n".join(relations))
    binding.db.query("\n".join(ranges))
    binding.db.query("\n".join(metadata))
    assert _pe_count(binding) == 0, "镜像清库后 pe 必须零行"


def _reset_arm(binding, ref0s: list[int], file_latest: int, expect_count: int | None) -> float:
    """臂重置：镜像清库 + 生产基线入口重建 + 清空基线登记的 regen 积压。

    重置期间口径钉在 off——基线不走 `collect_window`，钉住只是杜绝「臂起点受
    口径影响」的任何可能性。清积压的理由：基线会把全库 2,229 个交付根登记进
    `model_update_pending`，而下一个数据批次会把 pending 积压并进自己的模型
    计划一起生成（2026-08-13 run4 实测：两臂窗口批各花 22 分钟把 2,229 根全量
    重生成一遍）——那是基线补课，不是窗口 A/B 的被测面，且两臂同样清空、起点
    仍由断言 0 钉住同构；窗口自己登记/结算的计划行照常进签名。
    """
    os.environ[NET_ENV] = "off"
    started = time.monotonic()
    _wipe_mirroring_reinit(binding, ref0s)
    receipt = binding.sync.baseline(DBNUM)
    binding.db.query(f"DELETE model_update_pending WHERE dbnum = {DBNUM};")
    elapsed = time.monotonic() - started
    assert receipt["dbnum"] == DBNUM
    assert _watermark(binding) == file_latest, "重建后水位对齐文件"
    count = _pe_count(binding)
    if expect_count is not None:
        assert count == expect_count, (
            f"重建后行数 {count} != 完整基线 {expect_count}——臂起点不同构"
        )
    return elapsed


def _dump(binding, ref0s: list[int]) -> dict:
    """终态签名。pe 按 Ref0 区间取全量行（含 dbnum 缺失的墓碑/幻影），其余维度
    锚定在「有 noun 的活行」平面上。"""
    rows: dict[str, dict] = {}
    for ref0 in ref0s:
        for row in _query(
            binding,
            f"SELECT record::id(id) AS key, sesno, noun, name, dbnum, deleted "
            f"FROM pe:{ref0}_0..{ref0}_{RANGE_END};",
        ):
            rows[str(row["key"])] = {
                "sesno": row.get("sesno"),
                "noun": row.get("noun"),
                "name": row.get("name"),
                "dbnum": row.get("dbnum"),
                "deleted": bool(row.get("deleted")),
            }

    live_ids = sorted(key for key, row in rows.items() if not row["deleted"])

    # 活行的 noun 属性表内容（Add 的 UPSERT CONTENT 与 Modified 的 UPSERT MERGE
    # 的最终态都在这里）。幻影裸行没有 noun，自然不进这一层。
    by_noun: dict[str, list[str]] = {}
    for key in live_ids:
        noun = (rows[key]["noun"] or "").strip()
        if noun:
            by_noun.setdefault(noun, []).append(key)
    attrs: dict[str, tuple] = {}
    for noun, noun_ids in sorted(by_noun.items()):
        for chunk in _chunks(sorted(noun_ids), CHUNK):
            id_list = ", ".join(f"'{i}'" for i in chunk)
            for row in _query(
                binding,
                f"SELECT * FROM type::table('{noun}') "
                f"WHERE record::id(id) IN [{id_list}] LIMIT {CHUNK + 1};",
            ):
                # pythonize 出来的 id 是 "BOX:24384_26184" 全称；键只留 noun:refno，
                # 归因豁免按 refno 尾段截取才对得上盲区集合。
                thing = str(row.pop("id", None)).rsplit(":", 1)[-1]
                attrs[f"{noun}:{thing}"] = _normalize(row)

    # 属主边含边 id（复合 id [pe:{owner}, 序号]——成员列表的顺序是语义）。
    owner_edges: set[tuple] = set()
    for chunk in _chunks(live_ids, CHUNK):
        targets = ", ".join(f"pe:{i}" for i in chunk)
        for row in _query(
            binding,
            f"SELECT <string>in AS child, <string>id AS edge, <string>out AS parent "
            f"FROM pe_owner WHERE out IN [{targets}] LIMIT 200000;",
        ):
            owner_edges.add((row["parent"], row["edge"], row["child"]))

    ref_edges: set[tuple] = set()
    for chunk in _chunks(live_ids, CHUNK):
        sources = ", ".join(f"pe:{i}" for i in chunk)
        for row in _query(
            binding,
            f"SELECT <string>in AS src, <string>out AS dst "
            f"FROM ref_rev WHERE in IN [{sources}] LIMIT 200000;",
        ):
            ref_edges.add((row["src"], row["dst"]))

    pending = {
        (row["action"], row["target_refno"], row.get("source_end_sesno"))
        for row in _query(
            binding,
            f"SELECT action, target_refno, source_end_sesno FROM model_update_pending "
            f"WHERE dbnum = {DBNUM} LIMIT 200000;",
        )
    }

    # dbnum_info_table 按 Ref0 记账（record id 就是 ref0 数字）。dbnum / max_ref1
    # 两个字段刻意不进签名：update_dbnum_event 对两者都是「最后一个事件说了算」
    # （墓碑创建时 $value.dbnum 是 NONE、max_ref1 直接覆写不是取 max），谁的语句
    # 排在末尾就是谁——这是事件实现的既有噪声，与收集口径无关（证据文档点名）。
    info = {
        int(row["ref0"]): {"count": row.get("count"), "sesno": row.get("sesno")}
        for row in _query(
            binding,
            f"SELECT record::id(id) AS ref0, count, sesno FROM dbnum_info_table "
            f"WHERE record::id(id) IN [{', '.join(str(r) for r in ref0s)}];",
        )
    }

    watermark_rows = _query(
        binding,
        f"SELECT applied_sesno, sesno, file_latest_sesno FROM dbnum_watermark:{DBNUM};",
    )

    return {
        "rows": rows,
        "live_ids": live_ids,
        "attrs": attrs,
        "owner_edges": owner_edges,
        "ref_edges": ref_edges,
        "pending": pending,
        "info": info,
        "watermark": _normalize(watermark_rows),
    }


def _classify_pe_rows(replay: dict, net: dict, oracle: dict[str, str]):
    """两臂 pe 全量行差异 → 逐条归因（ADR-022 §5 与其背景一节立案的盲区家族）。

    oracle 是文件仲裁者：`parse.net_changes` 的净三态（与生产 B+ 点查逐字对齐，
    live 对拍零分歧）。返回 (归因桶, 未归因清单, 参与归因的盲区 refno 集)。
    """
    buckets: dict[str, list[str]] = {}
    unexplained: list[str] = []
    blind_refnos: set[str] = set()

    def put(bucket: str, line: str, refno: str | None = None):
        buckets.setdefault(bucket, []).append(line)
        if refno is not None:
            blind_refnos.add(refno)

    def bare(row: dict) -> bool:
        return row["noun"] is None and row["dbnum"] is None

    for key in sorted(set(replay["rows"]) | set(net["rows"])):
        r = replay["rows"].get(key)
        n = net["rows"].get(key)
        if r == n:
            continue
        cls = oracle.get(key)
        detail = f"{key}: 回放={r} 净={n} 文件仲裁={cls}"

        if r and n and not r["deleted"] and not n["deleted"]:
            same_but_sesno = {kk: vv for kk, vv in r.items() if kk != "sesno"} == {
                kk: vv for kk, vv in n.items() if kk != "sesno"
            }
            if same_but_sesno and r["sesno"] != n["sesno"] and cls in ("modified", "added"):
                # §5.5 口径：pe.sesno 的 last-touch 来源不同——回放取 op 流会话、净取
                # 索引记录页的会话反查（ADR-022 §5.5）。同一活行除 sesno 外逐字段相等
                # 时归一（与墓碑 sesno 归一同族，都是 §5.5 的来源切换，非数据分歧）。
                put(
                    "§5 修改活行 last-touch sesno 戳位（回放=op流 / 净=记录页反查，§5.5）",
                    f"{key}: 回放 sesno={r['sesno']} 净 sesno={n['sesno']} 文件={cls}",
                )
                continue

        if r and n and r["deleted"] and n["deleted"]:
            same_identity = {k: v for k, v in r.items() if k != "sesno"} == {
                k: v for k, v in n.items() if k != "sesno"
            }
            if same_identity:
                # §5.5 归一：净墓碑 sesno 戳窗口终点（净差分判不出删除动作发生在
                # 哪个会话），回放戳删除会话——身份字段全等时对 sesno 豁免。
                put("§5 墓碑 sesno 戳位（回放=删除会话 / 净=窗口终点，已归一）",
                    f"{key}: 回放 sesno={r['sesno']} 净 sesno={n['sesno']}")
                continue
            unexplained.append(detail)
            continue

        if n and n["deleted"] and r is None:
            if cls == "deleted":
                # 回放的跨会话删除盲区（issue-019 家族；ADR-022 背景「漏报」）：
                # 元素确于窗口内被删（文件仲裁 deleted），回放 op 流一无所知。
                put("回放漏报删除（净立碑 / 回放无行）", detail, key)
                continue
            unexplained.append(detail)
            continue

        if r and r["deleted"] and n is None:
            if cls is None and bare(r):
                # §5.2：窗口内「加了又删」，净路径两端都不在场则什么都不写；
                # 回放剔除临时 Add 后剩孤儿 Deleted 腿，造出从未发布过的裸墓碑。
                put("回放临时墓碑（加了又删，§5.2 净不立碑）", detail, key)
                continue
            unexplained.append(detail)
            continue

        if r and not r["deleted"] and bare(r) and (n is None or n["deleted"]):
            if cls == "deleted" or (cls is None and n is None):
                # 孤儿 Modified 腿：临时 Add 被终态对账剔除后，Modified 腿的
                # UPSERT MERGE 在缺行处造出裸幻影行（ADR-022 背景「孤儿腿误报」）。
                put("回放孤儿 Modified 腿幻影行（净不写 / 净立碑）", detail, key)
                continue
            unexplained.append(detail)
            continue

        if r and r["deleted"] and n and not n["deleted"]:
            if cls in ("added", "modified"):
                # 最重的盲区形态：元素在窗口终点仍在场（文件仲裁），回放却立了碑
                # ——终态对账的最终索引读没读到活记录（与差分器三条实测规则同源）。
                put("回放把终点仍在场的元素立碑（数据损失形态盲区，净持真）",
                    detail, key)
                continue
            unexplained.append(detail)
            continue

        if r is None and n and not n["deleted"]:
            if cls in ("added", "modified"):
                put("回放漏报存在（净持真）", detail, key)
                continue
            unexplained.append(detail)
            continue

        if r and not r["deleted"] and not bare(r) and n and n["deleted"]:
            if cls == "deleted":
                # T11b 存量删除：元素在起点(≤K)是活的真实行、窗口内被跨会话删除。
                # 净口径正确立碑，回放漏删（issue-019 家族）把活行留着——净持文件真值。
                # 全文件基线的 A/B 永不进这支（被删元素在基线本就无行），是 T11b 专属。
                put("回放漏删存量活行（净立碑持文件真值）", detail, key)
                continue
            unexplained.append(detail)
            continue

        unexplained.append(detail)

    return buckets, unexplained, blind_refnos


def _unchanged_rewrite_refnos(binding, oracle_raw: dict, k: int, file_latest: int) -> set[str]:
    """净三态判 modified、但两端内容逐字段相同（记录被原样重写换页）的 refno。

    与 `net_window::collect_net_window` 的 `unchanged_rewrites` 同口径：这些元素
    净路径不发操作（真无事发生），回放路径却会按逐会话 diff 发 Modified（改了又
    改回 / 原样重写，ADR-022 §5.1 家族）。用 `parse.element` 在窗口两端各取一次
    属性 dump 比对；`found_sesno` 是「记录被重写过」本身，剔除后比内容。
    """
    unchanged: set[str] = set()
    for entry in oracle_raw["modified"]:
        refno = entry["refno"]
        try:
            base = binding.parse.element(str(DB_FILE), refno, sesno=k)
            latest = binding.parse.element(str(DB_FILE), refno, sesno=file_latest)
        except RuntimeError:
            continue  # 两端任一读不出内容 → 按有变化对待（宁多勿漏）
        strip = lambda dump: {key: val for key, val in dump.items() if key != "found_sesno"}
        if _normalize(strip(base)) == _normalize(strip(latest)):
            unchanged.add(refno)
    return unchanged


@pytest.fixture(scope="module")
def ab_baseline(binding):
    """一次引导：SYS meta 解析撑起 MDB 范围 → 8000 首次导入基线（生产基线入口）。

    返回 file_latest / full_count / ref0s；ref0s 供 Ref0 区间签名寻址。
    """
    assert DB_FILE.is_file(), f"testbed 项目副本缺 {DB_FILE}"
    header = binding.parse.header(str(DB_FILE))
    assert header["dbnum"] == DBNUM
    file_latest = int(header["latest_sesno"])

    started = time.monotonic()
    binding.incr.execute_manual()
    bootstrap_secs = time.monotonic() - started

    started = time.monotonic()
    receipt = binding.sync.baseline(DBNUM)
    baseline_secs = time.monotonic() - started
    assert receipt["dbnum"] == DBNUM
    assert _watermark(binding) == file_latest > 0, "引导基线必须把水位推到文件水位"
    full_count = _pe_count(binding)
    assert full_count > 0, "引导基线必须留下数据"

    ref0s = sorted(
        int(str(r["prefix"]).removeprefix("pe:"))
        for r in _query(
            binding,
            f"SELECT string::split(<string>id, '_')[0] AS prefix, count() AS count "
            f"FROM pe WHERE dbnum = {DBNUM} GROUP BY prefix;",
        )
    )
    assert ref0s, "基线必须能解出 Ref0 段"
    print(
        f"引导完成：SYS {bootstrap_secs:.1f}s + 8000 基线 {baseline_secs:.1f}s"
        f"（planned_roots={receipt['planned_roots']}），"
        f"file_latest={file_latest} pe={full_count} ref0s={ref0s}"
    )
    return {"file_latest": file_latest, "full_count": full_count, "ref0s": ref0s}


@pytest.fixture()
def net_window_env():
    """臂间切换 AIOS_NET_WINDOW，结束恢复原值——环境是进程级共享资源。"""
    original = os.environ.get(NET_ENV)
    yield
    if original is None:
        os.environ.pop(NET_ENV, None)
    else:
        os.environ[NET_ENV] = original


@LIVE
def test_net_and_replay_full_executions_land_equivalent_states(
    binding, ab_baseline, net_window_env
):
    file_latest = ab_baseline["file_latest"]
    full_count = ab_baseline["full_count"]

    k = int(os.environ.get("AIOS_AB_K", file_latest // 2))
    assert 1 <= k < file_latest, f"回拨位 K={k} 必须落在 1..{file_latest} 内"

    # 文件仲裁者：净三态（与生产 B+ 点查逐字对齐）。Ref0 段并上仲裁两侧，
    # 防「窗口触达基线没有的段」漏出签名视野。
    oracle_raw = binding.parse.net_changes(str(DB_FILE), k + 1, file_latest)
    oracle: dict[str, str] = {}
    for kind in ("added", "deleted", "modified"):
        for entry in oracle_raw[kind]:
            oracle[entry["refno"]] = kind
    ref0s = sorted(
        set(ab_baseline["ref0s"]) | {int(r.split("_")[0]) for r in oracle}
    )
    unchanged_rewrites = _unchanged_rewrite_refnos(binding, oracle_raw, k, file_latest)
    print(
        f"窗口 {k + 1}..={file_latest} 文件仲裁: added={oracle_raw['counts']['added']} "
        f"deleted={oracle_raw['counts']['deleted']} modified={oracle_raw['counts']['modified']}"
        f"（其中原样重写/改了又改回 {len(unchanged_rewrites)} 条: "
        f"{sorted(unchanged_rewrites)[:20]}）"
    )

    dumps: dict[str, dict] = {}
    base_dumps: dict[str, dict] = {}
    timings: dict[str, dict] = {}
    try:
        for mode, flag in (("replay", "off"), ("net", "on")):
            # a. 臂重置（镜像清库 + 生产基线入口）：起点是文件的确定函数。
            reset_secs = _reset_arm(binding, ref0s, file_latest, full_count)
            base_dumps[mode] = _dump(binding, ref0s)

            # b. 拨回水位造窗口：只动水位不动数据（重放幂等，ADR-001）。
            binding.db.query(
                f"UPDATE dbnum_watermark:{DBNUM} SET applied_sesno = {k}, sesno = {k};"
            )

            # c. 本臂口径。预览与执行共用 collect_window 唯一入口（ADR-011），
            #    预览 warnings 的口径自报是 Python 侧唯一探测面；off 臂反向断言。
            os.environ[NET_ENV] = flag
            preview = binding.db.preview_manual_update()
            marks = [w for w in preview["warnings"] if "净窗口" in w]
            if mode == "net":
                assert marks, (
                    f"AIOS_NET_WINDOW=on 未接通净口径（预览无口径自报）: "
                    f"{preview['warnings']}"
                )
                print(f"[net] 口径自报: {marks[0]}")
            else:
                assert not marks, f"off 档不得出现净口径自报: {marks}"

            started = time.monotonic()
            outcome = binding.incr.execute_manual(dbnums=[DBNUM])
            window_secs = time.monotonic() - started
            receipt = outcome["receipt"]
            assert not receipt["blocked"], f"{mode}: 窗口执行不应阻断: {receipt['blocked']}"
            assert outcome["drained"] >= 1, f"{mode}: 窗口批次必须被 worker 消费"
            assert _watermark(binding) == file_latest, f"{mode}: 执行后水位对齐文件"

            started = time.monotonic()
            dumps[mode] = _dump(binding, ref0s)
            dump_secs = time.monotonic() - started
            timings[mode] = {"reset": reset_secs, "window": window_secs}
            d = dumps[mode]
            created = sorted(set(d["rows"]) - set(base_dumps[mode]["rows"]))
            print(
                f"[{mode}] 重置 {reset_secs:.1f}s，窗口执行 {window_secs:.1f}s，"
                f"签名 {dump_secs:.1f}s：pe 全量 {len(d['rows'])}（活 {len(d['live_ids'])}）/ "
                f"属性表 {len(d['attrs'])} / 属主边 {len(d['owner_edges'])} / "
                f"ref_rev {len(d['ref_edges'])} / pending {len(d['pending'])}；"
                f"较基线新建 {len(created)} 行: {created[:10]}"
            )
    finally:
        # 收尾自洽：任何失败路径都不许把水位留在拨回态（数据在基线位=文件终态，
        # 把水位对回 file_latest 与数据一致；成功路径本就落在这里）。
        if _watermark(binding) != file_latest:
            binding.db.query(
                f"UPDATE dbnum_watermark:{DBNUM} SET applied_sesno = {file_latest}, "
                f"sesno = {file_latest};"
            )

    replay, net = dumps["replay"], dumps["net"]

    # 0. 两臂起点必须同构——重置的确定性是 A/B 成立的前提。
    for dim in ("rows", "attrs", "owner_edges", "ref_edges", "pending", "info", "watermark"):
        assert base_dumps["replay"][dim] == base_dumps["net"][dim], (
            f"两臂重置后的起点在 {dim} 维不同构——重建不是文件的确定函数？"
        )

    # 1. 水位。
    assert net["watermark"] == replay["watermark"], (
        f"水位行不一致: 回放={replay['watermark']} 净={net['watermark']}"
    )

    # 2. pe 全量行：严格相等，例外逐条归因（§5 + 立案盲区家族），零未归因。
    buckets, unexplained, blind_refnos = _classify_pe_rows(replay, net, oracle)
    for bucket, lines in sorted(buckets.items()):
        print(f"[归因] {bucket}：{len(lines)} 条")
        for line in lines:
            print(f"    {line}")
    assert not unexplained, (
        "pe 终态存在归因不了的差异（缺陷，逐条如下）：\n  " + "\n  ".join(unexplained)
    )

    # 3. 活行平面（两臂都活着的行）以及其属性表 / 属主边 / ref_rev：严格相等。
    #    盲区行已在 §2 归因并被排除在活行平面外（它们至多单侧在场）。
    both_live = set(replay["live_ids"]) & set(net["live_ids"])
    live_diffs = {
        key: (replay["rows"][key], net["rows"][key])
        for key in both_live
        if replay["rows"][key] != net["rows"][key]
    }
    assert not live_diffs, (
        f"两臂共同活行有 {len(live_diffs)} 条字段不一致（前 5 条）: "
        f"{json.dumps(dict(list(sorted(live_diffs.items()))[:5]), ensure_ascii=False)}"
    )

    phantom_attr_keys = {
        key
        for key in set(replay["attrs"]) | set(net["attrs"])
        if key.rsplit(":", 1)[-1] in blind_refnos
    }
    attr_diffs = {
        key: (replay["attrs"].get(key), net["attrs"].get(key))
        for key in (set(replay["attrs"]) | set(net["attrs"])) - phantom_attr_keys
        if replay["attrs"].get(key) != net["attrs"].get(key)
    }
    assert not attr_diffs, (
        f"noun 属性表不一致 {len(attr_diffs)} 条（键）: {sorted(attr_diffs)[:5]}"
    )
    assert replay["attrs"] or net["attrs"], "属性表签名为空——寻址失效，签名不可信"

    def edge_refnos(edge: tuple) -> set[str]:
        """从边元组的字符串形态里提取全部 pe refno（精确 token，不做子串匹配
        ——「24384_261」子串会误命中「24384_2610」，把真实差异豁免掉）。"""
        out: set[str] = set()
        for part in edge:
            text = str(part).replace("[", " ").replace("]", " ").replace(",", " ")
            for token in text.split():
                if token.startswith("pe:"):
                    out.add(token[3:])
        return out

    # 属主边：仅盲区 refno 的边可豁免（幻影行的 children 重写）。
    # ref_rev 另有一条已立案的口径差（2026-08-13 run2 实测抓到，13 条边）：
    # 「原样重写/改了又改回」的元素净口径不发操作（真无事发生），回放却会发
    # Modified 并顺手重建该元素的出向 ref_rev 边（DELETE + INSERT，内容与旧边
    # 相同）。生产店里这些边在窗口前就已在位（增量维护装的），重放重建等于
    # no-op；本用例的臂起点是重置后的空 ref_rev 店（基线不建 ref_rev，自愈
    # 靠«下次真实触达或全量重建»——见 build_reverse_index_statements 文档），
    # 于是差异被放大成「回放有边、净没边」。归 §5.1 家族，逐条打印留证。
    def edge_diff(name: str, extra_exempt: set[str]):
        left = replay[name] - net[name]
        right = net[name] - replay[name]
        exempt_refnos = blind_refnos | extra_exempt
        blind = lambda e: bool(edge_refnos(e) & exempt_refnos)
        left_blind = {e for e in left if blind(e)}
        right_blind = {e for e in right if blind(e)}
        if left_blind or right_blind:
            print(
                f"[归因] {name} 差异 {len(left_blind) + len(right_blind)} 条全部落在"
                f"盲区/原样重写 refno 上：仅回放 {sorted(left_blind)[:20]} "
                f"仅净 {sorted(right_blind)[:20]}"
            )
        return sorted(left - left_blind)[:5], sorted(right - right_blind)[:5], len(
            left - left_blind
        ) + len(right - right_blind)

    for name, extra in (("owner_edges", set()), ("ref_edges", unchanged_rewrites)):
        only_replay, only_net, leftover = edge_diff(name, extra)
        assert leftover == 0, (
            f"{name} 在归因后仍不一致：仅回放 {only_replay} 仅净 {only_net}"
        )
    # 净口径不得凭空多出 ref_rev 边（豁免只对「回放有、净没有」的方向成立）。
    net_only_ref = {
        e for e in net["ref_edges"] - replay["ref_edges"]
        if not (edge_refnos(e) & blind_refnos)
    }
    assert not net_only_ref, f"净口径凭空多出 ref_rev 边（缺陷）: {sorted(net_only_ref)[:5]}"

    # 4. 模型计划行：严格相等；例外仅限「目标本身是已归因盲区 refno」的行
    #    （如漏报删除对应的 delete_cleanup）——逐条打印，其余必须相等。
    def norm_target(target: str) -> str:
        return target.lstrip("=").replace("/", "_")

    pending_diff = replay["pending"] ^ net["pending"]
    blind_pending = {row for row in pending_diff if norm_target(row[1]) in blind_refnos}
    for row in sorted(blind_pending):
        side = "回放" if row in replay["pending"] else "净"
        print(f"[归因] 计划行差异（盲区目标，仅{side}侧）: {row}")
    leftover_pending = pending_diff - blind_pending
    assert not leftover_pending, (
        f"计划行在盲区归因后仍不一致 {len(leftover_pending)} 条："
        f"仅回放 {sorted(r for r in leftover_pending if r in replay['pending'])[:5]} "
        f"仅净 {sorted(r for r in leftover_pending if r in net['pending'])[:5]}"
    )

    # 5. dbnum_info_table：count 按记账恒等式核（终态 = 起点 + 本臂新建行
    #    - 本臂对基线活行立的碑——墓碑/幻影的 CREATE 事件会 +1，这是事件层
    #    对两臂一视同仁的规则）；sesno 两臂直接相等。
    for mode in ("replay", "net"):
        end, base = dumps[mode], base_dumps[mode]
        for ref0 in ref0s:
            created = sum(
                1
                for key in end["rows"]
                if key not in base["rows"] and key.startswith(f"{ref0}_")
            )
            tombstoned_live = sum(
                1
                for key, row in end["rows"].items()
                if row["deleted"]
                and key in base["rows"]
                and not base["rows"][key]["deleted"]
                and key.startswith(f"{ref0}_")
            )
            expect = (base["info"].get(ref0) or {"count": 0})["count"] + created - tombstoned_live
            got = (end["info"].get(ref0) or {"count": 0})["count"]
            assert got == expect, (
                f"{mode}: dbnum_info_table:{ref0} count 记账不平：终态 {got} != "
                f"起点 {(base['info'].get(ref0) or {}).get('count')} + 新建 {created} "
                f"- 立碑 {tombstoned_live}"
            )
    info_sesno = {
        ref0: (replay["info"].get(ref0, {}).get("sesno"), net["info"].get(ref0, {}).get("sesno"))
        for ref0 in ref0s
    }
    sesno_diffs = {r: v for r, v in info_sesno.items() if v[0] != v[1]}
    assert not sesno_diffs, f"dbnum_info_table sesno 两臂不一致: {sesno_diffs}"

    blind_total = sum(len(v) for k, v in buckets.items() if not k.startswith("§5"))
    print(
        f"[A/B] 终态等价成立：共同活行 {len(both_live)} 严格相等；"
        f"墓碑 sesno 归一 {sum(len(v) for k, v in buckets.items() if k.startswith('§5'))} 条；"
        f"回放盲区归因 {blind_total} 条（净口径持文件真值）；"
        f"原样重写 ref_rev 顺手重建豁免见 [归因] 打印；"
        f"窗口执行 回放 {timings['replay']['window']:.1f}s vs "
        f"净 {timings['net']['window']:.1f}s"
    )


# ── T11b：存量库删除等价直证 ────────────────────────────────────────────────

def _resolve_fixture_exe() -> Path | None:
    """定位 Rust 权威 `db_session_fixture` 可执行档（真实文件切片对拍用）；缺则 None。"""
    target = os.environ.get("CARGO_TARGET_DIR")
    roots = [Path(target)] if target else []
    roots += [REPO_ROOT / "target", REPO_ROOT.parent / "target"]
    exe_name = "db_session_fixture.exe" if os.name == "nt" else "db_session_fixture"
    for root in roots:
        for profile in ("debug", "release"):
            exe = root / profile / exe_name
            if exe.is_file():
                return exe
    return None


def _atomic_install(data: bytes, dst: Path, scratch: Path) -> None:
    """把 `data` 原子落到 `dst`：同卷临时文件 + fsync + `os.replace`。

    `os.replace` 在同卷上是原子的、且覆盖已存在目标（Windows Py3.3+ 亦然）——进程
    被 kill / 抛 PermissionError 时 `dst` 要么是旧内容要么是新内容，绝不留半截截断
    的源库（这正是 agent-3 P1 要堵的洞）。`scratch` 必须与 `dst` 同卷，否则
    `os.replace` 抛 `OSError: [Errno 18] cross-device`。
    """
    fd, tmp = tempfile.mkstemp(dir=str(scratch), prefix="swap-")
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(tmp, dst)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise


def _live_deleted_targets(binding, deleted_refnos: set[str]) -> set[str]:
    """窗口被删 refno 里当前库中确为活行（存在且 deleted=false）的那些。

    空跑主防线：为空即说明存量库没有被删元素的活行，删除必然空跑（恒绿假证据）。
    """
    keys = sorted(r.replace("/", "_") for r in deleted_refnos)
    live: set[str] = set()
    for chunk in _chunks(keys, CHUNK):
        id_list = ", ".join(f"'{k}'" for k in chunk)
        for row in _query(
            binding,
            f"SELECT record::id(id) AS key, deleted FROM pe "
            f"WHERE record::id(id) IN [{id_list}];",
        ):
            if row.get("deleted") is False:
                live.add(str(row["key"]))
    return live


@LIVE
def test_net_and_replay_agree_on_a_stock_deletion(binding, ab_baseline, net_window_env):
    """T11b：窗口起点早于删除会话、存量库内确有活行时的删除等价直证。

    现有全链 A/B 起点是「当前文件基线」（state@latest），窗口内被删元素在基线本就
    无行、两臂删除语句都落空（墓碑归一实测 0）——恒绿不是证据。这里用 `session_cut`
    把库切到 sesno K（K<删除会话）的**存量态**做 baseline，让被删元素在起点**活着**，
    再跑 K+1..latest 让删除真正命中活行。

    判定分工（务必分清，最易做错处）：
    - 被测对象是**纯文件判定**：net 只吃「文件 + 起止 sesno」给 Deleted 集。
    - 删除**独立机制基准**是 core.dll `elementsDeletedBetween` 的键集差（旧根有键、
      新根无键，report §4.4）——`parse.net_changes` 的 deleted 集正是该判据的纯文件
      复刻，作为窗口删除 oracle；**不**用 `search_latest_refno` 点查当独立证明（同源）。
    - DB 查询**只**用于：① 窗口前证被删 refno 在起点是活行（`deleted=false`，空跑
      主防线）；② 窗口后证净口径真把活行立成了墓碑（下游附加断言），**不**作删除判据。
    - 允许 net / replay 删除条目发散（回放有跨会话删除盲区，issue-019）——逐条归因，
      净口径持文件真值。

    红证钩子：`AIOS_T11B_FORCE_EMPTYRUN=1` 时故意用全量文件做基线，重现空跑缺陷
    （被删元素起点无活行）→ 空跑主防线必红。证明本用例不是恒绿。
    """
    file_latest = ab_baseline["file_latest"]
    source_bytes = DB_FILE.read_bytes()
    original_sha = snap.sha256_bytes(source_bytes)

    # 0. Rust 权威对拍（真实文件）：Python 镜像会话链 == db_session_fixture inspect。
    py_latest, py_cuts = snap.session_chain(source_bytes)
    assert py_latest == file_latest, f"Python 链 latest={py_latest} != 文件水位 {file_latest}"
    fixture_exe = _resolve_fixture_exe()
    if fixture_exe is None:
        # 找不到权威切片档默认**硬失败**——不静默 print 跳过（agent-3 P2）。
        assert os.environ.get("AIOS_T11B_ALLOW_NO_RUST_CHECK") == "1", (
            "找不到 db_session_fixture 可执行档——Rust 权威切片对拍无法进行，硬失败。"
            "先 `cargo build --bin db_session_fixture --no-default-features "
            "--features ws,gen_model,manifold,project_hd`；确需降级再显式设 "
            "AIOS_T11B_ALLOW_NO_RUST_CHECK=1（仅合成不变量单测兜底镜像正确性）。"
        )
        print(
            "[T11b][WARN] 未找到 db_session_fixture，且 AIOS_T11B_ALLOW_NO_RUST_CHECK=1 →"
            " 降级：跳过真实文件 Rust 权威链对拍（合成不变量单测仍兜底镜像正确性）"
        )
    else:
        rust_latest, rust_sesnos = snap.rust_inspect_chain(fixture_exe, DB_FILE)
        assert rust_latest == py_latest, (
            f"Rust inspect latest={rust_latest} != Python 镜像 {py_latest}"
        )
        assert sorted(rust_sesnos) == sorted(py_cuts), (
            "Python 切割镜像与 Rust 权威会话链在真实文件上不一致（镜像漂移）"
        )
        print(f"[T11b] Rust inspect 链对拍通过：{len(py_cuts)} 会话，latest={file_latest}")

    # 1. 选切点 K（<删除会话）；文件层净删除集当删除 oracle。
    k = int(os.environ.get("AIOS_T11B_K", 24))
    assert k in py_cuts, (
        f"testbed 会话链缺 sesno={k}；可用 {sorted(py_cuts)[:3]}…{sorted(py_cuts)[-3:]}"
    )
    assert 1 <= k < file_latest, f"K={k} 必须落在 1..{file_latest}"
    oracle_raw = binding.parse.net_changes(str(DB_FILE), k + 1, file_latest)
    oracle: dict[str, str] = {}
    for kind in ("added", "deleted", "modified"):
        for entry in oracle_raw[kind]:
            oracle[entry["refno"]] = kind
    deleted_refnos = {r for r, kind in oracle.items() if kind == "deleted"}
    assert deleted_refnos, (
        f"窗口 {k + 1}..={file_latest} 文件层无净删除——选错 K/窗口，无法证删除"
    )

    # 2. 切 @K 存量快照（Python 镜像）+ Rust inspect 回读确认是合法 @K 文件。
    #    scratch 建在 DB_FILE 同卷（os.replace 原子替换要求同卷）；先留 pristine 备份。
    scratch = Path(tempfile.mkdtemp(prefix="aios-t11b-", dir=str(DB_FILE.parent)))
    pristine = scratch / "pristine"
    pristine.write_bytes(source_bytes)
    assert snap.sha256_file(pristine) == original_sha, "pristine 备份 SHA 必须等于原文件"

    cut_snapshot = snap.cut_bytes(source_bytes, k)
    cut_sha = snap.sha256_bytes(cut_snapshot)
    cut_path = scratch / f"ams8000_at_{k}"
    cut_path.write_bytes(cut_snapshot)
    # @K 文件大小必须恰为 (latest_page+1)*PAGE_SIZE（截断规则的独立复核，便宜项）。
    expected_cut_size = (py_cuts[k][1] + 1) * snap.PAGE_SIZE
    assert len(cut_snapshot) == expected_cut_size, (
        f"@K 快照大小 {len(cut_snapshot)} != (latest_page+1)*PAGE_SIZE={expected_cut_size}"
    )
    cut_latest, cut_sesnos = snap.session_chain(cut_snapshot)
    assert cut_latest == k, f"切出的快照 latest={cut_latest} != K={k}"
    assert set(cut_sesnos) == {s for s in py_cuts if s <= k}, "切片会话集应恰为 ≤K"
    if fixture_exe is not None:
        rc_latest, rc_sesnos = snap.rust_inspect_chain(fixture_exe, cut_path)
        assert rc_latest == k and set(rc_sesnos) == set(cut_sesnos), (
            "Rust 权威不认可 Python 切出的 @K 快照（镜像与权威漂移）"
        )
    print(
        f"[T11b] 切点 K={k}（cut sha={cut_sha[:12]}），窗口 {k + 1}..={file_latest} "
        f"文件净删除 {len(deleted_refnos)} 条"
    )

    ref0s = sorted(set(ab_baseline["ref0s"]) | {int(r.split("_")[0]) for r in oracle})
    force_empty_run = os.environ.get("AIOS_T11B_FORCE_EMPTYRUN") == "1"

    dumps: dict[str, dict] = {}
    base_dumps: dict[str, dict] = {}
    live_targets_seen: set[str] = set()
    try:
        for mode, flag in (("replay", "off"), ("net", "on")):
            # a. 存量态起点：原子换入 @K 文件 → baseline@K（被删元素此刻活着）。
            baseline_source = source_bytes if force_empty_run else cut_snapshot
            _atomic_install(baseline_source, DB_FILE, scratch)
            reset_secs = _reset_arm(
                binding, ref0s, file_latest if force_empty_run else k, None
            )

            live_targets = _live_deleted_targets(binding, deleted_refnos)
            assert live_targets, (
                f"{mode}: 起点库里没有任何窗口被删元素是活行——仍是空跑，T11b 不成立"
                f"（换更早的 K 或含长寿命删除的窗口；force_empty_run={force_empty_run}）"
            )
            live_targets_seen |= live_targets
            base_dumps[mode] = _dump(binding, ref0s)

            # b. 原子换回全量文件（窗口执行才有 K+1..latest 可扫）+ 水位钉 K。
            _atomic_install(source_bytes, DB_FILE, scratch)
            assert snap.sha256_file(DB_FILE) == original_sha, "换回全量后 SHA 必须与原文件一致"
            binding.db.query(
                f"UPDATE dbnum_watermark:{DBNUM} SET applied_sesno = {k}, sesno = {k};"
            )

            # c. 本臂口径 → 执行窗口 → 水位回到 file_latest。
            os.environ[NET_ENV] = flag
            started = time.monotonic()
            outcome = binding.incr.execute_manual(dbnums=[DBNUM])
            window_secs = time.monotonic() - started
            assert not outcome["receipt"]["blocked"], f"{mode}: 窗口执行不应阻断"
            assert outcome["drained"] >= 1, f"{mode}: 窗口批次必须被 worker 消费"
            assert _watermark(binding) == file_latest, f"{mode}: 执行后水位对齐文件"
            dumps[mode] = _dump(binding, ref0s)
            print(
                f"[{mode}] 重置 {reset_secs:.1f}s 窗口 {window_secs:.1f}s "
                f"起点被删活行 {len(live_targets)}"
            )
    finally:
        # 收尾：原子恢复（优先 pristine 文件 os.replace，异常再 in-memory 兜底），
        # 校 SHA，**清理放最后**——顺序错了会在 kill/异常时留下截断源库。
        try:
            if pristine.exists():
                os.replace(pristine, DB_FILE)
            else:
                _atomic_install(source_bytes, DB_FILE, scratch)
        except OSError:
            _atomic_install(source_bytes, DB_FILE, scratch)
        assert snap.sha256_file(DB_FILE) == original_sha, "收尾：testbed 全量文件必须无损还原"
        shutil.rmtree(scratch, ignore_errors=True)
        if _watermark(binding) != file_latest:
            binding.db.query(
                f"UPDATE dbnum_watermark:{DBNUM} SET applied_sesno = {file_latest}, "
                f"sesno = {file_latest};"
            )

    replay, net = dumps["replay"], dumps["net"]

    # 0. 两臂 @K 起点同构。
    for dim in ("rows", "attrs", "owner_edges", "ref_edges", "pending", "info", "watermark"):
        assert base_dumps["replay"][dim] == base_dumps["net"][dim], (
            f"两臂 @K 起点在 {dim} 维不同构——切片不是文件的确定函数？"
        )
    # 1. 水位。
    assert net["watermark"] == replay["watermark"], "两臂水位不一致"

    # 2. pe 全量行严格相等，例外逐条归因（含 T11b 专属「回放漏删存量活行」）；零未归因。
    buckets, unexplained, blind_refnos = _classify_pe_rows(replay, net, oracle)
    for bucket, lines in sorted(buckets.items()):
        print(f"[归因] {bucket}：{len(lines)} 条")
        for line in lines[:10]:
            print(f"    {line}")
    assert not unexplained, (
        "pe 终态存在归因不了的差异（缺陷）：\n  " + "\n  ".join(unexplained[:20])
    )

    # 3. 空跑主防线：净口径必须把「起点活着、窗口内被删」的元素真的立成墓碑。
    net_tombstoned_live = {
        key
        for key in live_targets_seen
        if key in net["rows"]
        and net["rows"][key]["deleted"]
        and key in base_dumps["net"]["rows"]
        and not base_dumps["net"]["rows"][key]["deleted"]
    }
    assert net_tombstoned_live, (
        "净口径没有对任何存量活行立碑——这正是空跑（恒绿假证据）。"
        f"起点活着的被删元素 {sorted(live_targets_seen)[:10]}"
    )
    # 独立性：净立的碑必须落在文件删除 oracle 集内（core.dll 键集差复刻）。
    assert net_tombstoned_live <= deleted_refnos, (
        "净口径立碑的 refno 不在文件删除 oracle 内（越权删除）："
        f"{sorted(net_tombstoned_live - deleted_refnos)[:10]}"
    )

    # 4. 共同活行严格相等（盲区/漏删行至多单侧在场，已在 §2 归因排除；
    #    §5.5 的 last-touch sesno 戳位差已归因，这些活行除 sesno 外逐字段相等，
    #    从严格比对里剔除、另按「除 sesno 外相等」复核）。
    sesno_normalized = {
        line.split(":", 1)[0]
        for line in buckets.get(
            "§5 修改活行 last-touch sesno 戳位（回放=op流 / 净=记录页反查，§5.5）", []
        )
    }
    both_live = (set(replay["live_ids"]) & set(net["live_ids"])) - sesno_normalized
    live_diffs = {
        key: (replay["rows"][key], net["rows"][key])
        for key in both_live
        if replay["rows"][key] != net["rows"][key]
    }
    assert not live_diffs, (
        f"两臂共同活行有 {len(live_diffs)} 条字段不一致（前 5）: {sorted(live_diffs)[:5]}"
    )
    for key in sesno_normalized:  # 归一行：除 sesno 外必须逐字段相等，否则不是纯戳位差
        r, n = replay["rows"][key], net["rows"][key]
        assert {k: v for k, v in r.items() if k != "sesno"} == {
            k: v for k, v in n.items() if k != "sesno"
        }, f"§5.5 归一行 {key} 除 sesno 外仍有差异，不能当纯戳位差归一"

    missed = buckets.get("回放漏删存量活行（净立碑持文件真值）", [])
    print(
        f"[T11b] 存量删除等价成立：净立碑 {len(net_tombstoned_live)} 条（⊆ 文件删除 oracle）；"
        f"回放漏删归因 {len(missed)} 条；共同活行 {len(both_live)} 严格相等。"
        f"删除判定 oracle=core.dll elementsDeletedBetween 键集差（report §4.4），"
        f"DB 查询仅证起点活行/终态、不作删除判据。"
    )
