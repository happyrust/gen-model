# -*- coding: utf-8 -*-
"""净窗口单一口径的 live 全链回归（ADR-031；原 ADR-022 双臂 A/B 已退役）。

窗口执行走与服务完全相同的机器（`incr.execute_manual`：扫描 + 入队 + worker
冻结 + ADR-017 暂存窗口 + 窗口内模型生成 + 提交 + 水位收口），在 conftest 自起
的一次性内存 SurrealDB @8071 上对已跟踪 issue-019 快照跑固定窗口 25..=26。

序列：

a. 校验 issue-019 ZIP、manifest 以及 baseline@24 / final@26 的字节数和 SHA；
b. 原子换入 baseline@24，走生产基线入口，并断言固定 EQUI/BOX 都是活行；
c. 原子换入 final@26，预览固定三态 `ZONE modified + EQUI/BOX deleted`，再执行
   扫描→入队→worker→暂存→写回→水位；
d. 终态严格断言 changed=3、sessions=[25,26]、两目标恰好立碑、ZONE 仍活、
   活行恰减 2 且没有额外 PE 行；最后原子恢复原 db8000 并复核 SHA。

执行层双臂 A/B（`test_net_and_replay_full_executions_land_equivalent_states`）
已退役为历史证据——单路径下切 `AIOS_NET_WINDOW` 不再换臂。2026-08-13 两轮全绿
见 `docs/evidence/2026-08-13-session-index-diff-net-changes.md` 与 live 台账。
跨结构交叉验证仍由性质 h/i 与两条 live 对拍承担（直接调两个收集器）。

**opt-in**：设 `AIOS_NET_AB=1` 才跑（一次 8000 基线重建 + 窗口内真实模型
生成，全程分钟级）。跑法：

    cd python
    $env:AIOS_NET_AB = '1'
    $env:PYTHONUNBUFFERED = '1'
    $env:RUST_MIN_STACK = '16777216'
    .venv\\Scripts\\python.exe -m pytest tests/test_net_window_ab.py -q -s --tb=short

**红证**：`AIOS_T11B_FORCE_EMPTYRUN=1` 故意以 final@26 建 baseline，fixture 必须在
执行前的固定删除目标活行断言处变红。**并发禁忌**：本文件会原子替换固定 DB_FILE，
禁止 pytest-xdist 并行；切换全走同卷 `os.replace`。
"""

from __future__ import annotations

import json
import os
import shutil
import struct
import tempfile
import time
import zipfile
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
DB_FILE = Path(
    os.environ.get(
        "AIOS_NET_AB_DB_FILE",
        Path(__file__).resolve().parents[1]
        / "testbed" / "projects" / "AvevaMarineSample" / "ams000" / "ams8000_0001",
    )
).resolve()
ISSUE019_ROOT = (
    REPO_ROOT
    / "tests"
    / "fixtures"
    / "issues"
    / "issue-019-cross-session-parent-child-delete"
)
ISSUE019_REFS = {
    "zone": "24384_24775",
    "parent_equi": "24384_24778",
    "child": "24384_24779",
}
ISSUE019_NET = {
    "added": set(),
    "modified": {ISSUE019_REFS["zone"]},
    "deleted": {ISSUE019_REFS["parent_equi"], ISSUE019_REFS["child"]},
}
# AvevaMarineSample 的 SYST 元库（amssys）。引导只需它撑起 MDB /ALL，
# 不能无过滤 execute_manual()——那会把 CATA（5052/6890/7351…）全部入队，
# 之后每条 execute_manual 的 drain_queue_until_empty 都把剩余目录再消化一遍。
SYS_FILE = DB_FILE.parent / "amssys"
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


def _load_issue019_fixture() -> dict:
    """读取并逐字节核验 issue-019 归档；manifest 是本轮唯一独立真值。"""
    manifest_path = ISSUE019_ROOT / "manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert manifest["format"] == "aios-issue-fixture-v1"
    assert manifest["dbnum"] == DBNUM
    assert manifest["window"] == {
        "baseline_sesno": 24,
        "start_sesno": 25,
        "end_sesno": 26,
    }
    assert manifest["refs"] == ISSUE019_REFS

    archive_spec = manifest["archive"]
    archive = ISSUE019_ROOT / archive_spec["path"]
    assert archive.stat().st_size == archive_spec["bytes"]
    assert snap.sha256_file(archive) == archive_spec["sha256"]

    snapshots = {row["role"]: row for row in manifest["snapshots"]}
    assert set(snapshots) == {"baseline", "child_deleted", "parent_deleted"}
    payloads: dict[str, bytes] = {}
    with zipfile.ZipFile(archive) as packed:
        assert set(packed.namelist()) == {row["path"] for row in snapshots.values()}
        for role, spec in snapshots.items():
            data = packed.read(spec["path"])
            assert len(data) == spec["bytes"], f"{role} 字节数漂移"
            assert snap.sha256_bytes(data) == spec["sha256"], f"{role} SHA 漂移"
            payloads[role] = data

    baseline_presence = {
        row["refno"]: (row["noun"], row["present"])
        for row in snapshots["baseline"]["elements"]
    }
    final_presence = {
        row["refno"]: (row["noun"], row["present"])
        for row in snapshots["parent_deleted"]["elements"]
    }
    assert baseline_presence == {
        ISSUE019_REFS["zone"]: ("ZONE", True),
        ISSUE019_REFS["parent_equi"]: ("EQUI", True),
        ISSUE019_REFS["child"]: ("BOX", True),
    }
    assert final_presence == {
        ISSUE019_REFS["zone"]: ("ZONE", True),
        ISSUE019_REFS["parent_equi"]: ("EQUI", False),
        ISSUE019_REFS["child"]: ("BOX", False),
    }
    return {"manifest": manifest, "snapshots": snapshots, "payloads": payloads}


@pytest.mark.offline
def test_issue019_archive_is_the_fixed_net_window_truth():
    fixture = _load_issue019_fixture()
    assert fixture["snapshots"]["baseline"]["sesno"] == 24
    assert fixture["snapshots"]["parent_deleted"]["sesno"] == 26
    assert ISSUE019_NET == {
        "added": set(),
        "modified": {"24384_24775"},
        "deleted": {"24384_24778", "24384_24779"},
    }


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

    清积压的理由：基线会把全库 2,229 个交付根登记进 `model_update_pending`，
    而下一个数据批次会把 pending 积压并进自己的模型计划一起生成（2026-08-13
    run4 实测：窗口批花 22 分钟把 2,229 根全量重生成一遍）——那是基线补课，
    不是窗口回归的被测面；窗口自己登记/结算的计划行照常进签名。
    """
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
            f"FROM pe:`{ref0}_0`..`{ref0}_{RANGE_END}`;",
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
        assert noun.replace("_", "").isalnum(), f"noun 表名不合法，拒绝拼语句: {noun}"
        for chunk in _chunks(sorted(noun_ids), CHUNK):
            # SELECT * FROM table:id, table:id（SurrealQL 按记录 id 寻址）。
            things = ", ".join(f"{noun}:`{i}`" for i in chunk)
            for row in _query(binding, f"SELECT * FROM {things};"):
                # pythonize 出来的 id 是 "BOX:24384_26184" 全称；键只留 noun:refno，
                # 归因豁免按 refno 尾段截取才对得上盲区集合。
                thing = str(row.pop("id", None)).rsplit(":", 1)[-1]
                attrs[f"{noun}:{thing}"] = _normalize(row)

    # 属主边含边 id（复合 id [pe:{owner}, 序号]——成员列表的顺序是语义）。
    # 从本库 Ref0 区间走图，不要 `FROM pe_owner WHERE out IN […]`：那是对
    # pe_owner 全表扫描（引导后可超百万边）。
    owner_edges: set[tuple] = set()
    live_pe = {f"pe:{i}" for i in live_ids}
    for ref0 in ref0s:
        for row in _query(
            binding,
            f"SELECT <string>in AS child, <string>id AS edge, <string>out AS parent "
            f"FROM array::flatten(SELECT VALUE <-pe_owner FROM "
            f"pe:`{ref0}_0`..`{ref0}_{RANGE_END}`);",
        ):
            if row.get("parent") in live_pe:
                owner_edges.add((row["parent"], row["edge"], row["child"]))

    ref_edges: set[tuple] = set()
    for ref0 in ref0s:
        for row in _query(
            binding,
            f"SELECT <string>in AS src, <string>out AS dst "
            f"FROM array::flatten(SELECT VALUE ->ref_rev FROM "
            f"pe:`{ref0}_0`..`{ref0}_{RANGE_END}`);",
        ):
            if row.get("src") in live_pe:
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
    info_from = ", ".join(f"dbnum_info_table:{r}" for r in ref0s)
    info = {
        int(row["ref0"]): {"count": row.get("count"), "sesno": row.get("sesno")}
        for row in _query(
            binding,
            f"SELECT record::id(id) AS ref0, count, sesno FROM {info_from};",
        )
    } if ref0s else {}

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


def _dbnum_ref0s(binding) -> list[int]:
    """当前 dbnum 已落库行的 Ref0 段；固定夹具的 24384 永远并入，避免空库漏清。"""
    rows = _query(
        binding,
        f"SELECT string::split(<string>id, '_')[0] AS prefix, count() AS count "
        f"FROM pe WHERE dbnum = {DBNUM} GROUP BY prefix;",
    )
    return sorted(
        {24384}
        | {
            int(str(row["prefix"]).removeprefix("pe:"))
            for row in rows
        }
    )


def _atomic_install(data: bytes, dst: Path, scratch: Path) -> None:
    """同卷临时文件 + fsync + os.replace，目标始终是完整旧版或完整新版。"""
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
    """固定删除目标中，当前库里确为活行的精确集合。"""
    keys = sorted(refno.replace("/", "_") for refno in deleted_refnos)
    live: set[str] = set()
    for chunk in _chunks(keys, CHUNK):
        things = ", ".join(f"pe:`{key}`" for key in chunk)
        for row in _query(
            binding,
            f"SELECT record::id(id) AS key, deleted FROM {things};",
        ):
            if row.get("deleted") is False:
                live.add(str(row["key"]))
    return live


@pytest.fixture(scope="module")
def issue019_execution(binding):
    """在已跟踪 issue-019 24→26 快照上只执行一次生产全链，并立即恢复原文件/数据库。"""
    fixture = _load_issue019_fixture()
    manifest = fixture["manifest"]
    payloads = fixture["payloads"]
    window = manifest["window"]
    deleted_refnos = set(ISSUE019_NET["deleted"])
    zone_refno = next(iter(ISSUE019_NET["modified"]))

    assert DB_FILE.is_file(), f"testbed 项目副本缺 {DB_FILE}"
    original_bytes = DB_FILE.read_bytes()
    original_sha = snap.sha256_bytes(original_bytes)
    original_latest = int(binding.parse.header(str(DB_FILE))["latest_sesno"])
    scratch = Path(tempfile.mkdtemp(prefix="aios-issue019-", dir=str(DB_FILE.parent)))
    pristine = scratch / "pristine"
    pristine.write_bytes(original_bytes)
    assert snap.sha256_file(pristine) == original_sha

    result = None
    try:
        assert SYS_FILE.is_file(), f"testbed 缺 SYST 元库 {SYS_FILE}"
        sys_dbnum = int(binding.parse.header(str(SYS_FILE))["dbnum"])
        sys_receipt = binding.incr.execute_manual(dbnums=[sys_dbnum])["receipt"]
        assert not sys_receipt.get("blocked"), f"SYS 引导不应阻断: {sys_receipt.get('blocked')}"

        ref0s = _dbnum_ref0s(binding)
        force_empty_run = os.environ.get("AIOS_T11B_FORCE_EMPTYRUN") == "1"
        baseline_role = "parent_deleted" if force_empty_run else "baseline"
        baseline_sesno = 26 if force_empty_run else window["baseline_sesno"]
        _atomic_install(payloads[baseline_role], DB_FILE, scratch)
        assert int(binding.parse.header(str(DB_FILE))["latest_sesno"]) == baseline_sesno

        reset_secs = _reset_arm(binding, ref0s, baseline_sesno, None)
        base = _dump(binding, ref0s)
        live_targets = _live_deleted_targets(binding, deleted_refnos)
        assert live_targets == deleted_refnos, (
            "固定删除目标在起点不是活行："
            f"期望 {sorted(deleted_refnos)}，实际 {sorted(live_targets)}；"
            f"force_empty_run={force_empty_run}"
        )
        assert zone_refno in base["live_ids"], "固定 ZONE 在 baseline@24 必须是活行"
        baseline_live_count = len(base["live_ids"])

        _atomic_install(payloads["parent_deleted"], DB_FILE, scratch)
        final_spec = fixture["snapshots"]["parent_deleted"]
        assert snap.sha256_file(DB_FILE) == final_spec["sha256"]
        binding.db.query(
            f"UPDATE dbnum_watermark:{DBNUM} SET "
            f"applied_sesno = {window['baseline_sesno']}, "
            f"sesno = {window['baseline_sesno']};"
        )

        preview = binding.db.preview_manual_update()
        db_preview = next(
            row for row in preview["dbnums"] if int(row["dbnum"]) == DBNUM
        )
        preview_sesnos = [int(row["sesno"]) for row in db_preview["sessions"]]
        net_counts = {
            "added": int(db_preview["net_added"]),
            "modified": int(db_preview["net_modified"]),
            "deleted": int(db_preview["net_deleted"]),
        }
        assert preview_sesnos == [25, 26], f"冻结会话页清单漂移: {db_preview['sessions']}"
        assert net_counts == {"added": 0, "modified": 1, "deleted": 2}
        assert preview["warnings"], "preview 必须自报 ADR-031 净口径"
        first_warning = preview["warnings"][0]
        assert "ADR-031" in first_warning and "净窗口" in first_warning, first_warning

        started = time.monotonic()
        outcome = binding.incr.execute_manual(dbnums=[DBNUM])
        window_secs = time.monotonic() - started
        receipt = outcome["receipt"]
        assert not receipt["blocked"], f"窗口执行不应阻断: {receipt['blocked']}"
        assert outcome["drained"] >= 1, "窗口批次必须被 worker 消费"
        queued = [
            row
            for row in receipt["enqueued"] + receipt["merged"]
            if int(row["dbnum"]) == DBNUM
        ]
        assert len(queued) == 1, f"8000 应恰有一个生产队列行: {queued}"
        assert [int(queued[0]["start_sesno"]), int(queued[0]["end_sesno"])] == [25, 26]
        assert _watermark(binding) == 26

        end = _dump(binding, ref0s)
        end_live_count = len(end["live_ids"])
        tombstoned = {
            key
            for key, row in end["rows"].items()
            if row["deleted"]
            and key in base["rows"]
            and not base["rows"][key]["deleted"]
        }
        result = {
            "base": base,
            "end": end,
            "baseline_live_count": baseline_live_count,
            "end_live_count": end_live_count,
            "tombstoned": tombstoned,
            "live_targets": live_targets,
            "net_counts": net_counts,
            "merged_sesnos": preview_sesnos,
            "changed_elements": sum(net_counts.values()),
            "drained": int(outcome["drained"]),
            "batch_window": [int(queued[0]["start_sesno"]), int(queued[0]["end_sesno"])],
            "window_secs": window_secs,
            "reset_secs": reset_secs,
            "original_sha": original_sha,
            "final_sha": final_spec["sha256"],
        }
    finally:
        try:
            if pristine.exists():
                os.replace(pristine, DB_FILE)
            else:
                _atomic_install(original_bytes, DB_FILE, scratch)
        except OSError:
            _atomic_install(original_bytes, DB_FILE, scratch)
        assert snap.sha256_file(DB_FILE) == original_sha, "收尾：db8000 原文件 SHA 必须无损还原"

        restore_ref0s = _dbnum_ref0s(binding)
        _reset_arm(binding, restore_ref0s, original_latest, None)
        assert _watermark(binding) == original_latest
        shutil.rmtree(scratch, ignore_errors=True)

    assert result is not None
    return result


@LIVE
@pytest.mark.skip(
    reason=(
        "ADR-031 历史证据：单路径后无法保留执行层双臂。"
        "固定 issue-019 全链回归由 test_net_window_full_execution_lands_a_stable_signature 承担。"
    )
)
def test_net_and_replay_full_executions_land_equivalent_states():
    """退役：原双臂全链 A/B。保留名字以免外部脚本误以为用例失踪。"""


@LIVE
def test_net_window_full_execution_lands_a_stable_signature(issue019_execution):
    """issue-019 固定真值：扫描→入队→worker→暂存→写回→水位的严格终态签名。"""
    result = issue019_execution
    base = result["base"]
    end = result["end"]
    deleted_refnos = set(ISSUE019_NET["deleted"])
    zone_refno = next(iter(ISSUE019_NET["modified"]))

    assert result["drained"] >= 1
    assert result["changed_elements"] == 3
    assert result["merged_sesnos"] == [25, 26]
    assert result["batch_window"] == [25, 26]
    assert result["net_counts"] == {"added": 0, "modified": 1, "deleted": 2}
    assert end["watermark"] == _normalize(
        [{"applied_sesno": 26, "sesno": 26, "file_latest_sesno": 26}]
    )
    assert result["tombstoned"] == deleted_refnos
    assert zone_refno in end["live_ids"]
    assert end["rows"][zone_refno]["noun"] == "ZONE"
    assert not end["rows"][zone_refno]["deleted"]

    assert set(end["rows"]) == set(base["rows"]), "25..26 窗口不得创建或遗失额外 PE 行"
    assert set(base["live_ids"]) - set(end["live_ids"]) == deleted_refnos
    assert len(end["live_ids"]) == len(base["live_ids"]) - 2
    assert result["end_live_count"] == result["baseline_live_count"] - 2

    print(
        "[net] issue-019 固定签名通过：changed=3 sessions=[25,26] "
        f"tombstones={sorted(result['tombstoned'])} "
        f"watermark=26 original_sha={result['original_sha'][:12]} "
        f"window={result['window_secs']:.1f}s"
    )


@LIVE
@pytest.mark.skip(
    reason=(
        "ADR-031：原双臂 T11b 退役；"
        "固定 issue-019 单臂直证由 test_net_window_agrees_on_a_stock_deletion 承担。"
    )
)
def test_net_and_replay_agree_on_a_stock_deletion():
    """退役别名：原双臂 T11b。"""


@LIVE
def test_net_window_agrees_on_a_stock_deletion(issue019_execution):
    """T11b：两个 manifest 固定删除目标在 baseline@24 活着，执行后恰好都立碑。"""
    result = issue019_execution
    deleted_refnos = set(ISSUE019_NET["deleted"])
    assert result["live_targets"] == deleted_refnos
    assert result["tombstoned"] == deleted_refnos
    for refno in deleted_refnos:
        assert refno in result["base"]["rows"]
        assert not result["base"]["rows"][refno]["deleted"]
        assert result["end"]["rows"][refno]["deleted"]
    print(
        f"[T11b] 固定删除直证通过：起点活行/终态立碑={sorted(deleted_refnos)}；"
        f"final_sha={result['final_sha'][:12]}"
    )
