# -*- coding: utf-8 -*-
"""RVM 窄口径 AABB 对拍：E3D 导出的 `.rvm.json` vs gen-model `inst_relate`。

判定与 `scripts/e3d/rvm_gate_c_or_1r345_c.ps1` 同源——比三维尺寸跨度，
不依赖 ATT 身份解析；按「NOUN 序号」配对（`WALL 1 of …`）。

    .venv\\Scripts\\python.exe python\\scripts\\rvm_aabb_compare.py --fixture 1rs-wf03-w-c-rr001
    .venv\\Scripts\\python.exe python\\scripts\\rvm_aabb_compare.py --fixture c-or-1r345-c

1112 是结构库（WALL/STWALL 走 SweepSolid；GWALL 是挤出，本夹具不收）。
缺 `test_data/rvm/<根名>.rvm.json` 时默认只打印生成侧并退出 2；
`--require-snapshot` 则视为失败。先在 E3D 跑对应 `scripts/e3d/rvm_export_*.mac`，
再 `rvm_verify import --scope narrow`。
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Iterable, Sequence

sys.stdout.reconfigure(encoding="utf-8")

REPO = Path(__file__).resolve().parents[2]
ABS_TOL_MM = 3.0
REL_TOL = 0.03
SWEEP_NOUNS = ("WALL", "STWALL")


@dataclass(frozen=True)
class Pair:
    rvm: str
    gen: str
    noun: str


@dataclass(frozen=True)
class Fixture:
    key: str
    dbnum: int
    root_name: str
    root_refno: str
    snapshot: Path
    rvm_file: Path
    nouns: tuple[str, ...] | None
    pairs: tuple[Pair, ...] | None


def _rvm_paths(stem: str) -> tuple[Path, Path]:
    base = REPO / "test_data" / "rvm" / stem
    return base.with_suffix(".rvm.json"), base.with_suffix(".rvm")


_C_OR_JSON, _C_OR_RVM = _rvm_paths("C-OR-1R345-C")
_RR001_JSON, _RR001_RVM = _rvm_paths("1RS-WF03-W-C-RR001")

FIXTURES: dict[str, Fixture] = {
    "1rs-wf03-w-c-rr001": Fixture(
        key="1rs-wf03-w-c-rr001",
        dbnum=1112,
        root_name="/1RS-WF03-W-C-RR001",
        root_refno="17496_105799",
        snapshot=_RR001_JSON,
        rvm_file=_RR001_RVM,
        nouns=SWEEP_NOUNS,
        pairs=None,
    ),
    "c-or-1r345-c": Fixture(
        key="c-or-1r345-c",
        dbnum=8000,
        root_name="/C-OR-1R345-C",
        root_refno="24384_23257",
        snapshot=_C_OR_JSON,
        rvm_file=_C_OR_RVM,
        nouns=None,
        pairs=(
            Pair("FTUBE 1", "24384_23258", "FTUB"),
            Pair("BEND 1", "24384_23259", "BEND"),
            Pair("FTUBE 2", "24384_23260", "FTUB"),
            Pair("FTUBE 3", "24384_23261", "FTUB"),
            Pair("FTUBE 4", "24384_23262", "FTUB"),
            Pair("BEND 2", "24384_23263", "BEND"),
            Pair("FTUBE 5", "24384_23264", "FTUB"),
            Pair("FTUBE 6", "24384_23265", "FTUB"),
            Pair("FTUBE 7", "24384_23266", "FTUB"),
        ),
    ),
}


def span(box: Sequence[float] | None) -> tuple[float, float, float] | None:
    """`[xmin,ymin,zmin,xmax,ymax,zmax]` → `(dx,dy,dz)`。"""
    if box is None or len(box) != 6:
        return None
    return (box[3] - box[0], box[4] - box[1], box[5] - box[2])


def gen_box(mins: Sequence[float] | None, maxs: Sequence[float] | None) -> list[float] | None:
    if not mins or not maxs or len(mins) != 3 or len(maxs) != 3:
        return None
    return [float(mins[0]), float(mins[1]), float(mins[2]), float(maxs[0]), float(maxs[1]), float(maxs[2])]


def compare_spans(
    rvm: Sequence[float],
    gen: Sequence[float],
    abs_tol: float = ABS_TOL_MM,
    rel_tol: float = REL_TOL,
) -> tuple[str, float]:
    """三维跨度逐轴 `|gen-e3d| <= max(abs_tol, rel_tol * e3d)`。

    返回 `(OK|FAIL, 超出容差的最大余量)`；余量 ≤ 0 为 OK。
    """
    worst = 0.0
    for axis in range(3):
        tol = max(abs_tol, rel_tol * abs(rvm[axis]))
        delta = abs(rvm[axis] - gen[axis])
        excess = delta - tol
        if excess > worst:
            worst = excess
    return ("OK" if worst <= 0 else "FAIL", worst)


def find_rvm_member(members: Iterable[dict[str, Any]], prefix: str) -> dict[str, Any] | None:
    """`WALL 1` 匹配 `WALL 1 of CWALL /…`；要求前缀后跟空格，避免 `WALL 1` 误中 `WALL 10`。"""
    needle = prefix + " "
    for member in members:
        name = member.get("name") or ""
        if name == prefix or name.startswith(needle):
            return member
    return None


def load_snapshot(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def pairs_from_children(children: Sequence[dict[str, Any]], nouns: Sequence[str]) -> list[Pair]:
    """按 noun 分组、refno 序号升序，赋 E3D 同类序号（WALL 1、STWALL 1…）。"""
    wanted = {noun.upper() for noun in nouns}
    grouped: dict[str, list[dict[str, Any]]] = {noun: [] for noun in wanted}
    for child in children:
        noun = str(child.get("noun") or "").upper()
        if noun not in wanted:
            continue
        grouped[noun].append(child)
    pairs: list[Pair] = []
    for noun in nouns:
        rows = grouped.get(noun.upper(), [])
        rows.sort(key=_refno_sort_key)
        for index, row in enumerate(rows, start=1):
            pairs.append(Pair(rvm=f"{noun} {index}", gen=str(row["refno"]), noun=noun))
    return pairs


def _refno_sort_key(row: dict[str, Any]) -> tuple[int, int]:
    raw = str(row.get("refno") or "").replace("/", "_")
    left, _, right = raw.partition("_")
    try:
        return (int(left), int(right) if right else 0)
    except ValueError:
        return (0, 0)


def fetch_gen_aabbs(query: Callable[..., Any], refnos: Sequence[str]) -> dict[str, dict[str, Any]]:
    if not refnos:
        return {}
    keys = ", ".join(f"inst_relate:{refno}" for refno in refnos)
    sql = (
        "SELECT record::id(id) AS refno, aabb_d.mins AS mins, aabb_d.maxs AS maxs "
        f"FROM [{keys}] WHERE id != NONE;"
    )
    rows = query(sql)
    if isinstance(rows, list) and rows and isinstance(rows[0], list):
        rows = rows[0]
    return {str(row["refno"]): row for row in rows or []}


def format_span(values: Sequence[float] | None) -> str:
    if values is None:
        return "       -"
    return ",".join(f"{axis:8.1f}" for axis in values)


def compare_pairs(
    pairs: Sequence[Pair],
    snapshot_members: Sequence[dict[str, Any]] | None,
    gen_map: dict[str, dict[str, Any]],
    abs_tol: float = ABS_TOL_MM,
    rel_tol: float = REL_TOL,
) -> list[dict[str, Any]]:
    results = []
    for pair in pairs:
        rvm_member = find_rvm_member(snapshot_members or [], pair.rvm) if snapshot_members else None
        rvm_span = span(rvm_member.get("aabb_world_mm") if rvm_member else None)
        gen_row = gen_map.get(pair.gen)
        gen_span = span(gen_box(gen_row.get("mins") if gen_row else None, gen_row.get("maxs") if gen_row else None))
        if rvm_span is None or gen_span is None:
            verdict, excess = "n/a", 0.0
        else:
            verdict, excess = compare_spans(rvm_span, gen_span, abs_tol, rel_tol)
        results.append(
            {
                "rvm": pair.rvm,
                "gen": pair.gen,
                "noun": pair.noun,
                "rvm_span": rvm_span,
                "gen_span": gen_span,
                "verdict": verdict,
                "excess_mm": excess,
            }
        )
    return results


def print_report(fixture: Fixture, rows: Sequence[dict[str, Any]], snapshot_ok: bool) -> int:
    print(f"fixture {fixture.key}  dbnum={fixture.dbnum}  root={fixture.root_name}  pe={fixture.root_refno}")
    print(f"snapshot {'OK ' + str(fixture.snapshot.relative_to(REPO)) if snapshot_ok else 'MISSING ' + str(fixture.snapshot)}")
    print(f"{'member':<12} {'E3D (dx,dy,dz)':<32} {'gen (dx,dy,dz)':<32} verdict")
    print("-" * 92)
    fails = 0
    missing = 0
    for row in rows:
        if row["verdict"] == "FAIL":
            fails += 1
        elif row["verdict"] == "n/a":
            missing += 1
        print(
            f"{row['rvm']:<12} {format_span(row['rvm_span']):<32} "
            f"{format_span(row['gen_span']):<32} {row['verdict']}"
        )
    print()
    print(f"FAIL: {fails} / {len(rows)}   n/a: {missing}")
    if not snapshot_ok:
        print(
            "缺 RVM 快照：在 E3D 跑对应 rvm_export_*.mac（narrow，insu/obst off），"
            "再 rvm_verify import --scope narrow。"
        )
        return 2
    return 1 if fails else 0


def connect_db():
    import aios_db

    aios_db.set_config(str(REPO / "DbOption"))
    aios_db.connect(cwd=str(REPO))
    return aios_db


def resolve_pairs(aios_db, fixture: Fixture) -> list[Pair]:
    if fixture.pairs is not None:
        return list(fixture.pairs)
    children = aios_db.db.members(fixture.root_refno)
    if not isinstance(children, list):
        children = []
    nouns = fixture.nouns or SWEEP_NOUNS
    pairs = pairs_from_children(children, nouns)
    if not pairs:
        raise SystemExit(f"{fixture.root_name} 下没有 {nouns} 成员")
    return pairs


def run_fixture(
    fixture: Fixture,
    abs_tol: float = ABS_TOL_MM,
    rel_tol: float = REL_TOL,
    require_snapshot: bool = False,
) -> int:
    aios_db = connect_db()
    hits = aios_db.db.by_name(fixture.root_name, dbnum=fixture.dbnum)
    if fixture.root_refno not in hits:
        raise SystemExit(
            f"根元素对不上：by_name({fixture.root_name!r}, {fixture.dbnum})={hits} "
            f"期望含 {fixture.root_refno}"
        )
    pairs = resolve_pairs(aios_db, fixture)
    gen_map = fetch_gen_aabbs(aios_db.db.query, [pair.gen for pair in pairs])
    snapshot_members = None
    snapshot_ok = fixture.snapshot.is_file()
    if snapshot_ok:
        snapshot_members = load_snapshot(fixture.snapshot).get("members") or []
    elif require_snapshot:
        print(f"缺快照 {fixture.snapshot}（--require-snapshot）")
        return 2
    rows = compare_pairs(pairs, snapshot_members, gen_map, abs_tol, rel_tol)
    return print_report(fixture, rows, snapshot_ok)


def list_sweep_1112(limit: int) -> int:
    aios_db = connect_db()
    sql = (
        "SELECT noun, count() AS n FROM pe "
        "WHERE dbnum = 1112 AND deleted = false "
        "AND noun IN ['WALL', 'STWALL', 'SCTN', 'GENSEC', 'GWALL', 'PANE', 'FLOOR'] "
        "GROUP BY noun;"
    )
    print("== dbnum 1112 几何 noun 计数 ==")
    print(json.dumps(aios_db.db.query(sql), ensure_ascii=False, indent=2))
    named = aios_db.db.query(
        "SELECT record::id(id) AS refno, name FROM pe "
        "WHERE dbnum = 1112 AND noun = 'CWALL' AND name != NONE AND name != '' "
        "AND deleted = false LIMIT $limit;",
        {"limit": limit},
    )[0]
    print(f"\n== 带名 CWALL（前 {limit}）==")
    for row in named:
        print(f"  {row['name']}  {row['refno']}")
    return 0


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--fixture",
        default="1rs-wf03-w-c-rr001",
        choices=sorted(FIXTURES),
        help="默认 1112 CWALL /1RS-WF03-W-C-RR001（SweepSolid WALL/STWALL）",
    )
    parser.add_argument("--abs-tol", type=float, default=ABS_TOL_MM)
    parser.add_argument("--rel-tol", type=float, default=REL_TOL)
    parser.add_argument(
        "--require-snapshot",
        action="store_true",
        help="没有 .rvm.json 时退出 2，而不是只打印生成侧",
    )
    parser.add_argument(
        "--list-1112",
        action="store_true",
        help="只列出 1112 的 SweepSolid/挤出 noun 与带名 CWALL，不对拍",
    )
    parser.add_argument("--list-limit", type=int, default=20)
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    if args.list_1112:
        return list_sweep_1112(args.list_limit)
    return run_fixture(
        FIXTURES[args.fixture],
        abs_tol=args.abs_tol,
        rel_tol=args.rel_tol,
        require_snapshot=args.require_snapshot,
    )


if __name__ == "__main__":
    raise SystemExit(main())
