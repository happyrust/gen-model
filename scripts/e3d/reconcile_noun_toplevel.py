#!/usr/bin/env python3
"""对账：我方的生成根 vs core 3D 绘制侧的 `toplevel`（TOPF）位。

与 `reconcile_noun_granularity.py`（P1，对的是 `significant`）同一套办法、同一种四类清单，
换一个位。**为什么要单独对一次**：`significant` 是 DRAFT 局部更新
（`PartialUpdateDesiMgr`）的块判据；3D 绘制侧（`DES_DrawList` 的 TOPF 家族）用的是
`toplevel`，且**一次都不读 `significant`**（`Core3D.dll` 里后者 8 个读点，7 个在那个类里）。
我们的产物是 3D 模型，却在用前者选根。取证见
`docs/evidence/2026-09-04-ida-core3d-partial-update-recheck.md` §4.1。

我方今天有**两条**选根判据，都在生产路上，所以两条都对：

- **A · 子树枚举** `generation_root::enumerate_generation_roots_in_subtree`
  （direct 按需生成与增量规划器共用的唯一一段遍历）：
  `delivery ∨ (¬inside_delivery ∧ ¬coarse ∧ noun_is_significant)`。
- **B · 元素上卷** `generation_root::resolve_element_generation_root`：
  一个 noun 永远当不了根，当且仅当它是 coarse / point-loop 容器 / NON_DELIVERY_UNIT_NOUNS；
  其余在某个层级位置上都会被当 Normal 根返回。这条就是 P1 脚本用的口径。

纯离线，不需要 live E3D 进程。

用法：
    python scripts/e3d/reconcile_noun_toplevel.py [--caps <noun_caps.json>] [--json <out>]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
DEFAULT_CORE = REPO / "tests" / "fixtures" / "core-noun-granularity-e3d31.json"
DEFAULT_CAPS_CANDIDATES = [
    REPO.parent / "vendor" / "old-parse-pdms-db" / "noun_caps.json",
    Path.home()
    / ".cargo/git/checkouts/old-parse-pdms-db-03ff0ef956353e60/2de7cd2/noun_caps.json",
]

# ── 我方名单，与 generation_root.rs / model_impact.rs 逐字同步 ────────────────
DEFAULT_DELIVERY_UNIT_TYPES = ["BRAN", "HANG", "SUPPO", "EQUI"]
COARSE_HIERARCHY_NOUNS = ["WORL", "WORLD", "SITE", "ZONE"]
NON_DELIVERY_UNIT_NOUNS = ["FTUB"]
EXTRA_LOOP_CONTAINER_NOUNS = ["JLDATU", "PLDATU", "ENDATU"]


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def field_true_set(doc: dict, field: str) -> set[str]:
    nouns = doc["fields"][field]["nouns"]
    return {name.strip().upper() for name, value in nouns.items() if value is True}


def bullet(names, per_line: int = 8) -> str:
    ordered = sorted(names)
    lines = [" ".join(ordered[i : i + per_line]) for i in range(0, len(ordered), per_line)]
    return "\n".join("    " + line for line in lines) if lines else "    （空）"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--core", type=Path, default=DEFAULT_CORE)
    ap.add_argument("--caps", type=Path, default=None)
    ap.add_argument("--json", type=Path, default=None)
    args = ap.parse_args()

    caps_path = args.caps
    if caps_path is None:
        for candidate in DEFAULT_CAPS_CANDIDATES:
            if candidate.exists():
                caps_path = candidate
                break
    if caps_path is None or not caps_path.exists():
        sys.exit("找不到 noun_caps.json，用 --caps 指一个")

    core = json.loads(args.core.read_text(encoding="utf-8"))
    if core.get("schema") != 3:
        sys.exit(
            f"{args.core} schema={core.get('schema')}，本脚本要 schema 3（带 toplevel 块）；"
            "先跑 scripts/e3d/add_toplevel_to_granularity_snapshot.py"
        )
    caps = {
        row["noun_name"].strip().upper(): row
        for row in json.loads(caps_path.read_text(encoding="utf-8"))
        if row.get("noun_name", "").strip()
    }

    universe = {name.strip().upper() for name in core["fields"]["significant"]["nouns"]}
    significant = field_true_set(core, "significant")
    topf = field_true_set(core, "toplevel")
    primitive = field_true_set(core, "primitive_a") | field_true_set(core, "primitive_b")

    coarse = {n.upper() for n in COARSE_HIERARCHY_NOUNS}
    component_only = {n.upper() for n in NON_DELIVERY_UNIT_NOUNS}
    point_nouns = {name for name, row in caps.items() if row.get("point") is True}
    loop_containers = point_nouns | {n.upper() for n in EXTRA_LOOP_CONTAINER_NOUNS}
    mdu = {n.upper() for n in DEFAULT_DELIVERY_UNIT_TYPES} - component_only

    # A · 子树枚举：交付单元，或（非层级容器 ∧ significant）。没有 point/loop 这道筛。
    roots_enum = mdu | (significant - coarse)
    # B · 元素上卷：只有 coarse / point-loop / component-only 永远当不了根。
    roots_resolve = (universe - (coarse | component_only | loop_containers)) | mdu

    def four_way(ours: set[str]) -> dict:
        return {
            "agree": sorted(topf & ours),
            "over": sorted(ours - topf),
            "under": sorted(topf - ours),
            "unknown": sorted(
                row["noun"].strip().upper()
                for row in core["fields"]["toplevel"].get("unknown", [])
            ),
        }

    enum_cmp = four_way(roots_enum)
    resolve_cmp = four_way(roots_resolve)

    def why_not_a_root(noun: str) -> str:
        if noun in mdu:
            return "是 MDU（不该出现在这里）"
        if noun in coarse:
            return "coarse 层级容器"
        if not significant >= {noun}:
            return "core 判不 significant"
        return "?"

    def why_not_b_root(noun: str) -> str:
        if noun in coarse:
            return "coarse 层级容器"
        if noun in component_only:
            return "NON_DELIVERY_UNIT_NOUNS"
        if noun in loop_containers:
            return "point / loop 容器"
        return "?"

    under_a = enum_cmp["under"]
    out = {
        "core_snapshot": str(args.core),
        "core_sha256": core.get("core_sha256"),
        "toplevel_source": core.get("toplevel_source"),
        "toplevel_source_sha256": core.get("toplevel_source_sha256"),
        "caps_source": str(caps_path),
        "caps_sha256": sha256(caps_path),
        "counts": {
            "universe": len(universe),
            "core_toplevel": len(topf),
            "core_significant": len(significant),
            "core_primitive_union": len(primitive),
            "toplevel_and_significant": len(topf & significant),
            "toplevel_only": len(topf - significant),
            "significant_only": len(significant - topf),
            "our_roots_enumeration": len(roots_enum),
            "our_roots_resolution": len(roots_resolve),
        },
        "A_enumeration_vs_toplevel": {
            "agree_count": len(enum_cmp["agree"]),
            "over_count": len(enum_cmp["over"]),
            "over": enum_cmp["over"],
            "under_count": len(under_a),
            "under": under_a,
            "unknown": enum_cmp["unknown"],
            "under_detail": {
                n: {
                    "reason": why_not_a_root(n),
                    "core_primitive": n in primitive,
                    "core_significant": n in significant,
                    "graphics_behaviour": caps.get(n, {}).get("graphics_behaviour"),
                    "dict_primitive": caps.get(n, {}).get("primitive"),
                }
                for n in under_a
            },
        },
        "B_resolution_vs_toplevel": {
            "agree_count": len(resolve_cmp["agree"]),
            "over_count": len(resolve_cmp["over"]),
            "under_count": len(resolve_cmp["under"]),
            "under": resolve_cmp["under"],
            "unknown": resolve_cmp["unknown"],
            "under_detail": {
                n: {
                    "reason": why_not_b_root(n),
                    "core_primitive": n in primitive,
                    "core_significant": n in significant,
                }
                for n in resolve_cmp["under"]
            },
        },
        "cross": {
            "toplevel_only_and_primitive": sorted((topf - significant) & primitive),
            "significant_only_gb2": sorted(
                n
                for n in (significant - topf)
                if caps.get(n, {}).get("graphics_behaviour") == 2
            ),
            "mdu_toplevel": {n: (n in topf) for n in sorted(mdu)},
            "mdu_significant": {n: (n in significant) for n in sorted(mdu)},
        },
    }

    print(f"core 快照        : {args.core}  (schema {core['schema']})")
    print(f"  core_sha256    : {core.get('core_sha256')}")
    print(f"  toplevel 来源  : {core.get('toplevel_source')} / {core.get('toplevel_source_sha256')}")
    print(f"caps 源          : {caps_path}")
    print()
    print("── 计数 ─────────────────────────────────────────")
    for key, value in out["counts"].items():
        print(f"  {key:26} {value}")
    print()
    print("── A · 子树枚举（enumerate_generation_roots_in_subtree）vs TOPF ──")
    print(f"  一致            : {out['A_enumeration_vs_toplevel']['agree_count']}")
    print(f"  我们多算的      : {out['A_enumeration_vs_toplevel']['over_count']}")
    print(bullet(enum_cmp["over"]))
    print(f"  我们少算的      : {out['A_enumeration_vs_toplevel']['under_count']}  ← 真缺口在这一格")
    print(bullet(under_a))
    print(f"  core unknown    : {len(enum_cmp['unknown'])}")
    print()
    reasons: dict[str, int] = {}
    prim_under = 0
    for detail in out["A_enumeration_vs_toplevel"]["under_detail"].values():
        reasons[detail["reason"]] = reasons.get(detail["reason"], 0) + 1
        prim_under += 1 if detail["core_primitive"] else 0
    print("  少算的按原因分：")
    for reason, count in sorted(reasons.items(), key=lambda kv: -kv[1]):
        print(f"    {reason:24} {count}")
    print(f"    其中 core 也判 primitive 的: {prim_under}（core 会把它们上卷到 SignificantOwner）")
    print()
    print("── B · 元素上卷（resolve_element_generation_root）vs TOPF ──")
    print(f"  一致            : {out['B_resolution_vs_toplevel']['agree_count']}")
    print(f"  我们多算的      : {out['B_resolution_vs_toplevel']['over_count']}")
    print(f"  我们少算的      : {out['B_resolution_vs_toplevel']['under_count']}")
    print(bullet(resolve_cmp["under"]))
    for noun, detail in out["B_resolution_vs_toplevel"]["under_detail"].items():
        print(
            f"    {noun:8} 因 {detail['reason']:22} "
            f"prim={detail['core_primitive']} sig={detail['core_significant']}"
        )
    print()
    print("── 交叉 ─────────────────────────────────────────")
    print(f"  只 toplevel 且是 primitive: {len(out['cross']['toplevel_only_and_primitive'])}")
    print(f"  只 significant 且 gb==2   : {out['cross']['significant_only_gb2']}")
    print(f"  MDU toplevel              : {out['cross']['mdu_toplevel']}")
    print(f"  MDU significant           : {out['cross']['mdu_significant']}")

    if args.json:
        args.json.write_text(json.dumps(out, ensure_ascii=False, indent=2), encoding="utf-8")
        print(f"\n已写出 {args.json}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
