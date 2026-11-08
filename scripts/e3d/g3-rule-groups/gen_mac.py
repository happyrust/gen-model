"""生成 E3D 只读宏：27 条规则组逐个取 Q ATT 原文 + Q ALL 求值成员。

只读：无 SAVEWORK / NEW / DELETE / MOVE / COPY，无 PML 集合对象。
ALPHA LOG 路径必须等于 <宏文件名>.log（同目录），否则 l3_suite 读不到 scenario log。
"""
import json
import pathlib

ROWS = [
    ("01", "/AREA01_PIPE", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA01PIPE'"),
    ("02", "/AREA01_CABLE", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA01CABLE'"),
    ("03", "/AREA01_CIVIL", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA01CIVIL'"),
    ("04", "/AREA01_EQUI", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA01EQUI'"),
    ("05", "/AREA01_ELEC", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA01ELEC'"),
    ("06", "/AREA01_HVAC", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA01HVAC'"),
    ("07", "/AREA01_STRUC", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA01STRUC'"),
    ("08", "/AREA01_SUPP", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA01SUPP'"),
    ("09", "/AREA01_GRID", "REFGRD", "ATTRIB DESC OF REFGRD EQ 'AREA01GRID' OR ATTRIB DESC OF REFGRD EQ 'PLANTGRID'"),
    ("10", "/AREA02_PIPE", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA02PIPE'"),
    ("11", "/AREA02_CABLE", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA02CABLE'"),
    ("12", "/AREA02_CIVIL", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA02CIVIL'"),
    ("13", "/AREA02_EQUI", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA02EQUI'"),
    ("14", "/AREA02_ELEC", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA02ELEC'"),
    ("15", "/AREA02_HVAC", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA02HVAC'"),
    ("16", "/AREA02_STRUC", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA02STRUC'"),
    ("17", "/AREA02_SUPP", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA02SUPP'"),
    ("18", "/AREA02_GRID", "REFGRD", "ATTRIB DESC OF REFGRD EQ 'AREA02GRID' OR ATTRIB DESC OF REFGRD EQ 'PLANTGRID'"),
    ("19", "/AREA03_PIPE", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA03PIPE'"),
    ("20", "/AREA03_CABLE", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA03CABLE'"),
    ("21", "/AREA03_CIVIL", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA03CIVIL'"),
    ("22", "/AREA03_EQUI", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA03EQUI'"),
    ("23", "/AREA03_ELEC", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA03ELEC'"),
    ("24", "/AREA03_HVAC", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA03HVAC'"),
    ("25", "/AREA03_STRUC", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA03STRUC'"),
    ("26", "/AREA03_SUPP", "ZONE", "ATTRIB DESC OF ZONE EQ 'AREA03SUPP'"),
    ("27", "/AREA03_GRID", "REFGRD", "ATTRIB DESC OF REFGRD EQ 'AREA03GRID' OR ATTRIB DESC OF REFGRD EQ 'PLANTGRID'"),
]

HERE = pathlib.Path(__file__).resolve().parent
# 仓根 = scripts/e3d/g3-rule-groups 往上三级。
REPO = HERE.parents[2]
# 宏与它的 `.log` **必须同目录同名**：`e3d_query::run_file` 把 scenario log 定成
# `macro_path.with_extension("log")`（`e3d_query.rs:287`），指到别的路径 driver 读不到，
# 报 `read E3D query log <路径>` 退出 1。`*.log` 已被 .gitignore 忽略，不入库。
LOG = (HERE / "live27.log").as_posix()

lines = [
    "-- 只读宏：27 条规则组的 E3D 原文（Q ATT）与求值成员（Q ALL <noun> WITH (...)）。",
    "-- 无 SAVEWORK / NEW / DELETE / MOVE / COPY；无 PML 集合对象（COLL + Size()）。",
    'ALPHA LOG "%s" OVER' % LOG,
    "$P LIVE27-ALIVE",
]
for num, name, noun, body in ROWS:
    lines.append("$P == ATT %s %s" % (num, name))
    lines.append(name)          # 两行式：先选中元素
    lines.append("Q ATT")       # 再单独一行 Q ATT，才有 Scosel 行
    lines.append("$P == EVAL %s %s" % (num, name))
    lines.append("Q ALL %s WITH ( %s )" % (noun, body))
lines.append("$P LIVE27-DONE")
lines.append("ALPHA LOG END")

# 宏写 CRLF：仓内既有 331 个 `.mac` 全是 CRLF（`.gitattributes:10` 的 `*.mac -text`
# 表示不做换行归一，存什么跑什么），LF 虽然实测也能被 des.exe 执行，但不跟既有对齐。
(HERE / "live27.mac").write_text(
    "\r\n".join(lines) + "\r\n", encoding="utf-8", newline=""
)
print("wrote", HERE / "live27.mac", len(lines), "lines")
