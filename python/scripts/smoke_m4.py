# -*- coding: utf-8 -*-
"""M4 验收冒烟：4 个★新入口 —— parse.element / parse.noun_dict /
model.export_obj / sync.baseline（守护与错误路径）。

前置：SurrealDB fork server 在跑；gen-model 服务停着（本脚本不 full_init，
export_obj 走连接层）。从 python/ 目录运行：.venv\\Scripts\\python.exe scripts\\smoke_m4.py

**历史验收记录，不可原样复跑**（2026-08-12 起）：本脚本钉在 M4 当时的环境上
——仓库根 `DbOption` + 8009 正式库 + `D:/AVEVA/...` 真实工程。8009 的数据目录
已被 SurrealDB 3.x 写坏且决定不修（见 `python/testbed/README.md`），照原样跑
必失败。
等价物：`parse.element` 见 `pytest -m offline`（`test_parse_offline.py`，含
删元素读历史版本那条坑）；`export_obj` / `sync.baseline` 见
`python/testbed/run_full_loop.py`。`parse.noun_dict` 依赖 E3D 装机的
`attlib.dat`，**没有**自动化等价物，要验只能手跑本脚本对应段落。
"""

import json
import sys
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

REPO_ROOT = Path(__file__).resolve().parents[2]
DB_FILE = Path("D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams7997_0001")
ATTLIB = next(
    (
        p
        for p in (
            Path("D:/AVEVA/Everything3D3.1/attlib.dat"),
            Path("D:/AVEVA/Everything3D2.10/attlib.dat"),
        )
        if p.exists()
    ),
    Path("D:/AVEVA/attlib.dat"),
)
EQUI = "24381_100677"  # M2/M3 用过的 /-RX-CUP-001FA（已生成过 17 个实例）

import aios_db

aios_db.set_config(str(REPO_ROOT / "DbOption"))

failures = []


def check(name, ok, detail=""):
    print(f"[{'ok' if ok else 'FAIL'}] {name}" + (f" — {detail}" if detail else ""))
    if not ok:
        failures.append(name)


# ── 1. parse.element：最新版本 + 历史版本 ────────────────────────────────────
ele = aios_db.parse.element(str(DB_FILE), EQUI)
check(
    "parse.element 最新版本",
    ele["refno"] == EQUI and ele["noun"] == "EQUI" and ele["attrs"],
    f"noun={ele['noun']} name={ele.get('name')!r} found_sesno={ele['found_sesno']} "
    f"attrs={len(ele['attrs'])}",
)

# 历史版本：从 collect_changes 找一个真实被改过属性的元素，对比新旧两个版本
window = aios_db.parse.collect_changes(str(DB_FILE), 100, 102, detail=True)
target = next(
    (
        (op["refno"], int(sesno), sorted(op["modified"]))
        for sesno, ops in sorted(window.items(), key=lambda kv: int(kv[0]))
        for op in ops
        if op["op"] == "modified" and op.get("modified")
    ),
    None,
)
if target is None:
    print("[skip] parse.element 历史版本 — 窗口 100..102 内没有 modified 操作")
else:
    refno, ses, changed = target
    # 在「修改发生的会话」与「前一会话」两个历史点读，不依赖最新索引
    #（被改过的元素之后可能又被删，最新索引里就没有它了）。
    cur = aios_db.parse.element(str(DB_FILE), refno, sesno=ses)
    old = aios_db.parse.element(str(DB_FILE), refno, sesno=ses - 1)
    diff = [
        key
        for key in changed
        if cur["attrs"].get(key) != old["attrs"].get(key)
        or cur["explicit_attrs"].get(key) != old["explicit_attrs"].get(key)
    ]
    check(
        "parse.element 历史版本对比",
        old["found_sesno"] < ses == cur["found_sesno"] and bool(diff),
        f"{refno} 会话 {old['found_sesno']}→{cur['found_sesno']}，变了 {diff}",
    )

# 不存在的 refno 要报干净的错
try:
    aios_db.parse.element(str(DB_FILE), "1_1")
    check("parse.element 不存在报错", False, "居然没抛异常")
except RuntimeError as e:
    check("parse.element 不存在报错", "找不到元素" in str(e), str(e)[:60])

# ── 2. parse.noun_dict ───────────────────────────────────────────────────────
if ATTLIB.exists():
    dic = aios_db.parse.noun_dict(str(ATTLIB))
    nouns = {row["noun_name"]: row for row in dic["nouns"] if row["noun_name"]}
    equi, box = nouns.get("EQUI"), nouns.get("BOX")
    check(
        "parse.noun_dict 规模",
        dic["noun_count"] > 1000 and dic["field_count"] > 50,
        f"nouns={dic['noun_count']} fields={dic['field_count']}",
    )
    check(
        "parse.noun_dict 能力矩阵语义",
        equi is not None and box is not None and box["primitive"] and not equi["primitive"],
        f"BOX.primitive={box and box['primitive']} EQUI.primitive={equi and equi['primitive']}",
    )
else:
    print(f"[skip] parse.noun_dict — attlib 不存在: {ATTLIB}")

# ── 3. model.export_obj：连接层门 + 真实导出 ────────────────────────────────
try:
    aios_db.model.export_obj(EQUI, "obj_out")
    check("export_obj 未连接守护", False, "未连接居然没拦")
except RuntimeError as e:
    check("export_obj 未连接守护", "connect" in str(e), str(e)[:60])

aios_db.connect(cwd=str(REPO_ROOT))
out_dir = REPO_ROOT / "python" / ".scratch" / "obj_m4"

# 目标不写死：优先 EQUI，再从 inst_relate 里挑有实例的元素兜底
candidates = [EQUI] + [
    row["refno"]
    for row in aios_db.db.query(
        "SELECT record::id(in) AS refno FROM inst_relate LIMIT 20;"
    )[0]
]
exported = None
for refno in candidates:
    try:
        result = aios_db.model.export_obj(refno, str(out_dir))
    except RuntimeError:
        continue
    good = [f for f in result["files"] if f["triangles"] > 0]
    if good:
        exported = (refno, result, good)
        break
check("export_obj 导出", exported is not None, f"尝试了 {len(candidates)} 个候选")
if exported:
    refno, result, good = exported
    first = good[0]
    text = Path(first["path"]).read_text()
    check(
        "export_obj OBJ 内容",
        Path(first["path"]).exists() and text.count("\nv ") > 10 and "\nf " in text,
        f"{refno} → {Path(first['path']).name} insts={first['exported_insts']} "
        f"tris={first['triangles']} missing={len(result['missing_meshes'])}",
    )

# ── 4. sync.baseline：full_init 守护 + 参数校验路径 ─────────────────────────
try:
    aios_db.sync.baseline(99999)
    check("sync.baseline 硬守护", False, "未 full_init 居然没拦")
except RuntimeError as e:
    check("sync.baseline 硬守护", "full_init" in str(e), str(e)[:60])

aios_db.full_init(cwd=str(REPO_ROOT))
try:
    aios_db.sync.baseline(99999)
    check("sync.baseline 不存在的 dbnum 报错", False, "居然没抛异常")
except RuntimeError as e:
    check("sync.baseline 不存在的 dbnum 报错", "99999" in str(e), str(e)[:80])

print()
if failures:
    print(f"M4 冒烟失败 {len(failures)} 项: {failures}")
    sys.exit(1)
print("M4 冒烟全绿")
