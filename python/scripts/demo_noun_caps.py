# -*- coding: utf-8 -*-
"""示范 1：noun 能力矩阵（替代手写的 gm_noun_caps_probe.py）。

原探针在纯 Python 里复刻了 dabacon 页级解析 + ATTOPE/ATGTIX/ATNLOG 读取链
（约 250 行）；现在一次 `parse.noun_dict` 调用拿到与 Rust 生产实现同源的结果。

用法：.venv\\Scripts\\python.exe scripts\\demo_noun_caps.py [attlib.dat 路径] [输出.json]
"""

import json
import sys
from collections import Counter
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

REPO_ROOT = Path(__file__).resolve().parents[2]

import aios_db

aios_db.set_config(str(REPO_ROOT / "DbOption"))

attlib = sys.argv[1] if len(sys.argv) > 1 else r"D:\AVEVA\Everything3D3.1\attlib.dat"
out_json = Path(sys.argv[2] if len(sys.argv) > 2 else ".scratch/noun_caps.json")

dic = aios_db.parse.noun_dict(attlib)
rows = dic["nouns"]
print(f"nouns={dic['noun_count']} fields={dic['field_count']}")

out_json.parent.mkdir(parents=True, exist_ok=True)
out_json.write_text(json.dumps(rows, ensure_ascii=False, indent=1), encoding="utf-8")
print(f"wrote {out_json} rows={len(rows)}")

# 与原探针相同口径的汇总
bool_fields = [k for k, v in rows[0].items() if isinstance(v, bool)]
print("\n== capability counts over all nouns ==")
for name in bool_fields:
    print("%-24s true=%d" % (name, sum(1 for r in rows if r[name])))

geom = [r for r in rows if r["primitive"] or r["geomset"] or r["extrusion"]]
print(f"\nprimitive|geomset|extrusion union = {len(geom)}")
print(f"  of which named = {len([r for r in geom if r['noun_name']])}")

for key in ("graphics_behaviour",):
    counter = Counter(r.get(key) for r in rows)
    top = ", ".join(
        f"{value}:{count}"
        for value, count in sorted(counter.items(), key=lambda kv: -kv[1])[:8]
    )
    print("%-24s distinct=%d top=%s" % (key, len(counter), top))

points = sorted(r["noun_name"] for r in rows if r.get("point") and r["noun_name"])
print(f"\npoint==true nouns ({len(points)}): {', '.join(points[:80])}")
