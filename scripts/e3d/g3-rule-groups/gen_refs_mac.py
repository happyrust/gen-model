"""从 e3d-rule-groups.json 生成第二个只读宏：对 27 条规则的每个成员取 `Q REF`。

这样 E3D 侧的成员就能落到 refno 上，与 Rust `evaluate` 的 `members` 做**集合相等**比对
（契约 VAL-LIVE-005 的判定口径），而不是只比名字。

只读：无 SAVEWORK / NEW / DELETE / MOVE / COPY；无 PML 集合对象。
"""
import json
import pathlib

HERE = pathlib.Path(__file__).resolve().parent
# 仓根 = scripts/e3d/g3-rule-groups 往上三级。
REPO = HERE.parents[2]
FIXTURE = REPO / "tests/fixtures/group-contract/e3d-rule-groups.json"
OUT_MAC = HERE / "refs27.mac"
# 与 gen_mac.py 同理：宏与 `.log` 必须同目录同名（`e3d_query.rs:287`）。
LOG = (HERE / "refs27.log").as_posix()

doc = json.loads(FIXTURE.read_text(encoding="utf-8"))
lines = [
    "-- 只读宏：27 条规则组每个成员的 Q REF（拿 E3D 侧的 refno）。",
    "-- 无 SAVEWORK / NEW / DELETE / MOVE / COPY；无 PML 集合对象。",
    'ALPHA LOG "%s" OVER' % LOG,
    "$P REFS27-ALIVE",
]
count = 0
for row in doc["rows"]:
    lines.append("$P == REFS %s" % row["name"])
    for member in row["e3d_members"]:
        lines.append(member)
        lines.append("Q REF")
        count += 1
lines.append("$P REFS27-DONE")
lines.append("ALPHA LOG END")
# 宏写 CRLF，与仓内既有 `.mac` 一致（`.gitattributes:10` 的 `*.mac -text`）。
OUT_MAC.write_text(
    "\r\n".join(lines) + "\r\n", encoding="utf-8", newline=""
)
print("wrote", OUT_MAC, len(lines), "lines,", count, "members")
