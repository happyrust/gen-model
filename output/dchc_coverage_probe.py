"""Cross-check the model_impact attribute classification against two independent
sources: the E3D attribute dictionary export (ADR-008) and the runtime attribute
schema. Read-only; writes nothing.

    python output/dchc_coverage_probe.py

Inputs:
    src/data_interface/model_impact.rs   curated lists, parsed in place
    output/noun_attr_fields.json         4270 attributes x 57 dict fields (DCHC)
    all_attr_info.json                   runtime (noun, attr) schema
    noun_flags.json                      1931 dabacon nouns
"""

import json
import os
import re

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def load(*parts):
    return json.load(open(os.path.join(REPO, *parts), encoding="utf-8"))


src = open(os.path.join(REPO, "src", "data_interface", "model_impact.rs"),
           encoding="utf-8").read()


def curated_affects():
    return const_set("DIRECT_GEOMETRY_ATTR_NAMES")


def const_set(name):
    i = src.index("const " + name)
    j = src.index("[", i)
    k = src.index("];", j)
    return set(re.findall(r'"([A-Z0-9_:]+)"', src[j:k]))


dchc = {}
for rec in load("output", "noun_attr_fields.json").values():
    n = (rec.get("name") or "").strip().upper()
    if n:
        dchc[n] = rec["f"].get("DCHC", {}).get("i")

runtime = set()
pairs = 0
noun_attr = load("all_attr_info.json")["noun_attr_info_map"]
for attrs in noun_attr.values():
    for a in attrs.values():
        n = (a.get("name") or "").strip().upper()
        if n:
            runtime.add(n)
            pairs += 1

nouns = {(r.get("noun_name") or "").strip().upper()
         for r in load("noun_flags.json")}
nouns.discard("")

curated = curated_affects()

print("== sources ==")
print("  dictionary export : %d attribute names with a DCHC field" % len(dchc))
print("  runtime schema    : %d nouns / %d (noun,attr) pairs / %d names"
      % (len(noun_attr), pairs, len(runtime)))
print("  dabacon nouns     : %d" % len(nouns))
print("  runtime names missing from dictionary export : %d"
      % len(runtime - set(dchc)))

print()
print("== DCHC histogram over the whole dictionary ==")
hist = {}
for c in dchc.values():
    hist[c] = hist.get(c, 0) + 1
for code in sorted(hist, key=lambda c: (c is None, c)):
    print("  DCHC=%-4s %d" % (code, hist[code]))

print()
print("== composition of attribute_affects_model (%d entries) ==" % len(curated))
in_dict = sorted(n for n in curated if n in dchc)
runtime_only = sorted(n for n in curated if n not in dchc and n in runtime)
rest = sorted(n for n in curated if n not in dchc and n not in runtime)
as_noun = sorted(n for n in rest if n in nouns)
neither = sorted(n for n in rest if n not in nouns)
print("  in dictionary (comparable)   : %d" % len(in_dict))
print("  runtime attribute only       : %d  %s" % (len(runtime_only), runtime_only))
print("  dabacon NOUN name, dead arm  : %d" % len(as_noun))
print("     ", as_noun)
print("  neither attribute nor noun   : %d" % len(neither))
print("     ", neither)
print("  entries that are both        : %d"
      % len([n for n in curated if n in nouns and (n in dchc or n in runtime)]))

agree = [n for n in in_dict if dchc[n]]
zero = sorted(n for n in in_dict if not dchc[n])
print()
print("  comparable with DCHC != 0    : %d / %d" % (len(agree), len(in_dict)))
print("  comparable with DCHC == 0    : %d  %s" % (len(zero), zero))

print()
print("== effect lists vs dictionary DCHC ==")
for name in ["DATA_ONLY_ATTR_NAMES", "STRUCTURAL_ATTR_NAMES",
             "TRANSFORM_ONLY_ATTR_NAMES", "DEPENDENCY_CASCADE_ATTR_NAMES"]:
    names = const_set(name)
    known = sorted(n for n in names if n in dchc)
    print("  %s  entries=%d in dict=%d" % (name, len(names), len(known)))
    buckets = {}
    for n in known:
        buckets.setdefault(dchc[n], []).append(n)
    for code in sorted(buckets):
        print("      DCHC=%d : %2d  %s" % (code, len(buckets[code]), buckets[code]))
print()
print("== full membership of DCHC classes 1/2/3 (the semantics evidence) ==")
detail = []
for rec in load("output", "noun_attr_fields.json").values():
    n = (rec.get("name") or "").strip().upper()
    c = rec["f"].get("DCHC", {}).get("i")
    if n and c in (1, 2, 3):
        detail.append((c, n,
                       rec["f"].get("QTXT", {}).get("s", ""),
                       rec["f"].get("DESTEX", {}).get("s", "")))
for c, n, q, d in sorted(detail):
    print("  DCHC=%d  %-8s %-16s %s" % (c, n, q, d))
print("  -> 1 = head end, 2 = tail end, 3 = rigid pose, 4 = general catch-all")
print()
print("== owning nouns of the divergent TRANSFORM_ONLY attributes ==")
flags = {}
for r in load("noun_flags.json"):
    flags[str(r["noun_hash"])] = (r["noun_name"].strip().upper(),
                                  r["primitive"] or r["geomset"] or r["extrusion"])
for w in ["POSS", "POSE", "CPOS", "NPOS", "POSL", "YDIR", "ZDIR", "POS", "ORI", "BFORI"]:
    owners = set()
    for nh, attrs in noun_attr.items():
        for a in attrs.values():
            if (a.get("name") or "").strip().upper() == w:
                nn = flags.get(str(nh))
                owners.add("%s%s" % (nn[0], "(geo)" if nn[1] else "") if nn else "hash:" + str(nh))
                break
    shown = sorted(owners)[:12]
    print("  %-6s %3d nouns: %s%s" % (w, len(owners), shown,
                                      " ..." if len(owners) > 12 else ""))
