# OCC 退役 dabacon 源原语普查与生产网格验证（2026-08-24）

## 口径

- 直接读取 E3D 2.1/3.1 安装项目的 dabacon `*_0001`，在 `inst_geo` 规范化之前读取属性。
- 每个输入文件记录 SHA-256、dbnum、refno、owner、noun 与几何尺寸。
- SNOU/SLCY 等直接通过 `create_brep_shape → convert_to_unit_param →
  tessellate_libgm_param`；POHE 按 dabacon 成员树组装 POIN/POLOOP/LOOPTS 后进入同一生产
  tessellator。缺成员、缺坐标、缺索引均为错误，不使用默认值或近似形状。

## 命令与结果

定向单测：

```text
cargo test --locked --lib --no-default-features --features ws,gen_model,manifold,project_hd source_primitive_census -- --nocapture
cargo test: 3 passed, exit 0
```

POHE 现场闭包：

```text
D:\Rust\target\debug\occ_retire_census.exe --root D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000 --nouns POHE,POLYHE --validate-mesh --out docs\evidence\2026-08-24-occ-retire-source-census\E3D3.1-AvevaPlantSample-polyhedron-validated.json
files=235 indexed_elements=4164813 samples=4 counts={"POHE": 4}
exit 0
```

2.1 + 3.1 安装项目汇总（1,748 个源文件，35,133,637 个索引元素）：

```text
SLCY=3056
SNOU=2031
NSNO=18
POHE=1448
SNOU/NSNO nonzero YOFF=422
SPHE=0
```

仅 E3D 3.1 汇总：SLCY=2,782、SNOU=1,035、NSNO=10、POHE=1,350，非零 YOFF=214。

## 代表样本

### YOFF Snout

```text
file=D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000\acp7015_0001
sha256=81bbacbbb5d272b3c6e90342e240ebc8ef93f98e3b8098f444049a3eb7101ca7
dbnum=7015 refno=pe:15207_10558 noun=SNOU
DBOT=2000 DTOP=700 HEIG=1200 XOFF=0 YOFF=650
mesh: vertices=404 triangles=400 signed_volume=1849180847.5155287
aabb_min=[-1000,-1325,-600] aabb_max=[1000,675,600]
```

Y 向 AABB 与底/顶圆心 `-325/+325` 一致，验证了 `±YOFF/2` 的绝对摆位。

### SSCL/SLCY

```text
file=D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000\aps250154_0001
sha256=a780eb152da5439940888f9f3bb67644690239a2f353f666200ecf45964dd710
dbnum=250154 refno=pe:2013286698_1891 noun=SLCY DIAM=25 HEIG=250
```

E3D 3.1 Catalogue + PlantSample 的直接可网格化样本共 78 个，78/78 通过；其中非零 YOFF
样本 5/5 通过，全部有合法索引、有限坐标/法线、非退化 AABB 与正有向体积。

### POHE

```text
file=D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000\aps7200_0001
source_sha256=b7dc7a204619c9a645b46fef07254fe356319a6a62b18cd8ab917e6740f18922
evidence_sha256=0591138b654f3b35cf79397315992df1005ed00e9ec305334daf147c66f45772
```

4/4 个 POHE 均通过。三角形数为 20、20、12、12；有向体积均为正，范围从
84,817,895.89217119 到 11,276,083,741.455078。

## 裁决

- YOFF、SSCL/SLCY、多面体的“真实 dabacon 样本”前置已满足；这只是源到生产网格验证，
  不替代 RVM 双向距离门。
- 已扫描的 35,133,637 个元素中没有 SPHE。球体现场样本与 E3D RVM 尚缺，因此 FR-016
  的最终发布停止条件仍生效。
- T046–T049、8009/7997 双副本、caliber 原子重建和维护窗口均尚未执行。
