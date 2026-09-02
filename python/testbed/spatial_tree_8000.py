# -*- coding: utf-8 -*-
"""ams8000 空间树启动初始化 + 增量更新回放测试驱动。

测什么（对应方案 c:/Users/dpc/.cursor/plans/ams8000_空间树增量测试_472cdf86.plan.md）：

1. **启动初始化裁决矩阵**：同一个 SurrealDB 实例上反复「重启」（full_init 只能
   每进程一次，所以每次重启 = 一个新 Python 子进程），分别制造五种现场——
   快照新鲜（reused）、快照缺失（rebuilt）、库侧 epoch 漂移（rebuilt）、
   携带待重放空间意图（replayed）、快照字节损坏（rebuilt）——断言
   `spatial.tree_status()` 的 startup_verdict / state / entries / pending。
2. **增量窗口回放**：以 issue-019 夹具的 db8000 sesno-24 快照为基线建库，然后
   拿真实 ams8000 文件逐会话 `incr.apply_file(end=k)` 回放，每窗断言水位、
   树条目 == 库内可用指针数、epoch 单调；sesno 25/26 是已知的两次真实删除
   （BOX 24384_24779 / EQUI 24384_24778），据此验证删除→摘树→epoch 留痕。
   **每窗之后再重启一次**，断言中途重启的启动初始化按 reused 复用快照。

隔离与残留纪律：
- SurrealDB 用 bin/surreal.exe（fork 2.1.4）一次性内存实例 @8072，驱动整轮
  存活（跨 worker 子进程，「重启」重的是进程不是库），退出即全部丢弃；
  8009 / 8019 / 8071 / 9099 一概不碰。
- 驱动会把 testbed 项目副本里的 ams000/ams8000_0001 临时换成 sesno-24 基线
  快照（先备份，finally 逐字节还原）；full_init 拿的也是 testbed 副本的单实例
  锁——**测试期间勿并行跑 run_full_loop.py 或 pytest 房间增量档**。
- 仓库根的空间树快照产物（accel_tree_AvevaMarineSample.snapshot 及遗留
  .bin/.meta.json）开跑前挪走、结束后删测试产物并挪回，谁的都不毁。
- 上次异常退出留下的备份/挪位文件，下次启动先自动还原。

用法（在 python/ 目录；主 crate 变过要先 maturin develop 重建 pyd）：
    .venv\\Scripts\\python.exe testbed\\spatial_tree_8000.py
    .venv\\Scripts\\python.exe testbed\\spatial_tree_8000.py --max-windows 4 --gen-roots 2
    .venv\\Scripts\\python.exe testbed\\spatial_tree_8000.py --skip-windows   # 只跑启动矩阵
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import socket
import subprocess
import sys
import time
import zipfile
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

TESTBED = Path(__file__).resolve().parent
REPO_ROOT = TESTBED.parents[1]
RUN_DIR = Path(os.environ.get("SPATIAL8000_RUN_DIR", REPO_ROOT))
CONFIG = Path(os.environ.get("SPATIAL8000_CONFIG", TESTBED / "DbOption-spatial8000"))
# 双库对拍的 B 侧（对照库）：第二个一次性内存实例，直接在 final-26 上建基线。
ORACLE_CONFIG = TESTBED / "DbOption-spatial8000-oracle"
ORACLE_PORT = 8073
PROJECT_DB = Path(
    os.environ.get(
        "SPATIAL8000_PROJECT_DB",
        TESTBED / "projects" / "AvevaMarineSample" / "ams000" / "ams8000_0001",
    )
)
DB_BACKUP = PROJECT_DB.with_name(PROJECT_DB.name + ".spatial8000-backup")
FIXTURE = (
    REPO_ROOT / "tests" / "fixtures" / "issues" / "issue-019-cross-session-parent-child-delete"
)
WORK = TESTBED / ".spatial8000"
EXTRACTED = WORK / "extracted"
LOGS = WORK / "logs"
SURREAL_EXE = REPO_ROOT / "bin" / "surreal.exe"
SURREAL_PORT = 8072
SURREAL_NS, SURREAL_DB = "1516", "AvevaMarineSample"

# issue-019 manifest 的已知角色（db8000 真实历史）：
# sesno 25 删 BOX（child），sesno 26 删 EQUI（parent）。
EQUI = "24384_24778"
BOX_CHILD = "24384_24779"
BASELINE_SESNO = 24

# full_init / persist 写在仓库根（cwd）的空间树快照产物：V2 单文件 + 遗留两件。
TREE_ARTIFACTS = [
    RUN_DIR / "accel_tree_AvevaMarineSample.snapshot",
    RUN_DIR / "accel_tree_AvevaMarineSample.bin",
    RUN_DIR / "accel_tree_AvevaMarineSample.meta.json",
]
SHELVE_SUFFIX = ".bak-spatial8000"
RESULT_PREFIX = "@@SPATIAL8000-RESULT "


# ── 通用小件 ─────────────────────────────────────────────────────────────────


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def publish_session_snapshot(source: Path, sesno: int, target: Path) -> None:
    """Publish the append-only PDMS file exactly as it looked at ``sesno``.

    Page zero points at the latest session page.  Each session page stores its
    predecessor, sesno and final page; following that chain and rewriting the
    header pointer is the same cut used by ``db_session_fixture``.
    """
    page_size = 0x800
    header_session_page_offset = 40
    data = source.read_bytes()
    if len(data) < page_size or len(data) % page_size:
        raise RuntimeError(f"PDMS file is not page aligned: {source} ({len(data)} bytes)")

    def be_u32(offset: int) -> int:
        return int.from_bytes(data[offset:offset + 4], "big")

    page = be_u32(header_session_page_offset)
    seen: set[int] = set()
    while page not in (0, 0xFFFFFFFF) and page not in seen:
        seen.add(page)
        start = page * page_size
        if start + page_size > len(data):
            raise RuntimeError(f"session page {page} is outside {source}")
        current = be_u32(start + 12)
        if current == sesno:
            latest_page = be_u32(start + 20)
            end = (latest_page + 1) * page_size
            if end > len(data):
                raise RuntimeError(f"snapshot end {end} is outside {source}")
            snapshot = bytearray(data[:end])
            snapshot[header_session_page_offset:header_session_page_offset + 4] = (
                page.to_bytes(4, "big")
            )
            target.write_bytes(snapshot)
            return
        page = be_u32(start + 4)
    raise RuntimeError(f"session {sesno} is absent from {source}")


def port_in_use(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.settimeout(0.5)
        return probe.connect_ex(("127.0.0.1", port)) == 0


def wait_port(port: int, timeout: float = 30.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if port_in_use(port):
            return True
        time.sleep(0.2)
    return False


def surql(sql: str) -> str:
    """经 surreal CLI 对 8072 执行 SQL（驱动侧的库状态篡改用，不经绑定）。"""
    proc = subprocess.run(
        [
            str(SURREAL_EXE), "sql",
            "--endpoint", f"http://127.0.0.1:{SURREAL_PORT}",
            "--user", "root", "--pass", "root",
            "--ns", SURREAL_NS, "--db", SURREAL_DB, "--json",
        ],
        input=sql,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=30,
    )
    if proc.returncode:
        raise RuntimeError(f"surreal sql 失败（exit {proc.returncode}）: {proc.stderr[-400:]}")
    return proc.stdout


# ── worker 侧：每个阶段一个新进程（full_init 每进程只能一次） ────────────────


class Checks:
    def __init__(self) -> None:
        self.items: list[dict] = []

    def add(self, name: str, ok: bool, detail: str = "") -> bool:
        self.items.append({"name": name, "ok": bool(ok), "detail": str(detail)[:300]})
        return bool(ok)

    def result(self, **extra) -> dict:
        return {"ok": all(item["ok"] for item in self.items), "checks": self.items, **extra}


def _init_full(params: dict):
    import aios_db

    # 双库对拍：A 侧用主配置（8072），B 侧 worker 由驱动传 oracle 配置（8073）。
    aios_db.set_config(params.get("config") or str(CONFIG))
    # force=True：互踩探测对老部署包（无 sul_db.endpoint 的 /health）走保守拒绝，
    # 与 pytest 房间档的 conftest 同一处置；本驱动连的是自己的一次性实例。
    aios_db.full_init(cwd=str(RUN_DIR), force=True)
    return aios_db


def _epoch(aios_db) -> int:
    rows = aios_db.db.query("SELECT `value` FROM spatial_epoch:current;")[0]
    return int(rows[0]["value"]) if rows else 0


def _inst_rows_of(aios_db, refno: str) -> int:
    # 按记录 id 直接寻址：`inst_relate` 的 id 就是 refno，而这张表没有 `(in, out)`
    # 索引，`WHERE in = pe:{refno}` 在真库上是整表扫。
    rows = aios_db.db.query(
        f"SELECT count() FROM inst_relate:{refno} GROUP ALL;"
    )[0]
    return int(rows[0]["count"]) if rows else 0


def _pe_deleted(aios_db, refno: str):
    rows = aios_db.db.query(f"SELECT deleted FROM pe:{refno};")[0]
    return rows[0]["deleted"] if rows else None


def _tree_summary(aios_db) -> dict:
    status = aios_db.spatial.tree_status()
    return {
        "verdict": status.get("startup_verdict"),
        "state": status.get("state"),
        "entries": status.get("entries"),
        "pending": status.get("pending"),
    }


def _round_box(mins, maxs) -> tuple:
    # 树条目与指针值同源于同一批 f32，f64 表示应逐位相等；取 3 位小数只为吸收
    # 序列化通道的表示差异，不为掩盖真实偏差。
    return tuple(round(float(v), 3) for v in list(mins) + list(maxs))


def _pointer_rows(aios_db) -> list[dict]:
    """库内可用包围盒指针：refno / 内容寻址哈希 / 值。

    谓词与重建/覆盖率的 current-only 口径同源（room_model::usable_aabb_pointer_count）：
    排除版本化数组 id 行与软删元素行。在本驱动自建的一次性库上与朴素谓词目前等价
    （无 fn::backup_data 遗产行、软删行在断言前已被 DeleteCleanup 清掉），但
    「树内容 == 指针值」的裁决必须与主库口径同源——口径分叉留着，哪天拿这套
    检查对存量库跑就会误报。
    """
    return aios_db.db.query(
        "SELECT record::id(in) AS refno, record::id(aabb) AS hash, aabb.d AS box "
        "FROM inst_relate WHERE !type::is::array(record::id(id)) "
        "AND in.deleted != true AND world_trans.d != none AND aabb.d != none;"
    )[0]


def _pointer_hash_set(rows: list[dict]) -> list:
    return sorted(f"{row['refno']}#{row['hash']}" for row in rows)


def _tree_value_set(aios_db) -> set[tuple]:
    return {
        (entry["refno"], _round_box(entry["mins"], entry["maxs"]))
        for entry in aios_db.spatial.tree_dump()
    }


def _check_tree_matches_pointers(checks: Checks, aios_db) -> list[dict]:
    """值级不变量：内存树内容 == 已提交指针值，双向逐条。返回指针行供 dump 复用。"""
    rows = _pointer_rows(aios_db)
    tree_set = _tree_value_set(aios_db)
    pointer_set = {
        (row["refno"], _round_box(row["box"]["mins"], row["box"]["maxs"])) for row in rows
    }
    only_tree = sorted(tree_set - pointer_set)[:5]
    only_pointer = sorted(pointer_set - tree_set)[:5]
    checks.add(
        "树内容 == 指针值（逐条双向）",
        not only_tree and not only_pointer,
        f"tree={len(tree_set)} pointers={len(pointer_set)} "
        f"树独有={only_tree} 指针独有={only_pointer}",
    )
    return rows


def worker_probe(params: dict) -> dict:
    """离线血统探针：不连库，纯 parse 层。顺带给待回放窗口记 op 统计。"""
    import aios_db

    aios_db.set_config(params.get("config") or str(CONFIG))
    source = params["file"]
    header = aios_db.parse.header(source)
    sessions = sorted(int(page["sesno"]) for page in aios_db.parse.sessions(source))
    lineage_ok = all(sesno in sessions for sesno in (24, 25, 26))
    deletes_ok = False
    if lineage_ok:
        changes = aios_db.parse.collect_changes(source, 25, 26)
        ses25 = changes.get("25", []) or changes.get(25, [])
        ses26 = changes.get("26", []) or changes.get(26, [])
        deletes_ok = any(
            op.get("refno") == BOX_CHILD and op.get("op") == "deleted" for op in ses25
        ) and any(op.get("refno") == EQUI and op.get("op") == "deleted" for op in ses26)

    # 待回放窗口的 op 统计（add/modified/deleted/none 各几条），让 E3D 追加的
    # 会话在报告里不再是黑盒。
    window_ops: dict[str, dict] = {}
    stat_upto = int(params.get("stat_upto") or 0)
    if lineage_ok and stat_upto > BASELINE_SESNO:
        upto = min(stat_upto, max(sessions))
        changes = aios_db.parse.collect_changes(source, BASELINE_SESNO + 1, upto)
        for sesno, ops in changes.items():
            counts: dict[str, int] = {}
            for op in ops:
                kind = op.get("op", "?")
                counts[kind] = counts.get(kind, 0) + 1
            window_ops[str(sesno)] = counts
    return {
        "ok": True,
        "latest_sesno": header.get("latest_sesno"),
        "dbnum": header.get("dbnum"),
        "sessions": sessions,
        "lineage_ok": bool(lineage_ok and deletes_ok),
        "window_ops": window_ops,
    }


def worker_prepare(params: dict) -> dict:
    """基线建库 → 生成 → 出清积压 → 落快照 → 值级校验。

    A 侧（默认）：baseline@24，显式 ensure 目标 EQUI + 抽样根，断言 BOX 有实例。
    B 侧（oracle）：baseline@26（final-26 文件），EQUI/BOX 已删不 ensure，
    生成全靠 drain_data 出清基线登记的全部单元；dump=True 带回指针哈希集与树内容。
    """
    checks = Checks()
    aios_db = _init_full(params)
    expect_watermark = int(params.get("expect_watermark") or BASELINE_SESNO)

    baseline = aios_db.sync.baseline(8000, "AvevaMarineSample")
    watermark = aios_db.db.watermark(8000)
    checks.add(f"baseline 水位 = {expect_watermark}", watermark == expect_watermark,
               f"watermark={watermark}, report={json.dumps(baseline, default=str)[:160]}")

    # 聚焦增量管道时，先清掉 baseline 刚登记的全库生成积压。
    # 必须位于 ensure 之前，否则会把目标根新产生的 post_regen_aabb
    # 一并删掉，导致“几何指针已更新、空间树未更新”。
    if params.get("skip_baseline_backlog"):
        pending_before = aios_db.db.query(
            "RETURN count(SELECT * FROM model_update_pending WHERE dbnum = 8000);"
        )[0]
        aios_db.db.query("DELETE model_update_pending WHERE dbnum = 8000;")
        pending_after = aios_db.db.query(
            "RETURN count(SELECT * FROM model_update_pending WHERE dbnum = 8000);"
        )[0]
        checks.add("聚焦模式清除基线全库积压", pending_after == 0,
                   f"before={pending_before}, after={pending_after}")

    generated: list[str] = []
    if params.get("ensure_equi", True):
        for refno, label in ((EQUI, "EQUI"), (BOX_CHILD, "BOX child")):
            row = aios_db.db.pe(refno)
            checks.add(f"{label} pe:{refno} 在位且未删",
                       bool(row) and not row.get("deleted"),
                       json.dumps(row, default=str)[:160])

        gen_failures = []
        targets = [EQUI]
        gen_roots = int(params.get("gen_roots") or 0)
        extra = aios_db.db.query(
            "SELECT record::id(id) AS refno FROM pe "
            f"WHERE dbnum = 8000 AND noun = 'EQUI' AND deleted = false LIMIT {gen_roots + 1};"
        )[0]
        for row in extra:
            refno = str(row["refno"])
            if refno != EQUI and len(targets) < gen_roots + 1:
                targets.append(refno)
        for refno in targets:
            try:
                aios_db.model.ensure(refno, force=(refno == EQUI))
                generated.append(refno)
            except Exception as error:  # noqa: BLE001 —— 个别根缺 CATA 允许跳过，但要留痕
                gen_failures.append(f"{refno}: {error}")
        checks.add("样本模型生成（至少含目标 EQUI）", EQUI in generated,
                   f"generated={generated}, failures={gen_failures}")

        box_rows = _inst_rows_of(aios_db, BOX_CHILD)
        checks.add("BOX child 生成后有几何实例", box_rows > 0, f"inst_rows={box_rows}")
    elif params.get("ensure_refnos"):
        # B 侧（oracle）：与 A 侧**逐根相同**的生成口径——驱动把 A 实际生成的样本
        # 清单传进来（已剔除 26 上不存在的目标 EQUI）。基线积压有两千多个根，
        # 全量出清不是本测试的开销预算；哈希对拍只要求两侧生成范围一致。
        gen_failures = []
        for refno in params["ensure_refnos"]:
            try:
                aios_db.model.ensure(refno)
                generated.append(refno)
            except Exception as error:  # noqa: BLE001
                gen_failures.append(f"{refno}: {error}")
        checks.add("对照库按 A 侧清单生成", not gen_failures,
                   f"generated={generated}, failures={gen_failures}")

    # 正常模式出清全库基线积压；聚焦模式只会出清上面 ensure
    # 重新产生的目标 pending。后续窗口也仍走同一 drain_data 入口。
    backlog = aios_db.incr.drain_data()
    checks.add("目标模型积压出清" if params.get("skip_baseline_backlog") else "基线模型积压出清",
               True, f"drain_data={backlog}")

    # 按需 ensure 是直写几何路径，不会为已被清除的 baseline regen
    # 补造窗口级 spatial_reconcile 意图。聚焦夹具在基线结束时从
    # 已提交指针重建一次树；被测的 25/26 窗口仍走真实增量收敛。
    if params.get("skip_baseline_backlog"):
        aios_db.spatial.rebuild()

    aios_db.spatial.persist(force=True)
    summary = _tree_summary(aios_db)
    checks.add("state ∈ {ready, ready_empty}", summary["state"] in ("ready", "ready_empty"),
               str(summary))
    checks.add("树条目 > 0", (summary["entries"] or 0) > 0, str(summary))
    rows = _check_tree_matches_pointers(checks, aios_db)
    checks.add("无待重放空间意图", (summary["pending"] or 0) == 0, str(summary))

    extra_fields: dict = {}
    if params.get("dump"):
        extra_fields["pointer_hashes"] = _pointer_hash_set(rows)
        extra_fields["tree"] = sorted(
            [refno, *box] for refno, box in _tree_value_set(aios_db)
        )
    return checks.result(entries=summary["entries"], epoch=_epoch(aios_db),
                         watermark=watermark, startup=summary, generated=generated,
                         **extra_fields)


def worker_restart(params: dict) -> dict:
    """重启一次进程并对启动裁决断言（S1..S5 与窗间重启共用）。"""
    checks = Checks()
    aios_db = _init_full(params)
    summary = _tree_summary(aios_db)

    checks.add(f"startup_verdict == {params['expect_verdict']}",
               summary["verdict"] == params["expect_verdict"], str(summary))
    expected_state = "ready_empty" if params.get("expect_entries") == 0 else "ready"
    checks.add(f"state == {expected_state}", summary["state"] == expected_state, str(summary))
    checks.add("pending == 0", (summary["pending"] or 0) == 0, str(summary))
    _check_tree_matches_pointers(checks, aios_db)
    if params.get("expect_entries") is not None:
        checks.add("树条目与上一阶段一致", summary["entries"] == params["expect_entries"],
                   f"entries={summary['entries']} expect={params['expect_entries']}")
    return checks.result(entries=summary["entries"], epoch=_epoch(aios_db), startup=summary)


def worker_apply_window(params: dict) -> dict:
    """一个增量窗口：apply → drain → 收尾三件套 → 值级断言。"""
    checks = Checks()
    aios_db = _init_full(params)

    startup = _tree_summary(aios_db)
    checks.add("窗口开跑前启动裁决 == reused（中途重启复用快照）",
               startup["verdict"] == "reused", str(startup))

    end = int(params["end"])
    # 夹具把完整增量源放在 WORK/full，MDB 路径则先换成 sesno-24
    # 基线。真实 watcher 现场是同一 MDB 文件先被发布新会话，再执行
    # apply。因此在 full_init 完成快照复用裁决后，再将源发布到
    # MDB 路径，使后续 pending drain 能按已推进水位钉住 sesno。
    source_path = Path(params["source"])
    if source_path.resolve() != PROJECT_DB.resolve():
        publish_session_snapshot(source_path, end, PROJECT_DB)
    applied = aios_db.incr.apply_file(params["source"], end=end)
    drained = aios_db.incr.drain_data()
    side_effects = aios_db.incr.drain_side_effects()
    reconciled = aios_db.spatial.reconcile()
    aios_db.spatial.persist()

    watermark = aios_db.db.watermark(8000)
    # warnings 只记录不判死：良性形态（如 ZONE 的 children_changed 解析不出生成根）
    # 属于数据长相；真正要防的「静默跳过」表现为 successes 为空 + 水位不动。
    checks.add("apply 无错误且确有成功文件",
               not applied.get("errors") and bool(applied.get("successes")),
               json.dumps({"errors": applied.get("errors"),
                           "warnings": applied.get("warnings")}, ensure_ascii=False)[:300])
    checks.add(f"watermark == {end}", watermark == end,
               f"watermark={watermark}, apply={json.dumps(applied, default=str)[:280]}, "
               f"drain_data={drained}, side_effects={side_effects}, reconciled={reconciled}")

    summary = _tree_summary(aios_db)
    checks.add("state ∈ {ready, ready_empty}",
               summary["state"] in ("ready", "ready_empty"), str(summary))
    rows = _check_tree_matches_pointers(checks, aios_db)
    checks.add("pending == 0（收尾后无滞留意图）", (summary["pending"] or 0) == 0, str(summary))

    epoch = _epoch(aios_db)
    prev = int(params["prev_epoch"])
    if params.get("expect_epoch_bump"):
        checks.add("epoch 严格递增（本窗含空间变更）", epoch > prev, f"{prev} -> {epoch}")
    else:
        checks.add("epoch 单调不减", epoch >= prev, f"{prev} -> {epoch}")

    if params.get("expect") == "box-deleted":
        checks.add("BOX child pe 已软删", _pe_deleted(aios_db, BOX_CHILD) is True, "")
        checks.add("BOX child 几何实例清零（摘树同源）",
                   _inst_rows_of(aios_db, BOX_CHILD) == 0, "")
    if params.get("expect") == "equi-deleted":
        checks.add("EQUI pe 已软删", _pe_deleted(aios_db, EQUI) is True, "")
        checks.add("EQUI 几何实例清零", _inst_rows_of(aios_db, EQUI) == 0, "")

    extra_fields: dict = {}
    if params.get("dump"):
        extra_fields["pointer_hashes"] = _pointer_hash_set(rows)
        extra_fields["tree"] = sorted(
            [refno, *box] for refno, box in _tree_value_set(aios_db)
        )
    return checks.result(entries=summary["entries"], epoch=epoch, watermark=watermark,
                         **extra_fields)


WORKERS = {
    "probe": worker_probe,
    "prepare": worker_prepare,
    "restart": worker_restart,
    "apply-window": worker_apply_window,
}


def worker_main(phase: str, params: dict) -> int:
    try:
        result = WORKERS[phase](params)
    except Exception as error:  # noqa: BLE001 —— worker 的一切失败都要变成结构化结果
        import traceback

        result = {"ok": False, "error": f"{type(error).__name__}: {error}",
                  "trace": traceback.format_exc()[-2000:]}
    print(RESULT_PREFIX + json.dumps(result, ensure_ascii=False, default=str), flush=True)
    return 0 if result.get("ok") else 1


# ── 驱动侧 ───────────────────────────────────────────────────────────────────


class Driver:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.report: list[dict] = []
        self.seq = 0
        self.surreal: subprocess.Popen | None = None
        self.oracle_surreal: subprocess.Popen | None = None
        self.db_sha_before: str | None = None
        self.shelved: list[tuple[Path, Path]] = []

    # -- 阶段执行 --------------------------------------------------------------

    def run_phase(self, name: str, phase: str, params: dict, timeout: float) -> dict:
        self.seq += 1
        started = time.monotonic()
        command = [sys.executable, str(Path(__file__).resolve()),
                   "--worker", phase, "--params", json.dumps(params, ensure_ascii=False)]
        try:
            # uv/venv launchers can replace the interpreter environment for a
            # spawned worker.  Pin the editable package and Rust DLL directory
            # explicitly so every phase imports the same freshly built aios_db.
            worker_env = os.environ.copy()
            pysrc = TESTBED.parent / "pysrc"
            rust_debug = Path(r"D:\Rust\target\debug")
            worker_env["PYTHONPATH"] = os.pathsep.join(
                str(path)
                for path in (pysrc, rust_debug)
                if path.exists()
            )
            proc = subprocess.run(
                command, cwd=str(REPO_ROOT), capture_output=True, text=True,
                encoding="utf-8", errors="replace", timeout=timeout,
                env=worker_env,
            )
            output = (proc.stdout or "") + "\n--- stderr ---\n" + (proc.stderr or "")
            result = None
            for line in (proc.stdout or "").splitlines():
                if line.startswith(RESULT_PREFIX):
                    result = json.loads(line[len(RESULT_PREFIX):])
            if result is None:
                result = {"ok": False, "error": f"worker 无结果行（exit {proc.returncode}）"}
        except subprocess.TimeoutExpired as error:
            output = f"TIMEOUT after {timeout}s\n{error.stdout or ''}\n{error.stderr or ''}"
            result = {"ok": False, "error": f"worker 超时（>{timeout}s）"}

        log_path = LOGS / f"{self.seq:02d}-{name}.log"
        log_path.write_text(output, encoding="utf-8")
        seconds = time.monotonic() - started
        entry = {"phase": name, "ok": result.get("ok", False), "seconds": round(seconds, 1),
                 "result": {k: v for k, v in result.items()
                            if k not in ("checks", "pointer_hashes", "tree")},
                 "failed_checks": [c for c in result.get("checks", []) if not c["ok"]],
                 "log": str(log_path)}
        self.report.append(entry)
        mark = "ok" if entry["ok"] else "FAIL"
        print(f"[{mark}] {name}（{seconds:.1f}s）")
        for check in entry["failed_checks"]:
            print(f"      x {check['name']} — {check['detail']}")
        if not entry["ok"] and "error" in result:
            print(f"      x {result['error']}")
        return result

    def record_synthetic(self, name: str, ok: bool, detail: str) -> None:
        """驱动自己做的裁决（如双库对拍）也进报告，与 worker 阶段同一形状。"""
        entry = {"phase": name, "ok": ok, "seconds": 0.0, "result": {},
                 "failed_checks": [] if ok else [{"name": name, "ok": False,
                                                  "detail": detail[:400]}],
                 "log": ""}
        self.report.append(entry)
        print(f"[{'ok' if ok else 'FAIL'}] {name}" + (f" — {detail[:160]}" if ok and detail else ""))
        if not ok:
            print(f"      x {detail[:400]}")

    # -- 环境与残留 ------------------------------------------------------------

    def recover_previous_crash(self) -> None:
        if DB_BACKUP.exists():
            print(f"发现上次残留的 {DB_BACKUP.name}，先还原项目库文件")
            shutil.copy2(DB_BACKUP, PROJECT_DB)
            DB_BACKUP.unlink()
        for artifact in TREE_ARTIFACTS:
            backup = artifact.with_name(artifact.name + SHELVE_SUFFIX)
            if backup.exists():
                print(f"发现上次残留的 {backup.name}，先还原")
                artifact.unlink(missing_ok=True)
                backup.rename(artifact)

    def preflight(self) -> None:
        assert SURREAL_EXE.exists(), f"缺 {SURREAL_EXE}（仓库自带 fork 2.1.4 服务端）"
        for port in (SURREAL_PORT, ORACLE_PORT):
            assert not port_in_use(port), (
                f"127.0.0.1:{port} 已被占用——先停掉占用者再跑"
            )
        assert PROJECT_DB.exists(), (
            f"缺项目副本 {PROJECT_DB}；先跑 testbed\\Sync-TestbedProjects.ps1"
        )
        manifest_path = FIXTURE / "manifest.json"
        assert manifest_path.exists(), f"缺 issue-019 夹具 manifest：{manifest_path}"
        self.manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        archive = FIXTURE / self.manifest["archive"]["path"]
        digest = sha256_file(archive)
        assert digest == self.manifest["archive"]["sha256"], (
            f"夹具 zip SHA256 不符：{digest} != {self.manifest['archive']['sha256']}"
        )
        self.archive = archive

    def extract_snapshots(self) -> None:
        EXTRACTED.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(self.archive) as bundle:
            bundle.extractall(EXTRACTED)
        self.snapshot_paths: dict[int, Path] = {}
        for snap in self.manifest["snapshots"]:
            path = EXTRACTED / snap["path"]
            digest = sha256_file(path)
            assert digest == snap["sha256"], (
                f"快照 {snap['path']} SHA256 不符：{digest} != {snap['sha256']}"
            )
            self.snapshot_paths[int(snap["sesno"])] = path
        print(f"夹具快照解包并对账通过：sesno {sorted(self.snapshot_paths)}")

    def shelve_tree_artifacts(self) -> None:
        for artifact in TREE_ARTIFACTS:
            if artifact.exists():
                backup = artifact.with_name(artifact.name + SHELVE_SUFFIX)
                backup.unlink(missing_ok=True)
                artifact.rename(backup)
                self.shelved.append((backup, artifact))
        if self.shelved:
            print(f"仓库根空间树产物已挪走 {len(self.shelved)} 件（结束后还原）")

    def restore_tree_artifacts(self) -> None:
        for artifact in TREE_ARTIFACTS:
            artifact.unlink(missing_ok=True)
        for backup, original in self.shelved:
            if backup.exists():
                backup.rename(original)

    def swap_in_baseline(self) -> None:
        self.db_sha_before = sha256_file(PROJECT_DB)
        shutil.copy2(PROJECT_DB, DB_BACKUP)
        shutil.copy2(self.snapshot_paths[BASELINE_SESNO], PROJECT_DB)
        print(f"项目库文件已换成 sesno-{BASELINE_SESNO} 基线快照（原文件已备份）")

    def restore_project_db(self) -> None:
        if not DB_BACKUP.exists():
            return
        shutil.copy2(DB_BACKUP, PROJECT_DB)
        restored = sha256_file(PROJECT_DB)
        if self.db_sha_before and restored != self.db_sha_before:
            print(f"警告：还原后的 {PROJECT_DB.name} SHA256 与开跑前不符！")
        else:
            DB_BACKUP.unlink()
            print(f"{PROJECT_DB.name} 已逐字节还原")

    def _spawn_surreal(self, port: int, log_name: str) -> subprocess.Popen:
        log = open(WORK / log_name, "w", encoding="utf-8")
        proc = subprocess.Popen(
            [str(SURREAL_EXE), "start", "--user", "root", "--pass", "root",
             "--bind", f"127.0.0.1:{port}", "memory"],
            cwd=str(REPO_ROOT), stdout=log, stderr=subprocess.STDOUT,
        )
        assert wait_port(port), f"SurrealDB 没能在 30s 内起来 @{port}（看 {log_name}）"
        print(f"一次性内存 SurrealDB 已就绪 @{port}")
        return proc

    def start_surreal(self) -> None:
        self.surreal = self._spawn_surreal(SURREAL_PORT, "surreal-8072.log")

    def start_oracle_surreal(self) -> None:
        self.oracle_surreal = self._spawn_surreal(ORACLE_PORT, "surreal-8073.log")

    def stop_surreal(self) -> None:
        for attr in ("surreal", "oracle_surreal"):
            proc = getattr(self, attr)
            if proc is not None:
                proc.kill()
                proc.wait()
                setattr(self, attr, None)

    # -- 库状态篡改（启动矩阵用） ------------------------------------------------

    @staticmethod
    def snapshot_artifact() -> Path:
        return TREE_ARTIFACTS[0]

    def delete_snapshot_files(self) -> None:
        for artifact in TREE_ARTIFACTS:
            artifact.unlink(missing_ok=True)

    def bump_epoch(self) -> None:
        surql("UPSERT spatial_epoch:current SET value = (value?:0) + 1, "
              "updated_at = time::now();")

    def plant_pending_intent(self) -> None:
        refno = EQUI.replace("_", "/")
        surql(
            "BEGIN TRANSACTION;\n"
            "UPSERT incr_side_effect_pending:spatial_reconcile_8000_9999 SET "
            "kind = 'spatial_reconcile', dbnum = 8000, end_sesno = 9999, "
            "db_type = 'DESI', changed_refnos = [], "
            f"refresh_refnos = ['{refno}'], remove_refnos = [], "
            "status = 'pending', attempts = 0, last_error = NONE, "
            "updated_at = time::now();\n"
            "UPSERT spatial_epoch:current SET value = (value?:0) + 1, "
            "updated_at = time::now();\n"
            "COMMIT TRANSACTION;"
        )

    def corrupt_snapshot(self) -> None:
        snapshot = self.snapshot_artifact()
        assert snapshot.exists(), f"预期存在的快照文件不见了：{snapshot}"
        with open(snapshot, "r+b") as handle:
            handle.write(b"SPATIAL8000-CORRUPTED-ON-PURPOSE")

    # -- 双库对拍（oracle） -------------------------------------------------------

    def run_oracle(self, a26: dict, ensure_refnos: list[str]) -> None:
        """B 侧：final-26 直接建基线 + 按 A 侧清单生成，与 A@26 逐条对拍。

        `aabb` 是内容寻址记录（id = 值哈希）：两条路径若收敛到同一几何，
        `(refno, aabb哈希)` 集合必须逐条相等——比数值容差更强的判据。
        """
        self.start_oracle_surreal()
        # 原始文件仍在 DB_BACKUP 里；当前内容（snapshot-24）直接被 final-26 顶掉，
        # finally 的 restore_project_db 统一还原。
        shutil.copy2(self.snapshot_paths[26], PROJECT_DB)
        print(f"项目库文件已换成 final-26（对照库基线用）；生成清单 {ensure_refnos}")

        oracle = self.run_phase(
            "ORACLE B：final-26 直接建基线 @8073", "prepare",
            {"config": str(ORACLE_CONFIG), "expect_watermark": 26,
             "ensure_equi": False, "ensure_refnos": ensure_refnos, "dump": True},
            timeout=3600)
        if not oracle.get("ok"):
            self.record_synthetic("ORACLE 对拍 A@26 == B", False,
                                  "B 侧建库失败，对拍未执行")
            return

        a_hashes = a26.get("pointer_hashes") or []
        b_hashes = oracle.get("pointer_hashes") or []
        only_a = sorted(set(a_hashes) - set(b_hashes))[:5]
        only_b = sorted(set(b_hashes) - set(a_hashes))[:5]
        self.record_synthetic(
            "ORACLE 对拍：指针哈希集 A@26 == B",
            a_hashes == b_hashes,
            f"A={len(a_hashes)} B={len(b_hashes)} A独有={only_a} B独有={only_b}")

        a_tree = [tuple(entry) for entry in (a26.get("tree") or [])]
        b_tree = [tuple(entry) for entry in (oracle.get("tree") or [])]
        only_a_tree = sorted(set(a_tree) - set(b_tree))[:5]
        only_b_tree = sorted(set(b_tree) - set(a_tree))[:5]
        self.record_synthetic(
            "ORACLE 对拍：空间树内容 A@26 == B",
            a_tree == b_tree,
            f"A={len(a_tree)} B={len(b_tree)} A独有={only_a_tree} B独有={only_b_tree}")

    # -- 主流程 -----------------------------------------------------------------

    def run(self) -> int:
        WORK.mkdir(exist_ok=True)
        LOGS.mkdir(parents=True, exist_ok=True)
        self.recover_previous_crash()
        self.preflight()
        self.extract_snapshots()

        # 血统探针在 swap 之前对真实文件做（离线 parse，不连库）。
        probe = self.run_phase(
            "probe（真实 ams8000 血统）", "probe",
            {"file": str(PROJECT_DB), "config": str(CONFIG),
             "stat_upto": BASELINE_SESNO + self.args.max_windows},
            timeout=300)
        if probe.get("lineage_ok"):
            # 保留 AVEVA 命名形态：增量管线按文件名判「是不是库文件」
            # （is_pdms_db_file_name），改名会被当 copy 文件静默跳过。
            source_dir = WORK / "full"
            source_dir.mkdir(parents=True, exist_ok=True)
            source = source_dir / PROJECT_DB.name
            shutil.copy2(PROJECT_DB, source)
            windows = [s for s in probe["sessions"] if s > BASELINE_SESNO]
            lineage_note = f"真实文件（latest sesno {probe.get('latest_sesno')}）"
        else:
            source = self.snapshot_paths[26]
            windows = [25, 26]
            lineage_note = "真实文件历史与 issue-019 不吻合，降级用夹具 final（只有 25/26）"
        windows = windows[: self.args.max_windows]
        window_ops = probe.get("window_ops") or {}

        def ops_label(sesno: int) -> str:
            counts = window_ops.get(str(sesno)) or {}
            return "+".join(f"{kind}×{n}" for kind, n in sorted(counts.items())) or "无操作统计"

        print(f"增量源：{lineage_note}；回放窗口 "
              f"{[f'{w}({ops_label(w)})' for w in windows]}")

        self.shelve_tree_artifacts()
        self.swap_in_baseline()
        exit_code = 1
        try:
            self.start_surreal()
            cfg = {"config": str(CONFIG)}

            prepare = self.run_phase("P0 prepare（基线 + 生成 + 落快照）", "prepare",
                                     {**cfg, "gen_roots": self.args.gen_roots,
                                      "expect_watermark": BASELINE_SESNO,
                                      "ensure_equi": True,
                                      "skip_baseline_backlog": self.args.skip_baseline_backlog},
                                     timeout=3600)
            if not prepare.get("ok"):
                return 1
            entries, epoch = prepare["entries"], prepare["epoch"]
            a_generated = [r for r in prepare.get("generated", []) if r != EQUI]

            restart = self.run_phase("S1 快照新鲜 → reused", "restart",
                                     {**cfg, "expect_verdict": "reused",
                                      "expect_entries": entries},
                                     timeout=900)
            self.delete_snapshot_files()
            restart = self.run_phase("S2 快照缺失 → rebuilt", "restart",
                                     {**cfg, "expect_verdict": "rebuilt"}, timeout=900)
            entries = restart.get("entries", entries)

            self.bump_epoch()
            restart = self.run_phase("S3 库侧 epoch 漂移 → rebuilt", "restart",
                                     {**cfg, "expect_verdict": "rebuilt"}, timeout=900)

            self.plant_pending_intent()
            restart = self.run_phase("S4 携带待重放意图 → replayed", "restart",
                                     {**cfg, "expect_verdict": "replayed"}, timeout=900)

            self.corrupt_snapshot()
            restart = self.run_phase("S5 快照损坏 → rebuilt", "restart",
                                     {**cfg, "expect_verdict": "rebuilt"}, timeout=900)
            entries = restart.get("entries", entries)
            epoch = restart.get("epoch", epoch)

            a26: dict | None = None
            if not self.args.skip_windows:
                for end in windows:
                    expect = {25: "box-deleted", 26: "equi-deleted"}.get(end)
                    # 只有 W25 必然 bump：BOX 是 EQUI 子树里唯一带几何的元素，
                    # 它的树条目确实被摘掉。W26 删 EQUI 时树上已无其条目——
                    # 「树应有内容」没变就不 bump 正是设计；其余窗口是否动树
                    # 取决于会话内容，只要求 epoch 单调不减。
                    window = self.run_phase(
                        f"W{end} 增量窗口 apply(end={end})（{ops_label(end)}）",
                        "apply-window",
                        {**cfg, "source": str(source), "end": end, "prev_epoch": epoch,
                         "expect": expect, "expect_epoch_bump": end == 25,
                         "dump": end == 26},
                        timeout=1800)
                    if not window.get("ok"):
                        print("窗口失败，中止后续窗口（水位未推进，续跑无意义）")
                        break
                    if end == 26:
                        a26 = window
                    entries, epoch = window["entries"], window["epoch"]
                    check = self.run_phase(
                        f"W{end} 之后重启 → reused", "restart",
                        {**cfg, "expect_verdict": "reused", "expect_entries": entries},
                        timeout=900)
                    if not check.get("ok"):
                        break

            # ── 双库对拍：A（基线@24 + 回放到 26）vs B（final-26 直接建基线）。
            # 必须排在 A 全部阶段之后：B 与 A 同项目名，B 的启动会用自己的指纹
            # 覆盖仓库根同名快照文件，排前面会打翻 A 后续的 reused 断言。
            if a26 is not None and not self.args.skip_oracle:
                self.run_oracle(a26, a_generated)

            exit_code = 0 if all(entry["ok"] for entry in self.report) else 1
            return exit_code
        finally:
            self.stop_surreal()
            self.restore_project_db()
            self.restore_tree_artifacts()
            report_path = Path(self.args.json_report) if self.args.json_report else WORK / "report.json"
            report_path.write_text(
                json.dumps(self.report, ensure_ascii=False, indent=2, default=str),
                encoding="utf-8")
            passed = sum(1 for entry in self.report if entry["ok"])
            print(f"\n{'全部通过' if passed == len(self.report) else '有失败'}"
                  f"：{passed}/{len(self.report)}；报告 {report_path}")
            if exit_code == 0:
                shutil.rmtree(EXTRACTED, ignore_errors=True)
                shutil.rmtree(WORK / "full", ignore_errors=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--worker", help="内部：以 worker 角色跑一个阶段")
    parser.add_argument("--params", help="内部：worker 参数（JSON）")
    parser.add_argument("--max-windows", type=int, default=6,
                        help="最多回放多少个增量窗口（默认 6）")
    parser.add_argument("--gen-roots", type=int, default=3,
                        help="基线阶段额外生成多少个抽样 EQUI 根（默认 3）")
    parser.add_argument(
        "--skip-baseline-backlog", action="store_true",
        help="只保留显式 ensure 的基线根，清除基线全库 pending；窗口 pending 仍正常 drain",
    )
    parser.add_argument("--skip-windows", action="store_true", help="只跑启动裁决矩阵")
    parser.add_argument("--skip-oracle", action="store_true",
                        help="跳过双库对拍（B 侧 final-26 基线 @8073）")
    parser.add_argument("--json-report", help="报告 JSON 输出路径（默认 .spatial8000/report.json）")
    args = parser.parse_args()

    if args.worker:
        return worker_main(args.worker, json.loads(args.params or "{}"))
    return Driver(args).run()


if __name__ == "__main__":
    raise SystemExit(main())
