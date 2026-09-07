"""Build a bounded, evidence-linked coverage report from saved snapshots."""
import json
from pathlib import Path
from equi8000_live import OUT,REPO
d=json.loads((OUT/'profiles-moved-result/types-comparison.json').read_text())
rows=[]
for r in d['comparisons']:
    noun=r['name'].replace('/CODEX_TYPES_','').replace('HOST_','').replace('_UPDATED','')
    rays=r.get('ray_probes',[])
    rows.append(f"| {noun} | {'通过' if r['passed'] else '差异待诊断'} | {r['relative_volume_error']*100:.6f}% | {r['bounds_error_mm']:.6f} | {sum(not p['passed'] for p in rays)}/{len(rays)} |")
text='''# AMS 8000 设备多类型增量测试（2026-09-05）

本轮扩展设备直接建模类型：22 类完成创建、尺寸/形状更新、改名、位移及删除测试。最终几何检查 14 类通过、8 类存在差异；22 个类型属性对象与 8 个多面体顶点共 30 组数据检查全部通过。POHE 尝试创建失败，未完成有效模型测试。不能称为“所有类型全部通过”。

## 环境与边界

- 真实 RocksDB：SurrealDB 2.1.4，18049；模型 API：18050；命名空间 `rocks8000_20260905`。
- 独立 AMS 项目副本，8000 库；设备 `/1-LNR-Q005-PJ`（24384/24776）。不修改共享原始 AMS 项目。
- 临时对象前缀 `/CODEX_TYPES_`，已删除，模型恢复为原始 BOX，恢复结果与 RVM 一致。
- 本清单覆盖设计设备直接基本体、负体、拉伸、旋转和现代多面体。BOX/NBOX 已在前两轮完成。本轮未覆盖 A* 目录对应类型、42 个几何集 noun、目录管嘴/型材及路由系统；不能将正体测试外推为这些类型的验证。APOLYH/AREVO 在当前源码明确标为 Unsupported。

## 最终几何对比

逐件匹配 E3D RVM 与生产 PlantMesh，应用接口实例和世界变换。容差：相对体积误差 1%、局部边界误差 2 mm。负体另在母体局部 XY 平面取 6×6 条 Z 向射线，对比命中数量和命中位置（2 mm 容差），避免孔洞错位但外包围盒、体积相同的漏检。

PYRA 的 RVM 提取面片存在混合绕向，直接有向体积会抵消；该凸体用内部点分解四面体计算体积。早期 PYRA 大体积差异属于比较器问题，最终已通过。曲面网格分段不同可能造成体积及射线位置差异，下面的“待诊断”不是未经定位就断言布尔算法错误。

| 类型 | 最终结果 | 体积相对差 | 边界差 mm | 射线失败/总数 |
|---|---|---:|---:|---:|
'''+ '\n'.join(rows)+'''

## 数据增量与 Plant UI

数据源是 E3D 真正 SAVEWORK，不是直接改 SurrealDB。分别检查 API 的 e3d-io 读取值与 RocksDB noun 属性行：尺寸、改名、POS，以及多面体 4 个顶点的 POS。最终 30/30 组断言通过。底层属性仍有 `complete=false` 和未解码诊断，因此只证明本次明确检查的字段正确，不能推断所有属性已经解码。

Plant UI 连接独立实例，实际观察到：

- 圆柱名称更新为 `/CODEX_TYPES_CYLI_UPDATED`，DIAM=520、HEIG=600、POS=(120,0,0)。
- 拉伸体名称更新为 `/CODEX_TYPES_EXTR_UPDATED`，HEIG=700、POS=(120,11000,0)。
- 树中各类型及 `_UPDATED` 名称更新，完整设备范围加载 23 个模型（22 个测试母体/正体，加原始 BOX）。负体通过母体布尔结果显示。
- 模型更新事件自动清空并重建已显示范围。一次几何失败后 UI 留下空场景；后端恢复后曾需重新点击设备显示按钮。此恢复体验不能判为完全通过。
- UI 房间/PANEL 归属查询仍报错，未在本轮修复。

所有类型的属性值逐项验证来自 UI 所用 API 与 RocksDB 对照；并没有声称逐个类型的全部属性都在屏幕上人工核验过。

## 操作覆盖与异常

- 基本体：CYLI、CONE、PYRA、RTOR、CTOR、SNOU、DISH、SLCY，及 NCYL、NCON、NPYR、NRTO、NCTO、NSNO、NDIS、NSLC。
- 复杂体：EXTR/NXTR（HEIG 500→700）、REVO/NREV（ANGL 270→180）、POLYHE/NPOLYH（四个正 X 顶点 300→400，保持面平面性）。
- 每个成功创建的类型都执行局部 E 平移 120 mm 和加 `_UPDATED` 改名，最后删除。
- 环面 ANGL 180→240 的 E3D TTY 修改会退出 0xC0000005。拆到单类型仍复现。改用 ROUT 450→520 后四种环面尺寸修改成功；角度 240 的失败仍保留，未用替代用例抹掉。
- EXTR/REVO 用 PLOO/PAVE 创建失败；改用设计对象的 LOOP/VERT 结构后成功。属于测试对象构造修正。
- 初始 POLYHE 的 POLPTL 在面表之后，E3D 可导出 RVM，但当前模型读取器依赖首成员为 POLPTL，报 NotManifold，整个设备候选发布失败。将测试对象重建为顶点表在前后通过。保留原始失败现场；本轮没有修复读取器的顺序依赖。
- POHE 创建两种顶点写法都失败，未生成有效样本；后续修改/删除尝试也失败，未计入 22 类。
- 首批和复杂体新增时仍出现模型发布滞留。重启/按需生成与自动增量结果分别保存，未把重启后的结果算成首次自动新增通过。

## 证据目录

根目录：`output/rocksdb-8000-20260905/equi-increment/`。

- `types-created-result`、`types-resized-final`、`types-moved-result`：基本体各阶段快照。
- `profiles-created-observed`、`profiles-regenerated`：复杂体首次发布失败现场。
- `profiles-valid-result`、`profiles-resized-result`、`profiles-moved-result`：有效复杂体基线、尺寸变化和位移改名。
- 最终检查以 `profiles-moved-result/types-comparison.json` 为准；每个负体含射线坐标、两侧命中位置及判定。`all-attributes.json` 保存 API 和数据库属性原文。早期比较报告未包含后续新增的射线检查，不可代替最终报告。
- `types-restored-final`：删除后的模型、树、RocksDB 发布状态与 RVM。仅剩原始 BOX，网格世界边界误差约 0.00054 mm。
- 每次 E3D 操作独立保存 run.mac、tty.log、launcher.log，崩溃文件保留。`types-pre-retry`、`types-resized-result` 的源文件一致性断言失败，不作为有效最终对比证据。

## 可复用脚本与检查

`equi8000_types.py`、`equi8000_profiles.py` 构造/更新测试对象；`compare_equi8000_types.py` 读取真实网格并作离线对比，已存在的属性证据不重新抓取覆盖。脚本绑定本次独立 AMS 测试环境，阶段目录拒绝覆盖。

新增了一条几何失败明细诊断输出，定位到 POLYHE 24384/26318；没有修改几何生成算法。服务构建成功，脚本语法检查通过。

后续优先处理 CYLI 的 340 mm 网格边界偏差，再区分 CONE/SNOU/REVO 及四种负体的曲面离散精度与几何错误；之后补 POHE 的有效创建方法、目录族和几何集族用例。
'''
(REPO/'docs/rocksdb-8000-types-increment-20260905.md').write_text(text,encoding='utf-8')
print('report written')
