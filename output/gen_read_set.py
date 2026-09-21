# -*- coding: utf-8 -*-
"""Generate the attribute *read set* of the model generator (dev plan
2026-09-15-attribute-change-class-alignment, D2 / S5).

The read set is the second vote of the metadata gate: a DESI attribute change
is skippable only when the dictionary says DCHC == 0 **and** no generator code
reads that attribute (S6).  "Reads" are approximated conservatively as every
uppercase string literal in generator source that is an E3D attribute name;
over-inclusion only costs skip coverage, omission would let a stale mesh through,
so the rule errs wide and the guard tests re-derive the same scan.

Rule (mirrored byte-for-byte by the guard tests, keep them in sync):
  * literal  = `"` [A-Z][A-Z0-9]{1,7} `"` anywhere in a .rs file;
  * each file is cut at its first `#[cfg(test)]` (test modules trail);
  * attribute universe = dictionary names (dchc_change_classes.json) ∪ runtime
    schema names (all_attr_info.json noun_attr_info_map);
  * a literal in the universe is a *name*, and so is any literal passed straight
    into an e3d-io reader (`.get_double("X")`, `.attribute("X")`, ...) even when
    the dictionary does not know it (Core3D pseudo attributes such as CPAR);
    a literal that is a dictionary noun (noun-family-matrix.json) and neither of
    those is ignored; anything else is a *non_attribute_literal* and is listed so
    the guard can tell "new unclassified literal" from "known non-attribute string";
  * STAMP_LITERALS are attribute names the generator never reads -- source only
    spells them to exclude them -- so they are listed apart in `stamp_literals`
    and kept out of `names` (2026-09-16 Q5).

Inputs : ../vendor/e3d-model/src/**/*.rs (src/bin excluded)
         ../vendor/e3d-io/src/{db_param_env.rs, record/catalogue_eval.rs, record/catalogue_expr.rs}
         src/** model-side modules listed in GEN_MODEL_READ_MODULES
         src/data_interface/dchc_change_classes.json, all_attr_info.json,
         ../vendor/e3d-model/data/noun-family-matrix.json
Outputs: ../vendor/e3d-model/data/attribute-read-set.json   (embedded by e3d-model)
         src/data_interface/generation_read_set.json          (embedded by gen-model)
"""
import json
import os
import re
import sys
from collections import Counter

sys.stdout.reconfigure(encoding="utf-8")

E3D_MODEL = os.path.join("..", "vendor", "e3d-model")
E3D_IO = os.path.join("..", "vendor", "e3d-io")

# gen-model 模型面读点：目录求值入口、房间、结构连接、电缆端点、direct 窗口规划器 / 刚体前移。
# 与 model_impact.rs 里的 GEN_MODEL_READ_MODULES 一字不差。
GEN_MODEL_READ_MODULES = [
    "src/fast_model/cata_model.rs",
    "src/fast_model/room_model.rs",
    "src/fast_model/room_topology.rs",
    "src/fast_model/room_predicate.rs",
    "src/fast_model/room_resident.rs",
    "src/data_interface/room_read_through.rs",
    "src/api/room_code.rs",
    "src/data_interface/structural_connection.rs",
    "src/data_interface/cable_endpoint.rs",
    "src/data_interface/window_root_plan.rs",
    "src/data_interface/rigid_motion.rs",
]

# e3d-io 目录表达式求值环境：替生成器读设计侧 ANGL/RADI/HEIG/DESP/UNIPAR，其余读的是目录元素
# （DATA 行的 PURP/DKEY/NUMB/PPRO/DPRO、SELE 的 QUES/ANSW/MAXA/TANS）。只作文档，不进 D2 那一票。
E3D_IO_CATALOGUE_EVAL_FILES = [
    "src/db_param_env.rs",
    "src/record/catalogue_eval.rs",
    "src/record/catalogue_expr.rs",
]

LITERAL = re.compile(r'"([A-Z][A-Z0-9]{1,7})"')
TEST_MARKER = "#[cfg(test)]"
# 戳（2026-09-16 Q5）：Design 应用随每次图形重建写下的缓存计数。它们在运行库 schema 里，所以按上面
# 那条规则本会被当成属性名——可源码提它们只是为了**按名字把它们排除掉**（e3d-model
# `element_diff::PLACEMENT_STAMP_ATTRIBUTES` 就是这张表，两仓守卫钉两边逐名相等），写一句名字不是读。
# 于是它们从 names 里单列进 `stamp_literals`：读取集合是元数据门 / 便宜路径判据的「生成器读它」那一票，
# 多收一个戳就把这条判据自己写的排除名单反过来挡住了。
# **例外**：真被传进 e3d-io 读取口（`.get_integer("CACHID")`）就照旧算读、照旧进 names——那时守卫那条
# 「戳不许出现在 names」立刻红，逼人回来看这处读是不是几何输入。
# 2026-09-19（计划 §8 N2 / t-334，用户拍板）：`SPAMAP` 与 `CACHID` 同档——同是 3.1 运行库 schema 里的簿记键、
# 字典无票、生成器不读、两态实测 0 红，进戳表。
STAMP_LITERALS = {"CACHID", "SPAMAP"}
# e3d-io `DbElement` 的全部按名读取口。直接传字面量进来的读点**无条件**算属性名——哪怕字典 / schema
# 都没收它（Core3D 伪属性 `CPAR` 就是这样），因为它被当属性读了这件事本身就是证据。
DIRECT_READERS = [
    ".get_string(\"", ".get_integer(\"", ".get_double(\"", ".get_bool(\"", ".get_ref(\"",
    ".get_integer_array(\"", ".get_double_array(\"", ".get_ref_array(\"", ".get_position(\"",
    ".get_direction(\"", ".get_orientation(\"", ".get_element(\"", ".get_element_array(\"",
    ".attribute(\"", ".has_attribute(\"",
]


def strip_tests(source: str) -> str:
    cut = source.find(TEST_MARKER)
    return source if cut < 0 else source[:cut]


def literals_in(path: str):
    """(全部大写字面量, 直接读点字面量)。"""
    with open(path, encoding="utf-8", errors="replace") as f:
        source = strip_tests(f.read())
    direct = set()
    for reader in DIRECT_READERS:
        start = 0
        while True:
            at = source.find(reader, start)
            if at < 0:
                break
            open_ = at + len(reader)
            close = source.index('"', open_)
            direct.add(source[open_:close])
            start = close
    return set(LITERAL.findall(source)), direct


def rust_sources(root: str, skip_bin: bool):
    out = []
    for dirpath, dirnames, filenames in os.walk(root):
        if skip_bin and os.path.basename(dirpath) == "bin" and os.path.dirname(dirpath) == root:
            dirnames[:] = []
            continue
        for name in filenames:
            if name.endswith(".rs"):
                out.append(os.path.join(dirpath, name))
    return sorted(out)


def write_json(path: str, payload) -> None:
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(payload, f, ensure_ascii=False, indent=0, separators=(",", ":"))
        f.write("\n")
    print(f"fixture written: {path}")


# 1. universe + nouns
dchc = json.load(open("src/data_interface/dchc_change_classes.json", encoding="utf-8"))
schema = set()
for attrs in json.load(open("all_attr_info.json", encoding="utf-8"))["noun_attr_info_map"].values():
    for rec in attrs.values():
        n = (rec.get("name") or "").strip().upper()
        if n:
            schema.add(n)
universe = set(dchc) | schema
nouns = {
    row["noun_name"]
    for row in json.load(open(os.path.join(E3D_MODEL, "data", "noun-family-matrix.json"), encoding="utf-8"))["nouns"]
}
print(f"universe: {len(universe)} attribute names (dictionary {len(dchc)} ∪ schema {len(schema)}); nouns {len(nouns)}")


def classify(literals: set, direct: set):
    """names = 在属性全集里的字面量 ∪ 直接读点字面量，戳除外；other = 其余非 noun 非戳字面量。"""
    names = sorted(n for n in literals if n in direct or (n in universe and n not in STAMP_LITERALS))
    stamps = sorted(n for n in literals if n in STAMP_LITERALS and n not in direct)
    rest = [n for n in literals if n not in universe and n not in direct and n not in STAMP_LITERALS]
    other = sorted(n for n in rest if n not in nouns)
    noun_only = sorted(n for n in rest if n in nouns)
    return names, other, noun_only, stamps


def describe(label: str, names, other, noun_only, stamps) -> None:
    print(f"{label}: {len(names)} attribute names, {len(other)} non-attribute literals, {len(noun_only)} noun literals, stamps {stamps}")
    print("  DCHC:", dict(sorted(Counter(str(dchc.get(n)) for n in names).items())))
    print("  DCHC=0:", [n for n in names if dchc.get(n) == 0])
    print("  not in dictionary:", [n for n in names if n not in dchc])


def collect(paths):
    literals, direct = set(), set()
    for path in paths:
        lits, reads = literals_in(path)
        literals |= lits
        direct |= reads
    return literals, direct


# 2. e3d-model src (src/bin excluded)
em_names, em_other, em_nouns, em_stamps = classify(*collect(rust_sources(os.path.join(E3D_MODEL, "src"), skip_bin=True)))
describe("e3d-model", em_names, em_other, em_nouns, em_stamps)
write_json(
    os.path.join(E3D_MODEL, "data", "attribute-read-set.json"),
    {
        "source": "uppercase string literals in e3d-model src/**/*.rs (src/bin excluded, each file cut at its first #[cfg(test)]) that are E3D attribute names (dictionary ∪ runtime schema); generated by gen-model output/gen_read_set.py",
        "rule": "literal = \"[A-Z][A-Z0-9]{1,7}\"; in the attribute universe, or passed straight into an e3d-io reader (.get_*(\"X\") / .attribute(\"X\")) -> names; stamp (see stamp_literals) that no reader takes -> stamp_literals, never names; dictionary noun -> ignored; anything else -> non_attribute_literals",
        "names": em_names,
        "stamp_literals": sorted(STAMP_LITERALS),
        "non_attribute_literals": em_other,
    },
)

# 3. gen-model model-side modules
for rel in GEN_MODEL_READ_MODULES:
    if not os.path.isfile(rel):
        sys.exit(f"GEN_MODEL_READ_MODULES entry missing: {rel}")
gm_names, gm_other, gm_nouns, gm_stamps = classify(*collect(GEN_MODEL_READ_MODULES))
describe("gen-model model side", gm_names, gm_other, gm_nouns, gm_stamps)

# 4. e3d-io catalogue evaluation (documentation only)
io_paths = [os.path.join(E3D_IO, rel) for rel in E3D_IO_CATALOGUE_EVAL_FILES]
for path in io_paths:
    if not os.path.isfile(path):
        sys.exit(f"E3D_IO_CATALOGUE_EVAL_FILES entry missing: {path}")
io_names, io_other, io_nouns, io_stamps = classify(*collect(io_paths))
describe("e3d-io catalogue eval (documentation, no vote)", io_names, io_other, io_nouns, io_stamps)

union = sorted(set(em_names) | set(gm_names))
write_json(
    "src/data_interface/generation_read_set.json",
    {
        "source": "e3d-model data/attribute-read-set.json ∪ gen-model model-side modules; generated by output/gen_read_set.py (dev plan 2026-09-15 D2 / S5)",
        "rule": "literal = \"[A-Z][A-Z0-9]{1,7}\" in .rs sources cut at the first #[cfg(test)]; attribute universe = dictionary ∪ runtime schema, plus any literal passed straight into an e3d-io reader; `stamp_literals` are attribute names source only spells to exclude them (e3d-model element_diff::PLACEMENT_STAMP_ATTRIBUTES) and are held out of every `names`; `names` is the D2 vote: a DESI attribute change is skippable only when DCHC == 0 and the name is absent here",
        "stamp_literals": sorted(STAMP_LITERALS),
        "e3d_model": em_names,
        "gen_model": {
            "files": GEN_MODEL_READ_MODULES,
            "names": gm_names,
            "non_attribute_literals": gm_other,
        },
        "e3d_io_catalogue_eval": {
            "vote": False,
            "why": "reads on behalf of catalogue evaluation: design-side names it touches (ANGL RADI HEIG DESP UNIPAR ISPE HBOR TBOR PBOR) are DCHC != 0 or already in e3d_model; its DCHC == 0 names (ANSW DKEY MAXA PSPE PURP QUES TANS) are read on catalogue DATA/SELE rows, never on DESI elements, and the metadata gate only runs on DESI windows",
            "files": E3D_IO_CATALOGUE_EVAL_FILES,
            "names": io_names,
            "non_attribute_literals": io_other,
        },
        "names": union,
    },
)
print(f"union (the vote): {len(union)} names; DCHC=0 members: {[n for n in union if dchc.get(n) == 0]}")
