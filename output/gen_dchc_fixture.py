# -*- coding: utf-8 -*-
"""Distill the per-attribute DCHC (design change code, field 596407) map out of
the 4.7 MB dictionary export into a small committed fixture, and recompute the
dead-entry cleanup lists for the curated tables.

Inputs : output/noun_attr_fields.json  (ADR-008 NounLayoutExport, not committed)
         all_attr_info.json            (runtime schema)
         src/data_interface/model_impact.rs (curated tables)
Output : src/data_interface/dchc_change_classes.json (committed fixture)
"""
import json
import re
import sys

sys.stdout.reconfigure(encoding="utf-8")

# 1. DCHC map from the dictionary export
dchc = {}
raw = json.load(open("output/noun_attr_fields.json", encoding="utf-8"))
for rec in raw.values():
    name = (rec.get("name") or "").strip().upper()
    code = rec.get("f", {}).get("DCHC", {}).get("i")
    if name and code is not None:
        dchc[name] = int(code)

nonzero = {k: v for k, v in dchc.items() if v != 0}
print(f"dictionary: {len(dchc)} attrs with DCHC, {len(nonzero)} non-zero")
from collections import Counter
print("code histogram:", dict(sorted(Counter(dchc.values()).items())))

with open("src/data_interface/dchc_change_classes.json", "w", encoding="utf-8", newline="\n") as f:
    json.dump(dict(sorted(dchc.items())), f, ensure_ascii=False, indent=0, separators=(",", ":"))
    f.write("\n")
print("fixture written: src/data_interface/dchc_change_classes.json")

# 2. dead-entry recomputation
src = open("src/data_interface/model_impact.rs", encoding="utf-8").read()


def table(name: str):
    m = re.search(r"pub const " + name + r": &\[&str\] = &\[(.*?)\];", src, re.S)
    body = re.sub(r"//[^\n]*", "", m.group(1))
    return [n for n in re.findall(r'"([A-Z0-9:]+)"', body)]


schema = set()
data = json.load(open("all_attr_info.json", encoding="utf-8"))["named_attr_info_map"]
nouns = set(n.strip().upper() for n in data)
for attrs in data.values():
    for rec in attrs.values():
        n = (rec.get("name") or "").strip().upper()
        if n:
            schema.add(n)
print(f"schema: {len(schema)} attr names, {len(nouns)} nouns")

for tname in ["DEPENDENCY_CASCADE_ATTR_NAMES", "DIRECT_GEOMETRY_ATTR_NAMES"]:
    entries = table(tname)
    unmatched = [n for n in entries if n not in schema]
    dead = sorted(n for n in unmatched if n not in dchc and n in nouns)
    keep = sorted(n for n in unmatched if n not in dead)
    print(f"\n{tname}: {len(entries)} entries, {len(unmatched)} unmatched")
    print(f"  noun-name dead branches ({len(dead)}): {' '.join(dead)}")
    print(f"  unmatched but kept ({len(keep)}): {' '.join(keep)}")
