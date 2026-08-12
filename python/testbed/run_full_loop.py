# -*- coding: utf-8 -*-
"""Python 绑定测试沙箱的全链路冒烟：解析 → 基线 → 生成 → 导出 → 房间/收尾。

一切都发生在沙箱自己的资源上——项目副本（testbed/projects）、8019 专用
SurrealDB（testbed/.surreal/pytest-ams）、项目副本里的单实例锁——与 8009
正式库和 9099 在跑服务零接触，**不用停任何服务**就能测执行层。

用法（在 python/ 目录）：
    .venv\\Scripts\\python.exe testbed\\run_full_loop.py                 # 全链路
    .venv\\Scripts\\python.exe testbed\\run_full_loop.py --parse-only    # 只测解析层
    .venv\\Scripts\\python.exe testbed\\run_full_loop.py --force-baseline

前置（README 有全套步骤）：
    1. .\\testbed\\Sync-TestbedProjects.ps1     # 项目副本就位（一次性/重灌）
    2. .\\testbed\\Start-TestSurreal.ps1        # 8019 测试库在跑
"""

from __future__ import annotations

import argparse
import json
import socket
import sys
import time
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

TESTBED = Path(__file__).resolve().parent
REPO_ROOT = TESTBED.parents[1]
CONFIG = TESTBED / "DbOption-pytest"
SURREAL_ADDR = ("127.0.0.1", 8019)

failures: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> bool:
    print(f"[{'ok' if ok else 'FAIL'}] {name}" + (f" — {detail}" if detail else ""))
    if not ok:
        failures.append(name)
    return ok


def brief(value, limit: int = 160) -> str:
    text = json.dumps(value, ensure_ascii=False, default=str)
    return text if len(text) <= limit else text[:limit] + "…"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dbnum", type=int, default=7997, help="目标 DESI 库号")
    parser.add_argument("--name", default="/-RX-CUP-001FA",
                        help="生成用样本构件名（M2..M5 冒烟同款 EQUI）")
    parser.add_argument("--parse-only", action="store_true", help="只跑解析层就退出")
    parser.add_argument("--force-baseline", action="store_true",
                        help="已有水位也强制重跑一遍基线")
    args = parser.parse_args()

    db_file = TESTBED / "projects" / "AvevaMarineSample" / "ams000" / f"ams{args.dbnum}_0001"

    import aios_db

    aios_db.set_config(str(CONFIG))

    # ── 1. 解析层（不连库） ──────────────────────────────────────────────────
    print(f"\n== 解析层：{db_file.name}（项目副本） ==")
    if not check("库文件副本存在", db_file.exists(), str(db_file)):
        print("先跑 testbed\\Sync-TestbedProjects.ps1 灌数据。")
        return 1
    check("is_db_file", aios_db.parse.is_db_file(str(db_file)))
    header = aios_db.parse.header(str(db_file))
    check("header", header.get("dbnum") == args.dbnum, brief(header))
    sessions = aios_db.parse.sessions(str(db_file))
    check("sessions", len(sessions) > 0, f"{len(sessions)} 个会话，最新 {header.get('latest_sesno')}")
    latest = header.get("latest_sesno") or 0
    if latest >= 2:
        changes = aios_db.parse.collect_changes(str(db_file), latest - 1, latest)
        check("collect_changes", isinstance(changes, (list, dict)),
              f"窗口 {latest - 1}..{latest} → {brief(changes, 120)}")

    if args.parse_only:
        print("\n[parse-only] 到此为止。")
        return 0 if not failures else 1

    # ── 2. 执行层初始化（沙箱自己的锁 + 8019 专用库） ────────────────────────
    print("\n== 执行层：full_init（testbed 专用 SurrealDB @8019） ==")
    try:
        with socket.create_connection(SURREAL_ADDR, timeout=2):
            pass
    except OSError:
        check("SurrealDB(8019) 可达", False,
              "先在另一个终端跑 testbed\\Start-TestSurreal.ps1")
        return 1
    check("SurrealDB(8019) 可达", True)

    started = time.time()
    aios_db.full_init(cwd=str(REPO_ROOT))
    check("full_init", True, f"{time.time() - started:.1f}s（锁在项目副本根，与在跑服务无关）")

    # ── 3. 基线（首次入库；CATA 按需，不整库预热） ──────────────────────────
    print(f"\n== 基线：dbnum={args.dbnum} ==")
    watermark = aios_db.db.watermark(args.dbnum)
    if watermark and not args.force_baseline:
        check("基线已就位，跳过", True, f"watermark={watermark}")
    else:
        started = time.time()
        report = aios_db.sync.baseline(args.dbnum, "AvevaMarineSample")
        watermark = aios_db.db.watermark(args.dbnum)
        check("sync.baseline", watermark > 0,
              f"{time.time() - started:.1f}s，watermark={watermark}，{brief(report)}")

    count_rows = aios_db.db.query(
        f"SELECT count() FROM pe WHERE dbnum={args.dbnum} GROUP ALL;")
    check("pe 行数", bool(count_rows), brief(count_rows, 80))

    # ── 4. 生成：单根重生成（CATA 闭包按需读文件） ──────────────────────────
    print(f"\n== 生成：{args.name} ==")
    refnos = aios_db.db.by_name(args.name, args.dbnum)
    if not check("by_name 定位", bool(refnos), f"{args.name} → {refnos}"):
        return 1
    refno = refnos[0]
    started = time.time()
    report = aios_db.model.ensure(refno, force=True)
    elapsed = time.time() - started
    check("model.ensure(force)", isinstance(report, dict), f"{elapsed:.1f}s，{brief(report)}")
    insts = aios_db.db.inst(refno)
    check("inst_relate 实例", len(insts) > 0, f"{len(insts)} 条实例边")

    # ── 5. 导出 OBJ 目视（整树单文件的形状断言，2026-08-12 审查修复计划 P3） ──
    out_dir = TESTBED / "out"
    out_dir.mkdir(exist_ok=True)
    export = aios_db.model.export_obj(refno, str(out_dir))
    check("model.export_obj", isinstance(export, dict), brief(export))
    files = export.get("files", []) if isinstance(export, dict) else []
    check("导出=整树单文件", len(files) == 1, brief(export, 120))
    if files:
        entry = files[0]
        obj_path = Path(entry.get("path", ""))
        if check("obj 文件在场", obj_path.is_file(), str(obj_path)):
            text = obj_path.read_text(encoding="utf-8")
            groups = sum(1 for line in text.splitlines() if line.startswith("o "))
            check("o 组数 == 导出实例数", groups == entry.get("exported_insts"),
                  f"{groups} 组 vs exported_insts={entry.get('exported_insts')}")
            check("triangles > 0", (entry.get("triangles") or 0) > 0,
                  f"triangles={entry.get('triangles')}")
    check("无缺失 mesh（刚 force 生成过）", not export.get("missing_meshes"),
          brief(export.get("missing_meshes", []), 120))

    # ── 6. 房间 + 零售组合收尾三件套 ────────────────────────────────────────
    print("\n== 房间与收尾 ==")
    drained = aios_db.room.drain()
    check("room.drain", isinstance(drained, dict), brief(drained))
    side_effects = aios_db.incr.drain_side_effects()
    check("incr.drain_side_effects", isinstance(side_effects, int), f"{side_effects} 条")
    reconciled = aios_db.spatial.reconcile()
    persisted = aios_db.spatial.persist()
    status = aios_db.spatial.status()
    check("spatial.reconcile+persist", isinstance(reconciled, int),
          f"收敛 {reconciled} 条，persist={persisted}，status={brief(status, 100)}")

    # ── 汇总 ────────────────────────────────────────────────────────────────
    print(f"\n{'全部通过' if not failures else '失败项：' + ', '.join(failures)}")
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
