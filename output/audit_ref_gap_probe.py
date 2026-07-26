# -*- coding: utf-8 -*-
"""Audit probe: do any ELEMENT-typed (reference) attributes sit in a
non-CASCADE curated table? Those would silently skip ref_rev edge
extraction (reference_cascade_targets only reverses DependencyCascade).
Also: which PARA*-prefixed real attributes exist (prefix-rule net width).
"""
import json
import re
import sys

sys.stdout.reconfigure(encoding="utf-8")

src = open("src/data_interface/model_impact.rs", encoding="utf-8").read()


def table(name: str) -> set:
    m = re.search(r"pub const " + name + r": &\[&str\] = &\[(.*?)\];", src, re.S)
    body = re.sub(r"//[^\n]*", "", m.group(1))  # strip inline comments
    return set(re.findall(r'"([A-Z0-9:]+)"', body))


tables = {
    n: table(n)
    for n in [
        "DATA_ONLY_ATTR_NAMES",
        "STRUCTURAL_ATTR_NAMES",
        "TRANSFORM_ONLY_ATTR_NAMES",
        "DEPENDENCY_CASCADE_ATTR_NAMES",
        "DIRECT_GEOMETRY_ATTR_NAMES",
    ]
}
print("== table sizes ==")
for k, v in tables.items():
    print(f"  {k}: {len(v)}")

data = json.load(open("all_attr_info.json", encoding="utf-8"))["named_attr_info_map"]

element_attrs = set()   # names with att_type == ELEMENT anywhere in schema
all_attrs = set()
type_by_name = {}       # name -> set of att_types (may vary per noun)
for noun, attrs in data.items():
    for rec in attrs.values():
        name = (rec.get("name") or "").strip().upper()
        if not name:
            continue
        all_attrs.add(name)
        t = rec.get("att_type", "")
        type_by_name.setdefault(name, set()).add(t)
        if t == "ELEMENT":
            element_attrs.add(name)

print(f"\nschema: {len(all_attrs)} distinct attr names, {len(element_attrs)} ELEMENT-typed")

print("\n== ELEMENT-typed names sitting in NON-cascade tables (potential ref_rev gap) ==")
for tname in ["DATA_ONLY_ATTR_NAMES", "STRUCTURAL_ATTR_NAMES",
              "TRANSFORM_ONLY_ATTR_NAMES", "DIRECT_GEOMETRY_ATTR_NAMES"]:
    hit = sorted(tables[tname] & element_attrs)
    print(f"  {tname}: {len(hit)}")
    for n in hit:
        print(f"    {n}  types={sorted(type_by_name[n])}")

print("\n== CASCADE entries that are ELEMENT-typed (edges actually extractable) ==")
casc = tables["DEPENDENCY_CASCADE_ATTR_NAMES"]
casc_elem = sorted(casc & element_attrs)
print(f"  {len(casc_elem)}/{len(casc)}: {', '.join(casc_elem)}")

print("\n== PARA*-prefixed real schema attributes (prefix-rule catch width) ==")
para = sorted(n for n in all_attrs if n.startswith("PARA"))
for n in para:
    print(f"  {n}  types={sorted(type_by_name[n])}")
