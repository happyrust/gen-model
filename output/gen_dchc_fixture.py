# -*- coding: utf-8 -*-
"""Distill the per-attribute design-change fields out of the 4.7 MB dictionary
export into small committed fixtures, generate the sixth (dictionary-derived)
attribute-effect table, and recompute the dead-entry cleanup lists for the
curated tables.

Inputs : output/noun_attr_fields.json  (ADR-008 NounLayoutExport, not committed;
                                        falls back to the vendored e3d-io copy
                                        ../vendor/e3d-io/schema/e3d31/catalog/,
                                        or pass a path as argv[1])
         all_attr_info.json            (runtime schema; same bytes as the copy
                                        aios_core embeds)
         src/data_interface/model_impact.rs (the five curated tables)
Outputs: src/data_interface/dchc_change_classes.json
             name -> DCHC (field 596407, read by change/IDCHNG 0x1022E302)
         src/data_interface/plcf_flags.json
             names whose PLCF (field 652066, the second field IDCHNG reads) == 1;
             report-only, never a verdict input (dev plan 2026-09-15 D4)
         src/data_interface/dictionary_change_class_attr_names.json
             the sixth effect table: every dictionary name with DCHC in {1,2,4}
             that none of the five curated tables registers (dev plan D3).
             DCHC 1/2 and schema ELEMENT references -> dependency_cascade,
             the rest -> direct_geometry, so `classify_attribute_effect` gives
             exactly what `classify_attribute_effect_with_meta` used to give
             after the A2 reference upgrade: behaviour unchanged, only the
             `Unknown` noise goes away.
"""
import json
import os
import re
import sys
from collections import Counter

sys.stdout.reconfigure(encoding="utf-8")

DICT_EXPORT_CANDIDATES = [
    "output/noun_attr_fields.json",
    "../vendor/e3d-io/schema/e3d31/catalog/noun_attr_fields.json",
]


def locate_dictionary_export() -> str:
    if len(sys.argv) > 1:
        return sys.argv[1]
    for path in DICT_EXPORT_CANDIDATES:
        if os.path.isfile(path):
            return path
    sys.exit(
        "dictionary export not found; tried "
        + ", ".join(DICT_EXPORT_CANDIDATES)
        + " (pass the path as argv[1])"
    )


def write_json(path: str, payload) -> None:
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(payload, f, ensure_ascii=False, indent=0, separators=(",", ":"))
        f.write("\n")
    print(f"fixture written: {path}")


# 1. DCHC / PLCF maps from the dictionary export
export_path = locate_dictionary_export()
raw = json.load(open(export_path, encoding="utf-8"))
print(f"dictionary export: {export_path} ({len(raw)} records)")

dchc = {}
plcf = {}
for rec in raw.values():
    name = (rec.get("name") or "").strip().upper()
    if not name:
        continue
    fields = rec.get("f", {})
    code = fields.get("DCHC", {}).get("i")
    if code is not None:
        dchc[name] = int(code)
    flag = fields.get("PLCF", {}).get("i")
    if flag is not None:
        plcf[name] = int(flag)

nonzero = {k: v for k, v in dchc.items() if v != 0}
print(f"dictionary: {len(dchc)} attrs with DCHC, {len(nonzero)} non-zero")
print("DCHC histogram:", dict(sorted(Counter(dchc.values()).items())))
print("PLCF histogram:", dict(sorted(Counter(plcf.values()).items())))
assert set(plcf.values()) <= {0, 1}, "PLCF is documented as a 0/1 flag; re-check IDCHNG if this fires"

write_json("src/data_interface/dchc_change_classes.json", dict(sorted(dchc.items())))

plcf_names = sorted(n for n, v in plcf.items() if v == 1)
write_json(
    "src/data_interface/plcf_flags.json",
    {
        "field": "PLCF",
        "field_hash": 652066,
        "reader": "Core3D 3.1 change/IDCHNG 0x1022E302: ATAINT(attr, 652066) == 1 sets the INTUBE flag dword_10E98500 that UPDATD hands to PLCDEL",
        "source": os.path.basename(export_path),
        "count": len(plcf_names),
        "names": plcf_names,
    },
)
print(f"PLCF == 1: {len(plcf_names)} names; DCHC of those:",
      dict(sorted(Counter(dchc.get(n) for n in plcf_names).items(), key=lambda kv: str(kv[0]))))

# 2. curated tables + runtime schema
src = open("src/data_interface/model_impact.rs", encoding="utf-8").read()


def table(name: str):
    m = re.search(r"pub const " + name + r": &\[&str\] = &\[(.*?)\];", src, re.S)
    body = re.sub(r"//[^\n]*", "", m.group(1))
    return [n for n in re.findall(r'"([A-Z0-9:]+)"', body)]


CURATED_TABLES = [
    "DATA_ONLY_ATTR_NAMES",
    "STRUCTURAL_ATTR_NAMES",
    "TRANSFORM_ONLY_ATTR_NAMES",
    "DEPENDENCY_CASCADE_ATTR_NAMES",
    "DIRECT_GEOMETRY_ATTR_NAMES",
]
curated = {}
for tname in CURATED_TABLES:
    for n in table(tname):
        curated.setdefault(n, tname)
print(f"curated: {len(curated)} names across {len(CURATED_TABLES)} tables")

schema = set()
references = set()
# `named_attr_info_map` is empty on disk (aios_core fills it at load time); the
# serialized table is `noun_attr_info_map`: noun hash -> attr hash -> AttrInfo.
data = json.load(open("all_attr_info.json", encoding="utf-8"))["noun_attr_info_map"]
for attrs in data.values():
    for rec in attrs.values():
        n = (rec.get("name") or "").strip().upper()
        if n:
            schema.add(n)
            if rec.get("att_type") == "ELEMENT":
                references.add(n)
# noun names come from the committed ADR-008 layout export (the schema only has hashes).
nouns = set(
    (rec.get("noun") or "").strip().upper()
    for rec in json.load(open("output/noun_layout.json", encoding="utf-8"))
)
nouns.discard("")
assert schema and references and nouns, "runtime schema / noun layout failed to load"
print(f"schema: {len(schema)} attr names ({len(references)} ELEMENT references), {len(nouns)} nouns")

# 3. the sixth table: DCHC in {1,2,4} and unregistered by the five curated tables
unregistered = sorted(n for n, c in dchc.items() if c in (1, 2, 4) and n not in curated)
dependency_cascade = [n for n in unregistered if dchc[n] in (1, 2) or n in references]
direct_geometry = [n for n in unregistered if n not in dependency_cascade]
write_json(
    "src/data_interface/dictionary_change_class_attr_names.json",
    {
        "source": "E3D dictionary DCHC (field 596407) minus the five curated tables in model_impact.rs; generated by output/gen_dchc_fixture.py",
        "rule": "DCHC in {1,2} or schema att_type == ELEMENT -> dependency_cascade; other DCHC == 4 -> direct_geometry",
        "dependency_cascade": dependency_cascade,
        "direct_geometry": direct_geometry,
    },
)
print(
    f"sixth table: {len(unregistered)} unregistered names with DCHC in {{1,2,4}} "
    f"({dict(sorted(Counter(dchc[n] for n in unregistered).items()))}); "
    f"dependency_cascade {len(dependency_cascade)}, direct_geometry {len(direct_geometry)}"
)
print("  dependency_cascade:", " ".join(dependency_cascade))
absent = [n for n in unregistered if n not in schema]
print(f"  dictionary-only (no owner in the runtime schema) {len(absent)}: {' '.join(absent)}")
prefix_rule = [n for n in unregistered if n.startswith("PARA")]
print(f"  already covered by the PARA* prefix rule {len(prefix_rule)}: {' '.join(prefix_rule)}")

# 4. dead-entry recomputation for the curated tables
for tname in ["DEPENDENCY_CASCADE_ATTR_NAMES", "DIRECT_GEOMETRY_ATTR_NAMES"]:
    entries = table(tname)
    unmatched = [n for n in entries if n not in schema]
    dead = sorted(n for n in unmatched if n not in dchc and n in nouns)
    keep = sorted(n for n in unmatched if n not in dead)
    print(f"\n{tname}: {len(entries)} entries, {len(unmatched)} unmatched")
    print(f"  noun-name dead branches ({len(dead)}): {' '.join(dead)}")
    print(f"  unmatched but kept ({len(keep)}): {' '.join(keep)}")
