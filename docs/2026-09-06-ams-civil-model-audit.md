# AMS 土建模型 RVM 对拍与 plant-ui 验证（2026-09-06）

## 结论

本轮修正了 AMS 土建模型中两类确定性偏差：SBFI 的目录坐标系补全规则，以及短弧 WALL 的曲线离散与起点过渡面。修正后，E3D 导出的 `/1RS-WF03-W-C-RR001` RVM 中所有可与当前生成根逐项对应的土建实体均为 `exact`：GWALL 20/20、SBFI 47/47、STWALL 4/4、WALL 4/4。最大坐标轴包围盒差为 0.0071 mm，来自 GWALL；WALL 的最大差为 0.0014 mm。

对拍明细见 [wf03-rvm-compare-civil-final.json](../output/ams-civil-audit-20260906/wf03-rvm-compare-civil-final.json)。其中基线独有的 FIXI 38 个、PLDA 56 个，以及 56 个无法键合的具名组，是 RVM 中的结构子组，并非当前生成器输出的模型根，因此没有计入“实际生成土建实体全匹配”的分母。

## 算法修复

### SBFI 方向矩阵

IDA 对 `core.dll!ORIMAT`（`0x526D674`）的分析表明，E3D 先把 descriptor 的方向换成右手设计坐标 `Z=(raw.x,-raw.y,raw.z)`，随后按全局 Y、Z、X 的顺序选择第一个不与 Z 平行的候选轴，并计算：

```text
X = normalize(Z × candidate)
Y = Z × X
```

此前用“投影全局 Z”补全横轴，会在部分半球方向和轴向退化分支中翻转套管位置。实现位于 `e3d-model/src/elmodl.rs` 的 `sbfitting_alignment_from_zdir`，并增加普通方向与 `Z=-Y` 奇异分支回归测试。

### WALL 短弧曲面

RVM facet 证明 WALL 曲线使用 10 mm chord 网格：样本 WALL 2 为 15 个等分扫掠区间，WALL 4 为 3 个区间，而不是 RTOR 分析图元使用的 0.5 mm 网格。实现现复用 `negative_part_rev_segments(radius, sweep_deg)` 并把结果传给 `rect_torus_solid_with_segments`。

WALL 4 的 RVM 还多出一个起点三角过渡面。IDA 路径为：

- `MDR_WallVisualisationManager::getCSGTree`：`0x10A00340`
- `ACC_MDR_CSGWall::DB_makeCSG`：`0x10A04590`
- `ACC_MDR_CSGWall::do_solid_segments`：`0x10A08FC0`
- `DB_RoutingBranch::createPathSegment`：`0x105B3B60`
- `ACC_CSGWallJoiner::calculateYDir`：`0x109FF910`

`calculateYDir` 使用的初始化向量在 `0x10AF2AC0`，内容为全局 `+Y`。短弧随后把后续截面转到径向方向，二者之间形成 ruled triangular transition。`default_wall_start_wedge` 按“默认 +Y 起始截面与内侧径向线的交点”构造该楔体。RR001 样本的新增点复现为 `(-5058.2188, -15289.8818)`，与 RVM facet 一致。

## 数据集覆盖

### DB 1112（土建主样本）

完整生成结果位于 [gen-1112-final/report.json](../output/ams-civil-audit-20260906/gen-1112-final/report.json)：访问 6059 个元素，生成 4702 个，跳过 5 个，失败 0 个。生成的土建 noun 包括 FLOOR 81、GWALL 120、PANE 4275、SBFI 137、STWALL 75、WALL 14。5 个跳过项均有确定的 E3D 无形状原因：2 个 GWALL 缺少有效轮廓，3 个 SBFI 的 DESP 尺寸退化。

### DB 7997 与 8000（混合模型交叉检查）

- DB 7997：生成 51609，跳过 23，失败 72；土建部分包括 FLOOR 191、GWALL 132、PANE 3938、SBFI 12、SCTN 1280、STWALL 169、WALL 48，土建失败为 0。72 个失败全部是依赖 db7000 的管件目录解析问题。
- DB 8000：生成 2617，跳过 0，失败 7；包含 PANE 1、SCTN 22，土建失败为 0。7 个失败全部是依赖 db7000/db7600 的管件目录解析问题。

对应报告分别为 [gen-7997-final/report.json](../output/ams-civil-audit-20260906/gen-7997-final/report.json) 与 [gen-8000-final/report.json](../output/ams-civil-audit-20260906/gen-8000-final/report.json)。这些管件目录问题不属于本轮土建几何算法。

## plant-ui 验证

使用当前 release 后端，监听 DB `1112,7997,7999,8000,251142`，项目为 `AvevaMarineSample`、MDB `/ALL`、namespace `strt_cpv_ui_20260905`。在 plant-ui 模型树中依次展开：

```text
/1RS-CIVI
  /1RS-WF03
    /1RS-WF03-W
      /1RS-WF03-W-C
        /1RS-WF03-W-C-RR001
```

对 `17496/105799` 强制按需生成得到 `Generated`、`model_available=true`、130 个模型实例；随后非强制复查得到 `AlreadyAvailable`、`publication_status=ready`。响应留存在 [plant-ui-ensure.json](../output/ams-civil-audit-20260906/plant-ui-ensure.json)。界面切换可见后成功显示完整环形墙体，日志记录“查询到 130 个元素、130 个网格实例”和“模型显示完成：1 个目标”。截图见 [plant-ui-civil-wall-final.png](../output/ams-civil-audit-20260906/plant-ui-civil-wall-final.png)。界面里的房间归属查询错误来自该样本缺少房间派生数据，不影响模型生成、发布或显示。

## 验证命令

- `cargo test --manifest-path ../vendor/e3d-model/Cargo.toml --lib`：259 passed，0 failed。
- `cargo fmt --manifest-path ../vendor/e3d-model/Cargo.toml --check`：通过。
- `git diff --check`（e3d-model）：通过；仅报告现有工作树的 LF/CRLF 转换提示。
- release 后端构建：通过；只有既有 dead-code 与 future-incompatibility 警告。
