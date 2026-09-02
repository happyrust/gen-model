#!/usr/bin/env python3
"""P1 对账：core 的 noun 粒度位 vs 我方的生成根 / primitive 名单。

纯离线：读 `tests/fixtures/core-noun-granularity-e3d31.json`（P0 产物）与
parse_pdms_db 内嵌的 `noun_caps.json` / `noun_flags.json`（我方 dict 判据的数据源），
输出四类差异。不需要 live E3D 进程，也不链接 gen-model。

我方生成根规则复刻自 `src/data_interface/generation_root.rs::resolve_element_generation_root`
的 noun 层面部分：一个 noun 永远不会作为生成根，当且仅当它是 COARSE_HIERARCHY_NOUNS、
loop/point 容器（`is_loop_container_noun`）、或 NON_DELIVERY_UNIT_NOUNS。
其余 noun 在某个层级位置上都会被返回为 Normal 根。

用法：
    python scripts/e3d/reconcile_noun_granularity.py [--caps <noun_caps.json>] [--json <out>]
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
    Path.home()
    / ".cargo/git/checkouts/old-parse-pdms-db-03ff0ef956353e60/2de7cd2/noun_caps.json",
    REPO.parent / "vendor" / "old-parse-pdms-db" / "noun_caps.json",
]

# ── 我方名单，与 generation_root.rs / model_impact.rs 逐字同步 ────────────────
DEFAULT_DELIVERY_UNIT_TYPES = ["BRAN", "HANG", "SUPPO", "EQUI"]
COARSE_HIERARCHY_NOUNS = ["WORL", "WORLD", "SITE", "ZONE"]
NON_DELIVERY_UNIT_NOUNS = ["FTUB"]
# `is_loop_container_noun` 在 dict 的 point 位之外硬编码的三个。
EXTRA_LOOP_CONTAINER_NOUNS = ["JLDATU", "PLDATU", "ENDATU"]


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_core(path: Path) -> dict:
    doc = json.loads(path.read_text(encoding="utf-8"))
    if doc.get("schema") != 2:
        sys.exit(f"{path} schema={doc.get('schema')}，本脚本只认 schema 2")
    return doc


def field_true_set(doc: dict, field: str) -> set[str]:
    nouns = doc["fields"][field]["nouns"]
    return {name.strip().upper() for name, value in nouns.items() if value is True}


def all_nouns(doc: dict) -> set[str]:
    return {name.strip().upper() for name in doc["fields"]["significant"]["nouns"]}


def load_caps(path: Path) -> dict[str, dict]:
    caps = json.loads(path.read_text(encoding="utf-8"))
    return {c["noun_name"].strip().upper(): c for c in caps if c.get("noun_name", "").strip()}


def bullet(names: set[str], per_line: int = 10) -> str:
    ordered = sorted(names)
    lines = [
        " ".join(ordered[i : i + per_line]) for i in range(0, len(ordered), per_line)
    ]
    return "\n".join(lines) if lines else "（空）"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--core", type=Path, default=DEFAULT_CORE)
    ap.add_argument("--caps", type=Path, default=None)
    ap.add_argument("--json", type=Path, default=None, help="把结果另存一份机器可读的")
    args = ap.parse_args()

    caps_path = args.caps
    if caps_path is None:
        for candidate in DEFAULT_CAPS_CANDIDATES:
            if candidate.exists():
                caps_path = candidate
                break
    if caps_path is None or not caps_path.exists():
        sys.exit("找不到 noun_caps.json，用 --caps 指一个")

    core = load_core(args.core)
    caps = load_caps(caps_path)

    universe = all_nouns(core)
    significant = field_true_set(core, "significant")
    prim_a = field_true_set(core, "primitive_a")
    prim_b = field_true_set(core, "primitive_b")
    core_primitive = prim_a | prim_b

    # ── 我方：哪些 noun 永远不会成为生成根 ──────────────────────────────────
    coarse = {n.upper() for n in COARSE_HIERARCHY_NOUNS}
    component_only = {n.upper() for n in NON_DELIVERY_UNIT_NOUNS}
    point_nouns = {name for name, c in caps.items() if c.get("point") is True}
    loop_containers = point_nouns | {n.upper() for n in EXTRA_LOOP_CONTAINER_NOUNS}
    never_root = coarse | component_only | loop_containers
    mdu = {n.upper() for n in DEFAULT_DELIVERY_UNIT_TYPES}
    our_roots = (universe - never_root) | mdu

    # ── 我方 primitive 名单 = dict 的 FIELD_PRIMITIVE(659518) 位 ────────────
    our_primitive = {name for name, c in caps.items() if c.get("primitive") is True}
    # 我方「几何 noun」的实际口径（`all_dictionary_geometry_nouns_follow_the_same_update_contract`）
    our_geometry = our_primitive | {
        name
        for name, c in caps.items()
        if c.get("geomset") is True or c.get("extrusion") is True
    }

    # ── 四类差异（significant）────────────────────────────────────────────
    over = our_roots - significant  # 我们多算的：core 说不显著，我们当根
    under = significant - our_roots  # 我们少算的：core 说显著，我们永不当根
    agree = significant & our_roots
    unknown = {
        name.strip().upper()
        for name in core["fields"]["significant"].get("unknown", [])
    }

    # ── primitive 对账 ────────────────────────────────────────────────────
    prim_over = our_primitive - core_primitive
    prim_under = core_primitive - our_primitive
    a_vs_dict_only_core = prim_a - our_primitive
    a_vs_dict_only_ours = our_primitive - prim_a

    neither = universe - significant - core_primitive

    # 「多算的」按 core 对它们的实际处置再分两格：core 上卷 vs core 完全丢弃。
    over_core_rolls_up = over & core_primitive - significant
    over_core_discards = over & neither

    # 「core 完全丢弃、我方却当根」这 1495 个里，真正可能长出几何的才是实际代价。
    # 用 dict 自己的几何能力做代理：geomset / extrusion / 非空 graphics_behaviour。
    def has_geometry(name: str) -> bool:
        c = caps.get(name)
        if not c:
            return False
        gb = c.get("graphics_behaviour")
        return bool(c.get("geomset") or c.get("extrusion") or (gb not in (None, 0)))

    discards_with_geometry = {n for n in over_core_discards if has_geometry(n)}
    discards_inert = over_core_discards - discards_with_geometry

    out = {
        "core_snapshot": str(args.core),
        "core_sha256": core.get("core_sha256"),
        "caps_source": str(caps_path),
        "caps_sha256": sha256(caps_path),
        "counts": {
            "universe": len(universe),
            "core_significant": len(significant),
            "core_primitive_a": len(prim_a),
            "core_primitive_b": len(prim_b),
            "core_primitive_union": len(core_primitive),
            "core_neither": len(neither),
            "caps_rows": len(caps),
            "our_point_nouns": len(point_nouns),
            "our_never_root": len(never_root),
            "our_roots": len(our_roots),
            "our_primitive": len(our_primitive),
        },
        "significant": {
            "list": sorted(significant),
            "over_count": len(over),
            "over_core_rolls_up_count": len(over_core_rolls_up),
            "over_core_discards_count": len(over_core_discards),
            "over_core_discards_with_geometry": sorted(discards_with_geometry),
            "over_core_discards_inert_count": len(discards_inert),
            "under": sorted(under),
            "unknown": sorted(unknown),
            "agree_count": len(agree),
            "mdu_not_significant": sorted(mdu - significant),
            "significant_and_primitive": sorted(significant & core_primitive),
        },
        "primitive": {
            "over": sorted(prim_over),
            "under": sorted(prim_under),
            "primitive_a_not_in_dict": sorted(a_vs_dict_only_core),
            "dict_not_in_primitive_a": sorted(a_vs_dict_only_ours),
            "geometry_union_count": len(our_geometry),
            "core_primitive_not_in_geometry_union": sorted(core_primitive - our_geometry),
            "geometry_union_not_core_primitive_count": len(our_geometry - core_primitive),
        },
        "under_detail": {
            n: {
                "reason": (
                    "coarse"
                    if n in coarse
                    else "component_only"
                    if n in component_only
                    else "point/loop container"
                    if n in loop_containers
                    else "?"
                ),
                "core_primitive_a": n in prim_a,
                "core_primitive_b": n in prim_b,
                "dict_primitive": n in our_primitive,
            }
            for n in sorted(under)
        },
    }

    print(f"core 快照      : {args.core}")
    print(f"  core_sha256  : {core.get('core_sha256')}")
    print(f"caps 源        : {caps_path}")
    print(f"  sha256       : {out['caps_sha256']}")
    print()
    print("── 计数 ─────────────────────────────────────────")
    for k, v in out["counts"].items():
        print(f"  {k:22} {v}")
    print()
    print("── significant 四类 ─────────────────────────────")
    print(f"  一致（core 显著 ∩ 我方可当根）: {len(agree)}")
    print(f"  我们多算的（core 不显著、我方当根）: {len(over)}")
    print(f"    其中 core 会上卷到 SignificantOwner 的（primitive 非 significant）: "
          f"{len(over_core_rolls_up)}")
    print(f"    其中 core 完全丢弃的（既非 significant 又非 primitive）: "
          f"{len(over_core_discards)}")
    print(f"      其中 dict 认为有几何能力的（真代价）: {len(discards_with_geometry)}")
    print(bullet(discards_with_geometry))
    print(f"      其中 dict 看不出任何几何能力的（多算但大概率空转）: "
          f"{len(discards_inert)}")
    print(f"  MDU 四个里 core 判不显著的: {sorted(mdu - significant)}")
    print(f"  既 significant 又 primitive: {len(significant & core_primitive)}")
    print()
    print("── core significant 全表（127）──────────────────")
    print(bullet(significant))
    print()
    print(f"  我们少算的（core 显著、我方永不当根）: {len(under)}")
    print(bullet(under))
    for n, d in out["under_detail"].items():
        print(
            f"    {n:8} 因 {d['reason']:22} "
            f"prim_a={d['core_primitive_a']} prim_b={d['core_primitive_b']} "
            f"dict_prim={d['dict_primitive']}"
        )
    print(f"  core unknown: {len(unknown)}")
    print()
    print("── primitive 对账 ───────────────────────────────")
    print(f"  core primitive_a 与 dict FIELD_PRIMITIVE(659518) 一致? "
          f"{not a_vs_dict_only_core and not a_vs_dict_only_ours}")
    if a_vs_dict_only_core:
        print(f"    只在 core primitive_a: {bullet(a_vs_dict_only_core)}")
    if a_vs_dict_only_ours:
        print(f"    只在 dict primitive : {bullet(a_vs_dict_only_ours)}")
    print(f"  我方缺的（core primitive、dict 不认）: {len(prim_under)}")
    print(bullet(prim_under))
    print(f"  我方多的（dict 认、core 不认）: {len(prim_over)}")
    print(bullet(prim_over))
    print()
    print(f"  换成我方实际口径 primitive∪geomset∪extrusion（{len(our_geometry)}）再比：")
    print(f"    core primitive 里它仍然没有的: "
          f"{len(core_primitive - our_geometry)}")
    print(bullet(core_primitive - our_geometry))
    print(f"    它有而 core 不判 primitive 的: {len(our_geometry - core_primitive)}")

    if args.json:
        args.json.write_text(
            json.dumps(out, ensure_ascii=False, indent=2), encoding="utf-8"
        )
        print(f"\n已写出 {args.json}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
