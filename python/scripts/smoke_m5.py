# -*- coding: utf-8 -*-
"""M5 验收冒烟：纯 Python 闭环缺口补齐 —— spatial 模块 + incr.resolve_window /
drain_side_effects / queue_status + room.code / nodes / names。

分两段：
- 连接层段（服务可以在跑）：resolve_window / queue_status / spatial.status /
  room.code|nodes|names + 执行层函数的硬守护检查。
- 执行层段（须先停服务）：--full 才跑，依次 drain_side_effects →
  spatial.reconcile → spatial.persist，验证零售组合的收尾三件套。

从 python/ 目录运行：.venv\\Scripts\\python.exe scripts\\smoke_m5.py [--full]

**历史验收记录，不可原样复跑**（2026-08-12 起）：本脚本钉在 M5 当时的环境上
——仓库根 `DbOption` + 8009 正式库 + `D:/AVEVA/...` 真实工程。8009 的数据目录
已被 SurrealDB 3.x 写坏且决定不修（见 `python/testbed/README.md`），照原样跑
必失败。
等价物：硬守护见 `pytest -m offline`（`test_guards_offline.py`，33 个入口逐条
验，且在干净子解释器里跑）；spatial / room / queue 的只读面见
`pytest -m "not offline"`（`test_connection_layer.py`）；收尾三件套见
`python/testbed/run_full_loop.py`。
"""

import sys
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

REPO_ROOT = Path(__file__).resolve().parents[2]
DB_FILE = Path("D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams7997_0001")
EQUI = "24381_100677"  # /-RX-CUP-001FA（M2..M4 同款样本）
RUN_FULL = "--full" in sys.argv[1:]

import aios_db

aios_db.set_config(str(REPO_ROOT / "DbOption"))

failures = []


def check(name, ok, detail=""):
    print(f"[{'ok' if ok else 'FAIL'}] {name}" + (f" — {detail}" if detail else ""))
    if not ok:
        failures.append(name)


# ── 0. 模块面：新入口全部可见 ────────────────────────────────────────────────
check(
    "spatial 模块注册",
    all(hasattr(aios_db.spatial, f) for f in ("status", "reconcile", "persist", "rebuild")),
)
check(
    "incr 新入口注册",
    all(hasattr(aios_db.incr, f) for f in ("resolve_window", "drain_side_effects", "queue_status")),
)
check("room 新入口注册", all(hasattr(aios_db.room, f) for f in ("code", "nodes", "names")))

# ── 1. 硬守护：mutating 新入口未 full_init 一律拒绝 ─────────────────────────
for name, call in [
    ("incr.drain_side_effects", aios_db.incr.drain_side_effects),
    ("spatial.reconcile", aios_db.spatial.reconcile),
    ("spatial.persist", aios_db.spatial.persist),
    ("spatial.rebuild", aios_db.spatial.rebuild),
]:
    try:
        call()
        check(f"{name} 硬守护", False, "未 full_init 居然没拦")
    except RuntimeError as e:
        check(f"{name} 硬守护", "full_init" in str(e), str(e)[:50])

# 连接层只读新入口在未 connect 时也要拒绝（用 connect 门而不是 full_init 门）
try:
    aios_db.spatial.status()
    check("spatial.status 未连接守护", False, "未连接居然没拦")
except RuntimeError as e:
    check("spatial.status 未连接守护", "connect" in str(e), str(e)[:50])

# ── 2. 连接层只读实测 ────────────────────────────────────────────────────────
aios_db.connect(cwd=str(REPO_ROOT))

st = aios_db.spatial.status()
check(
    "spatial.status",
    isinstance(st, dict) and {"pending", "retries", "stalled"} <= set(st),
    f"pending={st.get('pending')} stalled={st.get('stalled')}",
)

qs = aios_db.incr.queue_status()
check(
    "incr.queue_status",
    isinstance(qs, dict) and "paused" in qs and isinstance(qs.get("rows"), list),
    f"paused={qs.get('paused')} rows={len(qs.get('rows', []))}",
)

if DB_FILE.exists():
    win = aios_db.incr.resolve_window(str(DB_FILE))
    ok = isinstance(win, dict) and "up_to_date" in win and "dbnum" in win
    if ok and not win["up_to_date"]:
        ok = isinstance(win.get("window"), list) and len(win["window"]) == 2
    check("incr.resolve_window", ok, f"{win}")
else:
    print(f"[skip] incr.resolve_window — 测试库文件不存在: {DB_FILE}")

code = aios_db.room.code(EQUI)
check("room.code", code is None or isinstance(code, str), f"code={code!r}")
nodes = aios_db.room.nodes(EQUI)
names = aios_db.room.names(EQUI)
check(
    "room.nodes / names",
    isinstance(nodes, list) and isinstance(names, list),
    f"nodes={len(nodes)} names={names[:5]}",
)

# ── 3. 执行层段（--full；先停服务）──────────────────────────────────────────
if RUN_FULL:
    aios_db.full_init(cwd=str(REPO_ROOT))
    done = aios_db.incr.drain_side_effects()
    check("incr.drain_side_effects", isinstance(done, int), f"done={done}")
    converged = aios_db.spatial.reconcile()
    check("spatial.reconcile", isinstance(converged, int), f"converged={converged}")
    persisted = aios_db.spatial.persist()
    check("spatial.persist", isinstance(persisted, bool), f"persisted={persisted}")
    st2 = aios_db.spatial.status()
    check("收尾后无空间积压", st2.get("pending") == 0, f"pending={st2.get('pending')}")
else:
    print("[skip] 执行层段 — 加 --full 且先停服务再跑")

print()
if failures:
    print(f"M5 冒烟失败 {len(failures)} 项: {failures}")
    sys.exit(1)
print("M5 冒烟全绿" + ("" if RUN_FULL else "（连接层段）"))
