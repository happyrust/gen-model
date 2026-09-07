#!/usr/bin/env python3
"""把 `toplevel` 位补进 noun 粒度快照（schema 2 → 3）。

**为什么单独一个脚本、而不是并进 `dump_core_primary_list.py`**：那三个位
（`significant` / `primitive_a` / `primitive_b`）是 P0 从 **live E3D 进程**的
`core.dll!DB_Noun::getField(id, &out)` 导的；`toplevel` 不需要——它是**离线字典字段**
（`old-parse-pdms-db::dict::FIELD_TOPLEVEL = 661628`，`DB_Noun::toplevel` core.dll `0x58DBA70`），
`vendor/old-parse-pdms-db/noun_caps.json` 里现成就有。两个渠道不同，**快照里必须分得开**，
所以这一块自带 `provenance` 与自己的来源指纹，不冒充 live 取值。

为什么要它：Core3D 的 3D 绘制侧（`DES_DrawList` 的 TOPF 家族）用的是 `toplevel`，
**一次都不读 `significant`**——后者 8 个读点里 7 个在 `PartialUpdateDesiMgr`（DRAFT 图纸侧）。
取证见 `docs/evidence/2026-09-04-ida-core3d-partial-update-recheck.md` §4.1。

用法：
    python scripts/e3d/add_toplevel_to_granularity_snapshot.py            # 就地改快照
    python scripts/e3d/add_toplevel_to_granularity_snapshot.py --check    # 只校验，不写
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
DEFAULT_SNAPSHOT = REPO / "tests" / "fixtures" / "core-noun-granularity-e3d31.json"
DEFAULT_CAPS_CANDIDATES = [
    REPO.parent / "vendor" / "old-parse-pdms-db" / "noun_caps.json",
    Path.home()
    / ".cargo/git/checkouts/old-parse-pdms-db-03ff0ef956353e60/2de7cd2/noun_caps.json",
]

FIELD_TOPLEVEL = 661628


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def resolve_caps(explicit: Path | None) -> Path:
    if explicit is not None:
        if not explicit.exists():
            sys.exit(f"找不到 {explicit}")
        return explicit
    for candidate in DEFAULT_CAPS_CANDIDATES:
        if candidate.exists():
            return candidate
    sys.exit("找不到 noun_caps.json，用 --caps 指一个")


def build_toplevel_block(snapshot: dict, caps_path: Path) -> dict:
    caps = json.loads(caps_path.read_text(encoding="utf-8"))
    values = {
        row["noun_name"].strip().upper(): bool(row.get("toplevel"))
        for row in caps
        if row.get("noun_name", "").strip()
    }

    # 顺序跟着已有的块走，两块之间逐行 diff 才有意义。
    order = list(snapshot["fields"]["significant"]["nouns"].keys())
    missing = [name for name in order if name.strip().upper() not in values]
    extra = sorted(set(values) - {name.strip().upper() for name in order})
    if missing or extra:
        sys.exit(
            "noun 集合对不上，拒绝写入："
            f"快照有而 caps 没有 {len(missing)} 个 {missing[:10]}；"
            f"caps 有而快照没有 {len(extra)} 个 {extra[:10]}"
        )

    nouns = {name: values[name.strip().upper()] for name in order}
    true_count = sum(1 for value in nouns.values() if value)
    return {
        "source": f"old-parse-pdms-db/noun_caps.json（离线 attlib.dat 字典解析，字段 {FIELD_TOPLEVEL}）",
        "provenance": "offline-dict",
        "provenance_note": (
            "与 significant / primitive_a / primitive_b 不同渠道：那三个是 live "
            "`core.dll!DB_Noun::getField(id, &out)` 导的，这一个是离线字典解析。"
            "尚未与 live getField 逐值对过（primitive 当年对过 347 个、零差异）。"
        ),
        "field_id": FIELD_TOPLEVEL,
        # 其余三块的 field_type 是 core 自己 `DB_Noun::fieldType` 的裁决（0 = bool 重载）。
        # 这一块没走那条通道，不冒充测过：留 null，把已知的旁证写在下面一行。
        "field_type": None,
        "field_type_note": (
            "未经 `DB_Noun::fieldType` 实测。旁证：attlib 字段元数据表记 661628 的类型码为 1"
            "（= bool），且 `old-parse-pdms-db::dict` 按 bool 读它。"
        ),
        "count": len(nouns),
        "resolved_count": len(nouns),
        "unknown_count": 0,
        "not_found_count": 0,
        "true_count": true_count,
        "false_count": len(nouns) - true_count,
        "non_binary_values": [],
        "unknown": [],
        "nouns": nouns,
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--snapshot", type=Path, default=DEFAULT_SNAPSHOT)
    ap.add_argument("--caps", type=Path, default=None)
    ap.add_argument("--check", action="store_true", help="只校验，不写回")
    args = ap.parse_args()

    caps_path = resolve_caps(args.caps)
    snapshot = json.loads(args.snapshot.read_text(encoding="utf-8"))
    if snapshot.get("schema") not in (2, 3):
        sys.exit(f"{args.snapshot} schema={snapshot.get('schema')}，本脚本只认 2 或 3")

    block = build_toplevel_block(snapshot, caps_path)

    if args.check:
        existing = snapshot.get("fields", {}).get("toplevel")
        if existing is None:
            print("快照里还没有 toplevel 块")
            return 1
        same = existing.get("nouns") == block["nouns"]
        print(f"toplevel 块存在，逐值一致: {same}，true_count={existing.get('true_count')}")
        return 0 if same else 1

    snapshot["schema"] = 3
    snapshot["toplevel_source"] = caps_path.name
    snapshot["toplevel_source_bytes"] = caps_path.stat().st_size
    snapshot["toplevel_source_sha256"] = sha256(caps_path)
    snapshot["fields"]["toplevel"] = block

    text = json.dumps(snapshot, ensure_ascii=False, indent=2) + "\n"
    # 快照原本就是 CRLF，改行尾会让 `include_str!` 之外的差异淹掉真正的改动。
    args.snapshot.write_bytes(text.replace("\n", "\r\n").encode("utf-8"))

    print(f"写入 {args.snapshot}")
    print(f"  来源      : {caps_path}")
    print(f"  sha256    : {snapshot['toplevel_source_sha256']}")
    print(f"  toplevel  : {block['true_count']} / {block['count']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
