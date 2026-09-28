# -*- coding: utf-8 -*-
"""Build a self-contained HTML explorer for the attribute-change increment verdict.

Answers, for any E3D attribute short name (optionally pinned to a noun):
改了它，这一窗要不要重算？core 自己会做到什么粒度？我们走哪条路？为什么？

Every column is derived from the committed sources, never hand-typed, so the page
cannot drift from `model_impact.rs` / `window_root_plan.rs`.

Inputs : src/data_interface/model_impact.rs            (5 curated effect tables)
         src/data_interface/window_root_plan.rs        (P23, pinned to UPGRPH 0x1022EAEE)
         src/data_interface/dchc_change_classes.json   (E3D dictionary DCHC snapshot)
         src/data_interface/plcf_flags.json            (dictionary PLCF == 1)
         src/data_interface/generation_read_set.json   (gen-model + e3d-io read scan)
         ../vendor/e3d-model/data/attribute-read-set.json (e3d-model read scan)
         ../vendor/e3d-model/data/noun-family-matrix.json (I*COM* family bits per noun)
         resource/dict/noun_caps.json                  (PRMF/TOPF/POPF/GMSC/gb per noun)
         all_attr_info.json                            (runtime schema: noun -> attrs, att_type)
Output : output/attribute-verdict-explorer.html
"""
import json
import re
import sys
from collections import Counter
from datetime import datetime

sys.stdout.reconfigure(encoding="utf-8")

TABLE_ORDER = [
    ("DATA_ONLY_ATTR_NAMES", "DataOnly"),
    ("STRUCTURAL_ATTR_NAMES", "StructuralMembership"),
    ("TRANSFORM_ONLY_ATTR_NAMES", "TransformOnly"),
    ("DEPENDENCY_CASCADE_ATTR_NAMES", "DependencyCascade"),
    ("DIRECT_GEOMETRY_ATTR_NAMES", "DirectGeometry"),
]

# `change/UPGRPH` 类别 4 的 REPALL 叉：目录件 / 几何集族（`I*COM*` noun 谓词）
# 与三个内联 noun 常量（`0x1022f4fd` `0x1022f53c` `0x1022f549`）。
REPALL_FAMILIES = ["ICCOMP", "IGMCOM", "INGCOM", "IG2COM", "IPLCOM", "IPTCOM"]
REPALL_NOUNS = ["SPCO", "SVER", "DATA"]

# `descases/VDESPT`（`0x101F33A9`）DCHC 门外/门内的四条 noun × 属性专项规则（§9.2）。
VDESPT_RULES = {
    ("PLOO", "HEIG"): ["U", "进 EVALAT 前把元素换成 owner（PANE / FLOOR），按 owner 的 HEIG 评估"],
    ("SJOI", "JFRE"): ["V", "写回：JFRE 翻真时把位置差折进 DELP，让节点留在原地（不进队列）"],
    ("COCO", "CTYP"): ["W", "写回：不再匹配的 COCORE 引用 NULIFY（不进队列）"],
    ("DATA", "PPRO"): ["X", "另调 descases/SCRUSE(el)：规则重用侧"],
    ("DATA", "DPRO"): ["X", "另调 descases/SCRUSE(el)：规则重用侧"],
}

src = open("src/data_interface/model_impact.rs", encoding="utf-8").read()
plan_src = open("../vendor/e3d-model/src/category.rs", encoding="utf-8").read()


def curated(const_name, source=None):
    body = re.search(
        r"pub const " + const_name + r": .*?= &?\[(.*?)\];", source or src, re.S
    ).group(1)
    return re.findall(r'"([A-Z0-9:]+)"', re.sub(r"//[^\n]*", "", body))


effect_of = {}
table_index = {}
for idx, (const_name, effect) in enumerate(TABLE_ORDER):
    names = curated(const_name)
    for name in names:
        if name in effect_of:
            raise SystemExit(f"{name} 同时出现在两张效果表里，源码那条重名测试应当先红")
        effect_of[name] = effect
        table_index[name] = idx
    print(f"{const_name}: {len(names)}")

# P23：类别 3 也整件重建的 23 个 noun。源码那份由 `p23_matches_upgrph` 钉在
# `change/UPGRPH 0x1022EAEE` 的 23 条 `cmp eax, imm32` 上，这里直接引用，不再手抄。
p23 = curated("PLACEMENT_DERIVED_NOUNS", plan_src)
assert len(p23) == 23, f"P23 应当是 23 个 noun，读到 {len(p23)}"
print(f"P23（UPGRPH 类别 3 整件重建）: {len(p23)}")

dchc = json.load(open("src/data_interface/dchc_change_classes.json", encoding="utf-8"))
dictionary_size = len(dchc)
# `raw_dchc_code` 的两个强制码优先于字典（reverse §4.3/§11.2.1）。
dchc["REDRAW"] = 4
dchc["INTUBE"] = 1
plcf = set(json.load(open("src/data_interface/plcf_flags.json", encoding="utf-8"))["names"])
# WNOEVT (want-no-event) gate: attributes whose put fires no change event at all,
# decoded offline from attlib.dat (output/gen_wnoevt_flags.py, DCHC/PLCF parity).
wnoevt = set(json.load(open("src/data_interface/wnoevt_flags.json", encoding="utf-8"))["names"])
reads = set(json.load(open("src/data_interface/generation_read_set.json", encoding="utf-8"))["names"])
reads |= set(json.load(open("../vendor/e3d-model/data/attribute-read-set.json", encoding="utf-8"))["names"])

schema = set()
refs = set()
noun_attrs = {}
for noun_hash, attrs in json.load(open("all_attr_info.json", encoding="utf-8"))[
    "noun_attr_info_map"
].items():
    bucket = noun_attrs.setdefault(int(noun_hash), set())
    for rec in attrs.values():
        name = (rec.get("name") or "").strip().upper()
        if not name:
            continue
        schema.add(name)
        bucket.add(name)
        if rec.get("att_type") == "ELEMENT":
            refs.add(name)

caps_rows = json.load(open("resource/dict/noun_caps.json", encoding="utf-8"))
caps = {row["noun_name"]: row for row in caps_rows}
noun_name_of = {row["noun_hash"]: row["noun_name"] for row in caps_rows}
for row in json.load(open("output/noun_layout.json", encoding="utf-8")):
    noun_name_of.setdefault(row["hash"], row["noun"])

families = {
    row["noun_name"]: set(row["families"])
    for row in json.load(
        open("../vendor/e3d-model/data/noun-family-matrix.json", encoding="utf-8")
    )["nouns"]
}

print(
    f"dictionary {dictionary_size}  PLCF=1 {len(plcf)}  read-set {len(reads)}  "
    f"schema {len(schema)}  ELEMENT refs {len(refs)}  noun caps {len(caps)}"
)

universe = sorted(set(dchc) | set(effect_of) | reads | schema)
index_of = {name: i for i, name in enumerate(universe)}
rows = [
    [
        name,
        dchc.get(name, -1),
        table_index.get(name, -1),
        int(name in reads),
        int(name in refs),
        int(name in schema),
        int(name in plcf),
    ]
    for name in universe
]
print(f"universe {len(rows)} names")

# ── noun 维：位标记 + 属性下标（增量 base36 编码，整页自带不外挂） ──────────────
BIT_PRIMITIVE = 1        # 字典 PRMF → nounlib/LPRMTV：UPGRPH 的「单图元」叉
BIT_TOPLEVEL = 2         # 字典 TOPF → nounlib/LISTOP：UPGRPH 的 top == ref 叉（noun 级近似）
BIT_POINT = 4            # 字典 POPF → nounlib/LPOINT：EVALAT 规则 S
BIT_GEOMSET = 8          # 字典 GMSC
BIT_EXTRUSION = 16
BIT_P23 = 32             # UPGRPH 内联 23 个 noun 常量
BIT_IPCOMP = 64          # 字典 PIPC → nounlib/IPCOMP：管件
BIT_IHCOMP = 128         # 字典 HNGC → nounlib/IHCOMP：支吊架元件
BIT_REPALL = 256         # ICCOMP/IGMCOM/INGCOM/IG2COM/IPLCOM/IPTCOM 或 SPCO/SVER/DATA


def b36(n):
    if n == 0:
        return "0"
    digits = []
    while n:
        n, r = divmod(n, 36)
        digits.append("0123456789abcdefghijklmnopqrstuvwxyz"[r])
    return "".join(reversed(digits))


noun_rows = []
pair_count = 0
for noun_hash, names in sorted(noun_attrs.items()):
    noun = noun_name_of.get(noun_hash)
    if not noun:
        continue
    cap = caps.get(noun, {})
    fam = families.get(noun, set())
    bits = 0
    bits |= BIT_PRIMITIVE if cap.get("primitive") else 0
    bits |= BIT_TOPLEVEL if cap.get("toplevel") else 0
    bits |= BIT_POINT if cap.get("point") else 0
    bits |= BIT_GEOMSET if cap.get("geomset") else 0
    bits |= BIT_EXTRUSION if cap.get("extrusion") else 0
    bits |= BIT_P23 if noun in p23 else 0
    bits |= BIT_IPCOMP if "IPCOMP" in fam else 0
    bits |= BIT_IHCOMP if "IHCOMP" in fam else 0
    bits |= BIT_REPALL if (fam & set(REPALL_FAMILIES)) or noun in REPALL_NOUNS else 0
    idxs = sorted(index_of[n] for n in names if n in index_of)
    pair_count += len(idxs)
    deltas, prev = [], 0
    for i in idxs:
        deltas.append(b36(i - prev))
        prev = i
    noun_rows.append([noun, bits, ".".join(deltas), cap.get("graphics_behaviour") or 0])

print(f"nouns {len(noun_rows)}  (noun, attr) pairs {pair_count}")
print(
    "  P23 命中 schema 的:",
    sum(1 for r in noun_rows if r[1] & BIT_P23),
    "| toplevel:",
    sum(1 for r in noun_rows if r[1] & BIT_TOPLEVEL),
    "| primitive:",
    sum(1 for r in noun_rows if r[1] & BIT_PRIMITIVE),
    "| REPALL 族:",
    sum(1 for r in noun_rows if r[1] & BIT_REPALL),
)

# ── 仍按老口径打一遍统计，脚本输出就是回归基线 ───────────────────────────────
AFFECTS = {"DirectGeometry", "DependencyCascade", "StructuralMembership"}
verdicts = Counter()
gate2_extra = 0
conflicts = []


def effect(name, idx):
    if idx >= 0:
        return TABLE_ORDER[idx][1]
    if name.startswith("PARA"):
        return "DirectGeometry"
    return None


for name, code, idx, read, ref, _, _ in rows:
    eff = effect(name, idx) or ("DependencyCascade" if ref else "Unknown")
    if effect(name, idx) == "DataOnly" and code == 0:
        verdicts["MetadataOnly"] += 1
        continue
    if code == 0 and not read:
        gate2_extra += 1
        if eff in AFFECTS:
            conflicts.append((name, eff, idx >= 0))
        continue
    verdicts[eff] += 1

print("gate1 放行:", verdicts["MetadataOnly"], "| gate2 再放行:", gate2_extra)
print("gate2 与效果表打架:", len(conflicts), "其中 curated:", sum(1 for c in conflicts if c[2]))
print("  例:", " ".join(n for n, _, _ in conflicts[:12]))

stats = {
    "universe": len(rows),
    "curated": len(effect_of),
    "dict": dictionary_size,
    "reads": len(reads),
    "refs": len(refs),
    "schema": len(schema),
    "nouns": len(noun_rows),
    "pairs": pair_count,
    "plcf": len(plcf),
}

TEMPLATE = """<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>属性改动 → 增量去向 · gen-model-model-cache</title>
<style>
  :root {
    --bg: #0d1117; --panel: #161b22; --line: #21262d; --line2: #30363d;
    --fg: #e6edf3; --dim: #8b949e; --faint: #6e7681;
    --regen: #f85149; --cascade: #d29922; --transform: #58a6ff; --skip: #3fb950; --unknown: #bc8cff;
    --mono: ui-monospace, "Cascadia Mono", "SF Mono", Consolas, monospace;
  }
  * { box-sizing: border-box; }
  body { margin: 0; background: var(--bg); color: var(--fg);
         font: 14px/1.6 -apple-system, "Segoe UI", "Microsoft YaHei", sans-serif; }
  header { padding: 28px 24px 18px; border-bottom: 1px solid var(--line); }
  h1 { margin: 0 0 6px; font-size: 20px; font-weight: 600; letter-spacing: .2px; }
  .sub { color: var(--dim); font-size: 13px; max-width: 78ch; }
  .sub code { font-family: var(--mono); color: var(--fg); }
  .stats { display: flex; flex-wrap: wrap; gap: 10px; margin-top: 16px; }
  .stat { background: var(--panel); border: 1px solid var(--line2); border-radius: 6px;
          padding: 7px 12px; font-size: 12px; color: var(--dim); }
  .stat b { color: var(--fg); font-family: var(--mono); font-size: 14px; margin-right: 6px; }
  main { display: grid; grid-template-columns: minmax(0,1fr) 360px; gap: 24px; padding: 20px 24px 60px; }
  @media (max-width: 1100px) { main { grid-template-columns: minmax(0,1fr); } }
  .bar { display: flex; flex-wrap: wrap; gap: 10px; align-items: center; margin-bottom: 12px; }
  input[type=search] { background: var(--panel); border: 1px solid var(--line2);
    border-radius: 6px; color: var(--fg); font: 15px var(--mono); padding: 10px 14px; }
  #q { flex: 1 1 240px; }
  #noun { flex: 0 1 220px; }
  input[type=search]:focus { outline: none; border-color: var(--transform); }
  .chip { background: var(--panel); border: 1px solid var(--line2); border-radius: 999px;
    padding: 5px 13px; font-size: 12px; color: var(--dim); cursor: pointer; user-select: none; }
  .chip:hover { border-color: var(--faint); color: var(--fg); }
  .chip.on { background: #1f6feb22; border-color: var(--transform); color: var(--fg); }
  .toggle { display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--dim);
    background: var(--panel); border: 1px solid var(--line2); border-radius: 6px; padding: 6px 12px; cursor: pointer; }
  .toggle input { accent-color: var(--transform); }
  .count { color: var(--faint); font-size: 12px; margin: 0 0 8px; }
  .nouncard { background: var(--panel); border: 1px solid var(--line2); border-left: 3px solid var(--transform);
    border-radius: 6px; padding: 10px 14px; margin-bottom: 12px; font-size: 12.5px; color: var(--dim); }
  .nouncard b { color: var(--fg); font-family: var(--mono); }
  .nouncard .caps { margin-top: 6px; display: flex; flex-wrap: wrap; gap: 6px; }
  table { width: 100%; border-collapse: collapse; }
  th { text-align: left; font-size: 11px; font-weight: 500; color: var(--faint); text-transform: uppercase;
       letter-spacing: .6px; padding: 8px 10px; border-bottom: 1px solid var(--line2); position: sticky; top: 0;
       background: var(--bg); }
  td { padding: 8px 10px; border-bottom: 1px solid var(--line); vertical-align: top; }
  tr:hover td { background: #ffffff06; }
  .name { font-family: var(--mono); font-weight: 600; font-size: 14px; }
  .tag { display: inline-block; font-size: 11px; font-family: var(--mono); border-radius: 4px;
         padding: 2px 7px; border: 1px solid transparent; white-space: nowrap; }
  .t-regen { color: var(--regen); background: #f8514918; border-color: #f8514940; }
  .t-cascade { color: var(--cascade); background: #d2992218; border-color: #d2992240; }
  .t-transform { color: var(--transform); background: #58a6ff18; border-color: #58a6ff40; }
  .t-skip { color: var(--skip); background: #3fb95018; border-color: #3fb95040; }
  .t-unknown { color: var(--unknown); background: #bc8cff18; border-color: #bc8cff40; }
  .t-mute { color: var(--faint); background: #8b949e14; border-color: #8b949e30; }
  .why { color: var(--dim); font-size: 12px; }
  .why b { color: var(--fg); font-weight: 500; }
  .core { font-size: 12px; }
  .core .op { font-family: var(--mono); color: var(--fg); }
  .core .note { color: var(--faint); display: block; font-size: 11px; line-height: 1.45; max-width: 44ch; }
  .rule { color: var(--cascade); }
  aside section { background: var(--panel); border: 1px solid var(--line2); border-radius: 8px;
                  padding: 14px 16px; margin-bottom: 16px; }
  aside h2 { margin: 0 0 10px; font-size: 12px; text-transform: uppercase; letter-spacing: .6px; color: var(--faint); font-weight: 500; }
  aside pre { margin: 0; font-family: var(--mono); font-size: 11.5px; line-height: 1.55; color: var(--dim);
              white-space: pre; overflow-x: auto; }
  aside p { margin: 0 0 8px; font-size: 12.5px; color: var(--dim); }
  aside p:last-child { margin-bottom: 0; }
  aside code { font-family: var(--mono); color: var(--fg); font-size: 12px; }
  aside .nouns { font-family: var(--mono); font-size: 11.5px; color: var(--fg); line-height: 1.9; word-spacing: 2px; }
  .legend { display: grid; gap: 7px; font-size: 12px; color: var(--dim); }
  .legend div { display: flex; gap: 9px; align-items: baseline; }
  table.mini { font-size: 12px; }
  table.mini td { padding: 4px 6px; border-bottom: 1px solid var(--line); color: var(--dim); vertical-align: top; }
  table.mini td:first-child { font-family: var(--mono); color: var(--fg); white-space: nowrap; }
  footer { padding: 0 24px 40px; color: var(--faint); font-size: 12px; line-height: 1.8; }
</style>
</head>
<body>
<header>
  <h1>属性改动 → 增量去向</h1>
  <div class="sub">搜一个 E3D 属性短名，看这一窗要不要重算；再填一个 noun，把 core 自己的执行粒度
    也一起定下来——<code>change/UPGRPH</code> 对类别 3 / 4 的分叉看的是元素形状，不是属性。
    每一列都从 <code>model_impact.rs</code>、<code>window_root_plan.rs</code> 与六份随源码入库的产物里现算，不是手抄的。</div>
  <div class="stats">
    <div class="stat"><b>__UNIVERSE__</b>可查属性</div>
    <div class="stat"><b>__NOUNS_N__</b>可查 noun</div>
    <div class="stat"><b>__PAIRS__</b>noun×属性对</div>
    <div class="stat"><b>__CURATED__</b>手写效果表</div>
    <div class="stat"><b>__DICT__</b>字典 DCHC</div>
    <div class="stat"><b>__PLCF__</b>PLCF=1</div>
    <div class="stat"><b>__READS__</b>生成侧读到</div>
    <div class="stat"><b>__REFS__</b>ELEMENT 引用</div>
    <div class="stat"><b>__SCHEMA__</b>运行库 schema</div>
  </div>
</header>
<main>
  <div>
    <div class="bar">
      <input type="search" id="q" placeholder="属性名，如 DIAM / POS / SUPP / CACHID / NGMR" autofocus spellcheck="false">
      <input type="search" id="noun" list="nounlist" placeholder="noun（可空），如 EQUI / BOX / DOOR" spellcheck="false">
      <datalist id="nounlist"></datalist>
      <label class="toggle"><input type="checkbox" id="gate2"> 未读属性门（默认关）</label>
    </div>
    <div class="bar" id="filters">
      <span class="chip on" data-v="">全部</span>
      <span class="chip" data-v="Regen">重算</span>
      <span class="chip" data-v="Cascade">重算+级联</span>
      <span class="chip" data-v="Transform">仅位姿</span>
      <span class="chip" data-v="Skip">不算</span>
      <span class="chip" data-v="Unknownish">兜底 Unknown</span>
      <span class="chip" data-v="Divergent">我们比 core 多算</span>
    </div>
    <div id="nouncard"></div>
    <p class="count" id="count"></p>
    <table>
      <thead><tr><th>属性</th><th>我方去向</th><th>core 执行（UPGRPH）</th><th>效果表</th><th>DCHC</th><th>生成侧</th><th>判定依据</th></tr></thead>
      <tbody id="rows"></tbody>
    </table>
  </div>
  <aside>
    <section>
      <h2>一条属性 put 在 core 里走的门</h2>
      <pre>DB_Element::putAtt
 ├ 同值早退                     internalPutAtt
 ├ 字典 <b>WNOEVT</b> 为真 → 不发事件   ← 仅 __WNOEVT_N__ 个非模型属性
 ├ DESDRA_SCPlugs::PostSetAttribute
 └ descases/VDESPT  0x101F33A9
    ├ PLCDEL / GTPINI / SCSPOL   门外，照跑
    ├ <b>DCHC == 0 → 到此为止</b>（几何不动）
    ├ 规则 U/V/W/X（noun×属性专项）
    └ EVALAT → QCHGLS → UPGRPH</pre>
      <p style="margin-top:10px">`WNOEVT`（字典字段 <code>299311034</code>，<code>DB_Attribute::ReadData</code>
        <code>core.dll 0x58CE1F0</code> 读进 <code>+184</code>，<code>wnoevt() 0x58D5290</code> 取用）是
        <b>DCHC 之前的一道门</b>：置真的属性连变更事件都不发。现已从 <code>attlib.dat</code>
        <b>离线解出</b>（<code>output/gen_wnoevt_flags.py</code>，DCHC 4270/4270 · PLCF 58/58 逐条对齐校准）：
        全字典只有 <b>__WNOEVT_N__ 个</b>属性置了它——<span class="nouns">__WNOEVT_LIST__</span>——
        全是激光 / 照明 / 温度属性，<b>没有一个在模型生成的属性集里</b>。也就是说本页覆盖的每个属性
        <code>WNOEVT</code> 都为 0，这道门对增量判决永不触发；命中这 __WNOEVT_N__ 个之一时行上会打 <code>WNOEVT</code> 徽章。</p>
    </section>
    <section>
      <h2>我方决策链</h2>
      <pre>normalize(name)
 │  去 att. 前缀 / 限定段 / 大写
 ├─ 门1  效果表=DataOnly ∧ DCHC=0   → 不算
 ├─ 门2  DCHC=0 ∧ 生成侧不按名读     → 不算  [默认关]
 └─ 查 196 名效果表
    ├─ DataOnly 没过门1  → 重算
    ├─ TransformOnly     → 仅位姿 †
    ├─ DirectGeometry    → 重算
    ├─ DependencyCascade → 重算+级联
    ├─ Structural        → 重算
    └─ 没命中
       ├─ PARA* 前缀     → 重算（几何）
       ├─ att_type=ELEMENT → 重算+级联（A2）
       └─ 其余 / UDA:&lt;id&gt; → 重算（Unknown）</pre>
      <p style="margin-top:10px">† 仅位姿还要过刚体前移的排除：P23 / 点环容器 / 路由容器 / 负体 / 桥架件
        全部退回重算。填了 noun 之后这一档就是确定的，不再打星。</p>
    </section>
    <section>
      <h2>DCHC 类别 → core 做什么</h2>
      <table class="mini"><tbody>
        <tr><td>0</td><td>什么都不做（<code>VDESPT</code> 就挡了）</td></tr>
        <tr><td>1</td><td>头端连通：规则 H 把<b>前一个元件</b>记类别 4，自身只 <code>COLRES</code></td></tr>
        <tr><td>2</td><td>尾端连通：规则 I 把<b>自身</b>记类别 4</td></tr>
        <tr><td>3</td><td>刚体位姿：只改段矩阵——除了下面四条例外</td></tr>
        <tr><td>4</td><td>几何 / 表示：重建，粒度按元素形状分整件 / 单图元 / 全部</td></tr>
        <tr><td>—</td><td>字典没收（UDA、伪属性）：<code>ATAINT</code> 读失败 → 按 0 处理</td></tr>
      </tbody></table>
    </section>
    <section>
      <h2>P23 · 类别 3 也整件重建的 23 个 noun</h2>
      <p>几何由摆放派生的族：门窗在墙上开孔、栏杆立柱随相邻件、标注随被标对象。
        它们的 <code>POS</code>/<code>ORI</code> 变化等于几何变化，core 与我们都整件重建。
        名单是 <code>UPGRPH 0x1022EEF6–0x1022F05B</code> 那 23 条 <code>cmp eax, imm32</code> 逆 hash 来的。</p>
      <div class="nouns">__P23__</div>
    </section>
    <section>
      <h2>这张表管不到的四件事</h2>
      <p>去向是<b>整个元素</b>的一句话，下面任一成立都直接重算，属性表不再有发言权：</p>
      <p><code>type_changed</code> · <code>owner_changed</code> · <code>members</code>（含 children 顺序）· <code>opaque</code></p>
      <p>另外：一个属性差异都没检测到（非 detail 解析）也按 Unknown 重算。跨会话的 GETWORK 路
        <b>完全不分属性</b>——记录被重写就 <code>FNDTOP</code> 折到顶层件整件重画，改 <code>NAME</code> 也一样。</p>
    </section>
    <section>
      <h2>去向</h2>
      <div class="legend">
        <div><span class="tag t-regen">重算</span><span>整根重生成几何</span></div>
        <div><span class="tag t-cascade">重算+级联</span><span>还要反查目录/邻居</span></div>
        <div><span class="tag t-transform">仅位姿</span><span>只更新 world transform</span></div>
        <div><span class="tag t-skip">不算</span><span>不上卷、不级联、不重算</span></div>
      </div>
    </section>
    <section>
      <h2>口径</h2>
      <p>「生成侧读到」= <code>e3d-model</code> 与 <code>gen-model + e3d-io</code> 两份机扫大写字面量的并集，
        宁宽勿漏。字典里没有的名字（UDA、伪属性）DCHC 记 <code>—</code>，它和 0 不是一回事：<code>CACHID</code>
        就卡在这里。</p>
      <p>noun 的形状位来自字典：<code>PRMF</code>→<code>LPRMTV</code>、<code>TOPF</code>→<code>LISTOP</code>、
        <code>POPF</code>→<code>LPOINT</code>、<code>PIPC</code>→<code>IPCOMP</code>、<code>HNGC</code>→<code>IHCOMP</code>、
        <code>CATC/GMSC/NGMC/GMTC/PLNC/PTSC</code>→REPALL 族。</p>
      <p><b>top == ref 是拿 <code>TOPF</code> 近似的。</b>真正的 <code>FNDTOP</code> 看的是元素在树里的位置，
        本页只有 noun。<code>EVALAT</code> 的规则里，只按 noun 就判得了的三条已经建模：R（<code>PLOPEN</code>/<code>DP*</code>
        → 顶层件）、S（点类 → 所属几何件）、P（管件 → 分支）。规则 J/M/N/O/Q/T（孔洞 → owner、被
        PANE/GENSEC/SCTN/BPANEL 消费的成员 → 结构件）要祖先链才判得了，本页<b>不模拟</b>——
        在这些族上 core 只会比这里显示的<b>更粗</b>，不会更细。</p>
    </section>
  </aside>
</main>
<footer>生成于 __STAMP__ · <code>output/gen_attribute_verdict_explorer.py</code> · 数据随源码入库，重跑脚本即刷新<br>
core 侧取证：E3D 3.1 <code>Core3D.dll</code> / <code>core.dll</code>，ida-bridge 活桥。
<code>UPGRPH 0x1022EAEE</code>（分派骨架逐指令核）· <code>VDESPT 0x101F33A9</code>（DCHC 门逐指令核）·
<code>IDCHNG 0x1022E302</code>（全函数逐指令核）· <code>DB_Attribute::ReadData 0x58CE1F0</code>（WNOEVT/DCHC 字段位）。</footer>
<script>
const ROWS = __PAYLOAD__;
const NOUNS = __NOUNS__;
const VDESPT = __VDESPT__;
const WNOEVT = new Set(__WNOEVT__);
const TABLES = ["DataOnly","StructuralMembership","TransformOnly","DependencyCascade","DirectGeometry"];
const TABLE_CONST = ["DATA_ONLY","STRUCTURAL","TRANSFORM_ONLY","DEPENDENCY_CASCADE","DIRECT_GEOMETRY"];
const B = { PRIMITIVE:1, TOPLEVEL:2, POINT:4, GEOMSET:8, EXTRUSION:16, P23:32, IPCOMP:64, IHCOMP:128, REPALL:256 };
// EVALAT 规则 R：这几个 noun 上的任意类别 3/4 变化都经 FNDTOP 折到顶层件。
const RULE_R_NOUNS = new Set(["PLOPEN","DPCA","DPCY","DPSP","DPSE"]);

// noun -> { bits, gb, set:Set(attrIndex) }，增量 base36 解码。
const NOUN = new Map();
for (const [name, bits, packed, gb] of NOUNS) {
  const set = new Set();
  let cur = 0;
  if (packed) for (const d of packed.split(".")) { cur += parseInt(d, 36); set.add(cur); }
  NOUN.set(name, { bits, gb, set });
}

// ── core 侧：change/UPGRPH 对 (cls, noun) 的执行 ──────────────────────────────
function coreAction(cls, noun) {
  if (cls < 0) return { op: "不动", note: "字典没收这个名字，IDCHNG 的 ATAINT 读失败 → 类别 0", rebuild: false };
  if (cls === 0) return { op: "不动", note: "VDESPT 的 DCHC 门就挡了；PLCDEL / 自动着色仍跑，几何一动不动", rebuild: false };
  if (cls === 1) return { op: "前一元件整件重建", note: "规则 H：CRETUR + NATTA(PREV) 记 (prev, 4)；自身只 COLRES", rebuild: true };
  if (cls === 2) return { op: "自身重建", note: "规则 I：记 (self, 4)，再按下面类别 4 的粒度执行", rebuild: true };
  const c = noun ? NOUN.get(noun) : null;
  // EVALAT 的规则表在 UPGRPH 之前跑，命中就把目标元素换掉，UPGRPH 再也看不到原 noun。
  if (c && RULE_R_NOUNS.has(noun))
    return { op: "顶层件整件重建", note: "EVALAT 规则 R：PLOPEN / DP* 经 FNDTOP 折到顶层件，记类别 4", rebuild: true };
  if (c && (c.bits & B.POINT))
    return { op: "所属几何件重建", note: "EVALAT 规则 S：点类经 FNDEQP 折到所属几何件，记类别 4", rebuild: true };
  if (cls === 3) {
    if (!c) return { op: "只改段矩阵 ·分叉", note: "四条例外（P23 / 点类 / 管件 / 规则 M・J）要看元素形状，填个 noun 就定死", rebuild: null };
    if (c.bits & B.P23) return { op: "整件重建", note: "P23：几何由摆放派生，ADDREM(top, REPELE) + 两层成员 RMKELE", rebuild: true };
    if (c.bits & (B.IPCOMP | B.IHCOMP))
      return { op: "段矩阵 + 两侧管段重建", note: "SEGTRA 自身，SADREP 前后两段隐含管段；规则 P 另记 (BRAN, 4) 整根", rebuild: true };
    if (c.bits & B.TOPLEVEL) return { op: "只改段矩阵（整棵子树）", note: "top == ref：TRASCN 给子树里每个 TOPF 后代 STRTRA，再 STRTRA 自身", rebuild: false };
    if (c.bits & B.PRIMITIVE) return { op: "只改这个图元的段矩阵", note: "LPRMTV：SEGTRA(ref)，一个图元一段", rebuild: false };
    return { op: "子树 TOPF 各改段矩阵", note: "容器：TRASCN——这就是 core 自己的刚体前移", rebuild: false };
  }
  if (cls === 4) {
    if (!c) return { op: "重建 ·分叉", note: "整件 / 单图元 / 全部三种粒度要看元素形状，填个 noun 就定死", rebuild: true };
    if (c.bits & B.REPALL) return { op: "全部重画", note: "目录件 / 几何集：REPALL()，整张画列表", rebuild: true };
    if (c.bits & B.TOPLEVEL) return { op: "整件重建", note: "top == ref：REPELE = TOPDEL 删段 + ADDDES 重新 ELMODL", rebuild: true };
    if (c.bits & B.PRIMITIVE)
      return (c.bits & (B.IPCOMP | B.IHCOMP))
        ? { op: "单图元 + 两侧管段重建", note: "SADREP ×3：自身与前后两段隐含管段", rebuild: true }
        : { op: "单图元重建", note: "LPRMTV：SADREP(top, ref) → SEGREP，只重建这一个图元的段", rebuild: true };
    return { op: "子树只改段矩阵", note: "容器上的类别 4（如 ZONE 的 LEVE）：TRASCN，不重建几何", rebuild: false };
  }
  return { op: "—", note: "", rebuild: null };
}

// ── 我方：两道门 + 效果表 + 刚体前移排除 ──────────────────────────────────────
function verdictOf(row, gate2, noun) {
  const [name, code, idx, read, ref] = row;
  let base = idx >= 0 ? TABLES[idx] : (name.startsWith("PARA") ? "DirectGeometry" : null);
  const why = [];
  let eff = base;
  if (base === null) {
    eff = ref ? "DependencyCascade" : "Unknown";
    why.push(ref ? "A2：<b>att_type=ELEMENT</b>，未命中清单的引用属性自动升级为级联"
                 : "未命中任何清单，<b>保守</b>按 Unknown 处理");
  } else if (idx < 0) {
    why.push("<b>PARA*</b> 前缀，按几何输入归类");
  } else {
    why.push("效果表 <b>" + TABLE_CONST[idx] + "</b>");
  }
  if (base === "DataOnly" && code === 0) return { v: "Skip", eff, tier: "MetadataOnly",
    why: "门1 两票齐：效果表 DataOnly <b>∧</b> 字典 DCHC=0" };
  if (base === "DataOnly") why.push("字典<b>没投这一票</b>（DCHC " + (code < 0 ? "缺席" : code) + "），门1 不放行");
  if (code === 0 && !read) {
    if (gate2) return { v: "Skip", eff, tier: "UnreadAttribute",
      why: "门2 两票齐：字典 DCHC=0 <b>∧</b> 生成侧没有一处按名字读它" };
    why.push("门2 若打开可放行（DCHC=0 ∧ 无人读），当前<b>关着</b>");
  }
  let v = "Regen";
  let star = false;
  if (eff === "TransformOnly") {
    v = "Transform";
    const c = noun ? NOUN.get(noun) : null;
    if (c) {
      if (c.bits & B.P23) { v = "Regen"; why.push("刚体前移排除：<b>P23</b> 摆放派生 noun，退回重算"); }
      else if (c.bits & B.POINT) { v = "Regen"; why.push("刚体前移排除：<b>点 / 环容器</b>，退回重算"); }
      else if (c.bits & (B.IPCOMP | B.IHCOMP)) { v = "Regen"; why.push("刚体前移排除：<b>路由 / 支吊架元件</b>，整根重算"); }
      else why.push("这个 noun 不在任何刚体前移排除名单里，位姿便宜路径可用");
    } else star = true;
  }
  else if (eff === "DependencyCascade") v = "Cascade";
  return { v, eff, tier: null, star, why: why.join(" · ") };
}

const TAG = { Regen: ["t-regen","重算"], Cascade: ["t-cascade","重算+级联"],
              Transform: ["t-transform","仅位姿"], Skip: ["t-skip","不算"] };
const EFFTAG = { DataOnly: "t-skip", TransformOnly: "t-transform", DirectGeometry: "t-regen",
                 DependencyCascade: "t-cascade", StructuralMembership: "t-regen", Unknown: "t-unknown" };
const CAPLABEL = [[B.TOPLEVEL,"顶层件 TOPF"],[B.PRIMITIVE,"图元 PRMF"],[B.POINT,"点类 POPF"],
  [B.GEOMSET,"几何集 GMSC"],[B.EXTRUSION,"拉伸体"],[B.P23,"P23 摆放派生"],[B.IPCOMP,"管件 PIPC"],
  [B.IHCOMP,"支吊架元件 HNGC"],[B.REPALL,"REPALL 族"]];
const LIMIT = 250;
let filter = "";

function currentNoun() {
  const raw = document.getElementById("noun").value.trim().toUpperCase();
  return NOUN.has(raw) ? raw : "";
}

function renderNounCard(noun) {
  const box = document.getElementById("nouncard");
  const typed = document.getElementById("noun").value.trim();
  if (!noun) {
    box.innerHTML = typed
      ? `<div class="nouncard">字典里没有 <b>${typed.toUpperCase()}</b> 这个 noun，先按「不限 noun」显示。</div>`
      : "";
    return;
  }
  const c = NOUN.get(noun);
  const caps = CAPLABEL.filter(([bit]) => c.bits & bit).map(([, l]) => `<span class="tag t-mute">${l}</span>`);
  if (!caps.length) caps.push('<span class="tag t-mute">容器 / 无几何位</span>');
  box.innerHTML = `<div class="nouncard">已锁定 <b>${noun}</b>：schema 里 ${c.set.size} 个属性，
    graphicsBehaviour = <b>${c.gb}</b>。core 的类别 3 / 4 分叉已按它定死。
    <div class="caps">${caps.join("")}</div></div>`;
}

function render() {
  const q = document.getElementById("q").value.trim().toUpperCase();
  const gate2 = document.getElementById("gate2").checked;
  const noun = currentNoun();
  renderNounCard(noun);
  const scope = noun ? NOUN.get(noun).set : null;
  // 查 DIAM 时 DIAM 自己得排在 NDIAMETER 前面：完全一致 → 前缀 → 子串。
  const hits = [];
  for (let i = 0; i < ROWS.length; i++) {
    if (scope && !scope.has(i)) continue;
    if (!q) { hits.push(i); continue; }
    const name = ROWS[i][0];
    const at = name.indexOf(q);
    if (at < 0) continue;
    hits.push(i);
  }
  if (q) hits.sort((a, b) => {
    const na = ROWS[a][0], nb = ROWS[b][0];
    const ra = na === q ? 0 : na.startsWith(q) ? 1 : 2;
    const rb = nb === q ? 0 : nb.startsWith(q) ? 1 : 2;
    return ra - rb || na.length - nb.length || (na < nb ? -1 : 1);
  });
  const out = [];
  let total = 0;
  for (const i of hits) {
    const row = ROWS[i];
    const r = verdictOf(row, gate2, noun);
    const core = coreAction(row[1], noun);
    if (filter === "Unknownish") { if (r.eff !== "Unknown") continue; }
    else if (filter === "Divergent") { if (!(core.rebuild === false && r.v === "Regen")) continue; }
    else if (filter && r.v !== filter) continue;
    total++;
    if (out.length >= LIMIT) continue;
    const [name, code, idx, read, ref, inSchema, plcfFlag] = row;
    const [cls, label] = TAG[r.v];
    const special = noun && VDESPT[noun + "." + name];
    out.push(`<tr>
      <td><span class="name">${name}</span>${inSchema ? "" : ' <span class="tag t-mute">仅字典</span>'}${plcfFlag ? ' <span class="tag t-mute">PLCF</span>' : ""}${WNOEVT.has(name) ? ' <span class="tag t-mute">WNOEVT</span>' : ""}</td>
      <td><span class="tag ${cls}">${label}${r.star ? " *" : ""}</span>${r.tier ? ` <span class="tag t-mute">${r.tier}</span>` : ""}</td>
      <td class="core"><span class="op">${core.op}</span><span class="note">${core.note}</span>${
        special ? `<span class="note rule">VDESPT 规则 ${special[0]}：${special[1]}</span>` : ""}</td>
      <td><span class="tag ${EFFTAG[r.eff]}">${r.eff}</span></td>
      <td><span class="tag t-mute">${code < 0 ? "—" : code}</span></td>
      <td><span class="tag ${read ? "t-regen" : "t-mute"}">${read ? "读" : "不读"}</span>${ref ? ' <span class="tag t-cascade">ref</span>' : ""}</td>
      <td class="why">${r.why}</td></tr>`);
  }
  document.getElementById("rows").innerHTML = out.join("") ||
    '<tr><td colspan="7" class="why">没有匹配的属性名。</td></tr>';
  document.getElementById("count").textContent =
    total + " 个匹配" + (total > LIMIT ? `，只画前 ${LIMIT} 行` : "") +
    (noun ? ` · 限定在 ${noun} 上` : " · 未限定 noun，位姿档打 * 表示还要看元素形状") +
    (gate2 ? " · 未读属性门已打开" : "");
}

document.getElementById("nounlist").innerHTML =
  NOUNS.map(([n]) => `<option value="${n}">`).join("");

// 整页状态进 hash，便于把某一格结论直接贴给别人：#q=POS&noun=EQUI&gate2=1&f=Transform
function readHash() {
  const p = new URLSearchParams(location.hash.slice(1));
  if (p.has("q")) document.getElementById("q").value = p.get("q");
  if (p.has("noun")) document.getElementById("noun").value = p.get("noun");
  document.getElementById("gate2").checked = p.get("gate2") === "1";
  filter = p.get("f") || "";
  document.querySelectorAll("#filters .chip").forEach(c =>
    c.classList.toggle("on", c.dataset.v === filter));
}
function writeHash() {
  const p = new URLSearchParams();
  const q = document.getElementById("q").value.trim();
  const n = document.getElementById("noun").value.trim();
  if (q) p.set("q", q);
  if (n) p.set("noun", n);
  if (document.getElementById("gate2").checked) p.set("gate2", "1");
  if (filter) p.set("f", filter);
  const h = p.toString();
  history.replaceState(null, "", h ? "#" + h : location.pathname);
}
function update() { writeHash(); render(); }

document.getElementById("q").addEventListener("input", update);
document.getElementById("noun").addEventListener("input", update);
document.getElementById("gate2").addEventListener("change", update);
document.querySelectorAll("#filters .chip").forEach(chip => chip.addEventListener("click", () => {
  document.querySelectorAll("#filters .chip").forEach(c => c.classList.remove("on"));
  chip.classList.add("on");
  filter = chip.dataset.v;
  update();
}));
window.addEventListener("hashchange", () => { readHash(); render(); });
readHash();
render();
</script>
</body>
</html>
"""

page = (
    TEMPLATE.replace("__PAYLOAD__", json.dumps(rows, ensure_ascii=False, separators=(",", ":")))
    .replace("__NOUNS__", json.dumps(noun_rows, ensure_ascii=False, separators=(",", ":")))
    .replace(
        "__VDESPT__",
        json.dumps(
            {f"{n}.{a}": v for (n, a), v in VDESPT_RULES.items()},
            ensure_ascii=False,
            separators=(",", ":"),
        ),
    )
    .replace("__WNOEVT__", json.dumps(sorted(wnoevt), ensure_ascii=False, separators=(",", ":")))
    .replace("__WNOEVT_LIST__", " ".join(sorted(wnoevt)))
    .replace("__WNOEVT_N__", str(len(wnoevt)))
    .replace("__P23__", " ".join(p23))
    .replace("__STAMP__", datetime.now().strftime("%Y-%m-%d %H:%M"))
    .replace("__UNIVERSE__", str(stats["universe"]))
    .replace("__NOUNS_N__", str(stats["nouns"]))
    .replace("__CURATED__", str(stats["curated"]))
    .replace("__DICT__", str(stats["dict"]))
    .replace("__PLCF__", str(stats["plcf"]))
    .replace("__READS__", str(stats["reads"]))
    .replace("__REFS__", str(stats["refs"]))
    .replace("__SCHEMA__", str(stats["schema"]))
    .replace("__PAIRS__", str(stats["pairs"]))
)
with open("output/attribute-verdict-explorer.html", "w", encoding="utf-8", newline="\n") as f:
    f.write(page)
print("written: output/attribute-verdict-explorer.html")
