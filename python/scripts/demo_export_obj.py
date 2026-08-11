# -*- coding: utf-8 -*-
"""示范 3：按名字定位构件并导出 OBJ 目视检查（生成结果对不对，肉眼看）。

场景：怀疑某构件生成的网格不对。两步——`db.by_name` 拿 refno，
`model.export_obj` 把它（含子树全部实例）变换到世界坐标导出 OBJ，
拖进任意查看器（Windows 3D 查看器 / Blender / MeshLab）就能看。

用法：.venv\\Scripts\\python.exe scripts\\demo_export_obj.py <元素名> [输出目录]
例：   ... demo_export_obj.py /-RX-CUP-001FA .scratch/obj
"""

import sys
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

REPO_ROOT = Path(__file__).resolve().parents[2]

import aios_db

aios_db.set_config(str(REPO_ROOT / "DbOption"))
aios_db.connect(cwd=str(REPO_ROOT))

if len(sys.argv) < 2:
    print(__doc__)
    sys.exit(2)
name = sys.argv[1]
out_dir = sys.argv[2] if len(sys.argv) > 2 else ".scratch/obj"

refnos = aios_db.db.by_name(name)
if not refnos:
    print(f"找不到名为 {name!r} 的元素")
    sys.exit(1)
if len(refnos) > 1:
    print(f"名字不唯一（{len(refnos)} 个）：{refnos}，逐个导出")

for refno in refnos:
    try:
        result = aios_db.model.export_obj(refno, out_dir)
    except RuntimeError as error:
        print(f"{refno}: {error}")
        continue
    for f in result["files"]:
        print(f"{f['refno']}: {f['path']}  insts={f['exported_insts']}/{f['insts']} "
              f"tris={f['triangles']}")
    if result["missing_meshes"]:
        print(f"  缺 mesh 文件 {len(result['missing_meshes'])} 个"
              f"（没生成过？先 aios_db.model.ensure({refno!r})）")
