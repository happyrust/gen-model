"""把 E3D 只读宏（scripts/e3d/g3-rule-groups/live27.mac）的实跑日志解析成测试夹具。

输入：`--output` 运行目录里的 check-driver.log（默认 output/live-rule-groups/run1）
       tests/fixtures/group-contract/scosel-real-samples.json（name → refno）
输出：tests/fixtures/group-contract/e3d-rule-groups.json

用法：python scripts/e3d/g3-rule-groups/build_fixture.py [日志路径]

口径（library/e3d-reference.md §3）：
- 规则原文取 `Q ATT` 输出里的 `Scosel` 行，**原样保留**（整行，含 `Scosel` 前缀与
  `ZONE` 与 `EQ` 之间的两个空格）。
- 求值成员取 `Q ALL <noun> WITH (...)` 的输出，`None` 表示 0 命中。
"""
import json
import pathlib
import re
import sys

HERE = pathlib.Path(__file__).resolve().parent
# 仓根 = scripts/e3d/g3-rule-groups 往上三级。
REPO = HERE.parents[2]
# `--output` 指定的运行目录里，driver 会把 scenario log 存成 check-driver.log。
LOG = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else (
    REPO / "output/live-rule-groups/run1/check-driver.log"
)
SAMPLES = REPO / "tests/fixtures/group-contract/scosel-real-samples.json"
OUT = REPO / "tests/fixtures/group-contract/e3d-rule-groups.json"
MACRO = "scripts/e3d/g3-rule-groups/live27.mac"

text = LOG.read_text(encoding="utf-8-sig").replace("\r\n", "\n")
samples = json.loads(SAMPLES.read_text(encoding="utf-8"))["samples"]
refno_of = {s["name"]: s["refno"] for s in samples}

# 按 `== ATT nn /NAME` 切块；每块里取 Scosel 行。
att_blocks = re.split(r"^== ATT (\d+) (\S+)$", text, flags=re.M)
scosel_of = {}
for i in range(1, len(att_blocks), 3):
    num, name, body = att_blocks[i], att_blocks[i + 1], att_blocks[i + 2]
    match = re.search(r"^(Scosel.*)$", body, flags=re.M)
    if not match:
        raise SystemExit(f"{num} {name} 没有 Scosel 行")
    scosel_of[name] = match.group(1)

# 按 `== EVAL nn /NAME` 切块；成员行是 / 开头的非空行，`None` 表示空。
eval_blocks = re.split(r"^== EVAL (\d+) (\S+)$", text, flags=re.M)
members_of = {}
for i in range(1, len(eval_blocks), 3):
    num, name, body = eval_blocks[i], eval_blocks[i + 1], eval_blocks[i + 2]
    members = []
    for line in body.split("\n"):
        line = line.strip()
        if not line:
            continue
        if line == "None":
            continue
        if line.startswith("==") or line.startswith("$") or line.startswith("LIVE27"):
            break
        if line.startswith("/"):
            members.append(line)
    members_of[name] = members

rows = []
for name, scosel in scosel_of.items():
    rows.append(
        {
            "name": name,
            "refno": refno_of[name],
            "e3d_scosel": scosel,
            "e3d_members": members_of[name],
        }
    )

doc = {
    "source": "l3_suite --check-driver",
    "project": "AvevaPlantSample",
    "mdb": "ALL",
    "macro": MACRO,
    "log": LOG.relative_to(REPO).as_posix() if LOG.is_relative_to(REPO) else str(LOG),
    "note": (
        "E3D 自身对 27 条规则组的求值真值。规则原文取 `/<name>` 换行 + `Q ATT` 的 Scosel 行"
        "（原样，`ZONE` 与 `EQ` 之间是两个空格）；成员取 `Q ALL <noun> WITH (...)` 的输出，"
        "空列表表示 E3D 回 `None`（0 命中）。"
    ),
    "rows": rows,
}
OUT.write_text(json.dumps(doc, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")
print("wrote", OUT, len(rows), "rows")
for row in rows:
    print(f"  {row['name']:<16} {len(row['e3d_members']):>2} {row['e3d_scosel']}")
