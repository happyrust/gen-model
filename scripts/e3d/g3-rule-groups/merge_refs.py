"""把 refs27 的 Q REF 输出并进夹具：给每行加 `e3d_member_refnos`。

E3D 的 refno 形式是 `=2013286676/490`（ref0/ref1），仓内 RefU64 的字符串形式是
`2013286676_490`。这里统一转成后者，测试直接按字符串比。
"""
import json
import pathlib
import re

HERE = pathlib.Path(__file__).resolve().parent
# 仓根 = scripts/e3d/g3-rule-groups 往上三级。
REPO = HERE.parents[2]
FIXTURE = REPO / "tests/fixtures/group-contract/e3d-rule-groups.json"
LOG = REPO / "output/live-rule-groups/run2/check-driver.log"

text = LOG.read_text(encoding="utf-8-sig").replace("\r\n", "\n")
blocks = re.split(r"^== REFS (\S+)$", text, flags=re.M)
refnos_of = {}
for i in range(1, len(blocks), 2):
    name, body = blocks[i], blocks[i + 1]
    refnos = []
    for line in body.split("\n"):
        line = line.strip()
        match = re.match(r"^Ref =(\d+)/(\d+)$", line)
        if match:
            refnos.append(f"{match.group(1)}_{match.group(2)}")
    refnos_of[name] = refnos

REFS_NOTE = (
    " 每个成员另有 `Q REF` 取到的 refno（`e3d_member_refnos`，形式 ref0_ref1），"
    "用于与 Rust `evaluate` 的 members 做集合相等比对。"
)

doc = json.loads(FIXTURE.read_text(encoding="utf-8"))
for row in doc["rows"]:
    row["e3d_member_refnos"] = refnos_of[row["name"]]
    assert len(row["e3d_member_refnos"]) == len(row["e3d_members"]), row["name"]

# 幂等：重跑不能把同一段说明反复追加。
doc["note"] = doc["note"].replace(REFS_NOTE, "") + REFS_NOTE
doc["refs_log"] = "output/live-rule-groups/run2/check-driver.log"
FIXTURE.write_text(
    json.dumps(doc, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n"
)
print("updated", FIXTURE)
total = sum(len(row["e3d_member_refnos"]) for row in doc["rows"])
print("成员 refno 合计", total)
for row in doc["rows"]:
    if row["e3d_member_refnos"]:
        print(f"  {row['name']:<16} {row['e3d_member_refnos']}")
