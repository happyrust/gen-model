# -*- coding: utf-8 -*-
"""示范 2：单元素「文件 vs 库」一致性 + 历史回放（增量调试的第一现场）。

场景：某元素怀疑没同步对。三步定位——
1. `parse.element` 从库文件直读它现在/某历史会话的属性（e3d-io 底座，与生产收集器同一读法）；
2. `db.pe` 看库里这行现在长什么样（解析入库后的形态）；
3. `parse.net_window` 圈出它在窗口内的净变化（old-pdms-io 的逐会话回放 2026-09-04 已删）。

用法：.venv\\Scripts\\python.exe scripts\\demo_element_diff.py <库文件> <refno> [start] [end]
例：   ... demo_element_diff.py D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams7997_0001 24381_100677 100 102
"""

import json
import sys
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

REPO_ROOT = Path(__file__).resolve().parents[2]

import aios_db

aios_db.set_config(str(REPO_ROOT / "DbOption"))

if len(sys.argv) < 3:
    print(__doc__)
    sys.exit(2)
db_file, refno = sys.argv[1], sys.argv[2]

# 1. 文件侧：最新版本（最新索引里没有 = 元素已被删，本身就是重要结论）
try:
    ele = aios_db.parse.element(db_file, refno)
    print(f"== 文件侧（钉在会话 {ele['sesno']}）==")
    print(f"{ele['noun']} {ele['name']!r} owner={ele['owner']} children={len(ele['children'])}")
    print(json.dumps(ele["attrs"], ensure_ascii=False, indent=1)[:600])
except RuntimeError as error:
    print(f"== 文件侧 ==\n最新索引里没有它（{error}）——元素可能已被删除；"
          f"可用 sesno= 参数读历史版本")

# 2. 库侧：pe 行
aios_db.connect(cwd=str(REPO_ROOT))
pe = aios_db.db.pe(refno)
print("\n== 库侧（pe 行）==")
if pe is None:
    print("pe 行不存在——文件里有而库里没有，就是没入库/没跟上")
else:
    print(f"noun={pe.get('noun')} name={pe.get('name')!r} sesno={pe.get('sesno')} "
          f"deleted={pe.get('deleted')}")

# 3. 窗口内它的变更轨迹
if len(sys.argv) >= 5:
    start, end = int(sys.argv[3]), int(sys.argv[4])
    window = aios_db.parse.net_window(db_file, start, end, detail=True)["window"]
    print(f"\n== 净变化（{start}..{end}，每 refno 一条、挂 last-touch 会话）==")
    hits = 0
    for sesno, ops in sorted(window.items(), key=lambda kv: int(kv[0])):
        for op in ops:
            if op["refno"] != refno:
                continue
            hits += 1
            print(f"会话 {sesno}: {op['op']}")
            for key in ("added", "deleted", "modified"):
                if op.get(key):
                    print(f"  {key}: {json.dumps(op[key], ensure_ascii=False)[:300]}")
    if not hits:
        print("窗口内没有该元素的操作")
