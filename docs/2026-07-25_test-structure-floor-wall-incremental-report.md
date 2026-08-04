# 结构专业 FLOOR/WALL 按需与增量测试报告

日期：2026-07-25  
运行栈：`bin/surreal.exe`，`ws://127.0.0.1:8009`，namespace `1516`，database `AvevaMarineSample`  
设计库：dbnum `7997`  
目标查看器：`D:\work\plant-code\old\rs-plant-3d`

## 结论

- FLOOR、WALL、STWALL、GWALL 的真实按需模型生成已通过，目标元素均产生 `inst_relate`、`world_trans` 和有限 AABB。
- 修复了 FLOOR 隐含 PAVE/VERT 属性读取、缺失 owner 的查询兼容、极限圆角有限性，以及结构子构件到最小交付单元的生成根解析。
- 修复了 WALL 的核心问题：精确 CATA 闭包遗漏 `GMSS`，导致 `SPRF -> GSTR -> GMSS` 的目录几何没有按需解析，最终得到 0 个几何体。
- CATA 依赖缓存增加闭包规则版本。旧缓存不会继续掩盖新增的 `GMSS` 子树。
- 增量影响分类已覆盖结构构件：`POS` 走纯变换，`FRAD/HEIG/DESP` 走几何重生成，`SPRE` 走目录反向级联，`NAME` 只更新数据/模型树而不重建几何。

## 真实样本结果

| 类型 | 请求参考号 | 生成根/父级 | 结果 |
|---|---:|---:|---|
| FLOOR | `24381/180272` | `CFLOOR 24381/180271` | 按需生成通过；AABB `[0,0,0]` ～ `[24450,24450,100]`，平移 `[0,0,45130]` |
| WALL | `24381/180032` | `CWALL 24381/180031` | 修复前 0 几何；修复后按需生成通过，实际更新 22 个模型节点 |
| STWALL | `24381/180037` | `CWALL 24381/180031` | 实例、AABB、变换均存在 |
| GWALL | `24381/180703` | `24381/180702` | 按需生成通过；AABB `[2148.7905,23858.531,41550]` ～ `[6476.945,25436.676,45130]` |

WALL `24381/180032` 的最终数据库证据：

- `inst_relate:24381_180032.aabb = aabb:⟨582248735169211718⟩`
- AABB：`[-13020.827,-24188.408,-6000]` ～ `[-7467.167,-20341.62,-5900]`
- world translation：`[101.91,226.26,-6000]`

## 根因与修复

### FLOOR

1. 部分结构元素的 owner `pe` 行缺失，`[noun, owner.noun]` 反序列化失败后被上层吞成空集合。
2. PAVE/VERT 等隐含子元素只有 noun 表记录，没有通用 `pe` 行。
3. 样本 PAVE 的 `FRAD=24450` 与边长相等，需要保证轮廓、OCC 近似和最终 AABB 均为有限值。

修复：

- owner noun 查询使用空字符串兜底。
- PAVE/VERT/POINSP/CURVE 支持从 noun 表读取隐含元素。
- 结构轮廓参数查询支持从 PAVE/VERT 表回退。
- 增加极限圆角的轮廓到 PlantMesh 回归测试。

### WALL

WALL 使用 `SPRF -> GSTR -> GMSS`。精确 CATA 闭包原白名单仅包含 `GMSE/NGMS/PTSE/PSTR/SPRO/DTSE`，没有展开 `GMSS`。数据库只落了 `pe_owner` 边，没有其目录几何实体，`resolve_desi_comp` 因而返回空几何。

修复：

- 精确闭包加入 `GMSS`。
- 闭包依赖缓存增加 `CATA_CLOSURE_SCHEMA_VERSION=2`，规则变化后旧缓存自动失效。
- SPINE 缺通用 `pe` 时从 `SPINE` noun 表读取。

## 增量行为测试

结构子构件的路由断言：

| 修改 | 期望动作 | 测试结果 |
|---|---|---|
| PAVE/PLOO 下 `FRAD` | 重生成 FLOOR 几何 | 通过 |
| WALL/FLOOR 的 `HEIG`、`DESP` | 重生成几何 | 通过 |
| `POS` 移动 | 只更新 world transform | 通过 |
| WALL/STWALL 的 `SPRE` | 目录依赖反向级联并重生成 | 通过 |
| `NAME` | 更新数据和模型树，不重建几何 | 通过 |
| PAVE/VERT/PLOO 子构件 | 归一到 FLOOR/CFLOOR 最小交付单元 | 通过 |
| WALL 子构件 | 归一到 CWALL 最小交付单元 | 通过 |

这里的增量修改验证是分类器、生成根和真实模型生成三层验证；本轮没有声称完成 E3D UI 中的实际编辑会话。

## 已执行测试

```text
cargo test data_interface::model_impact::tests --lib
# 10 passed

cargo test data_interface::generation_root::tests --lib
# 1 passed

cargo test data_interface::on_demand_model::tests --lib
# 2 passed, 1 ignored（真实库测试单独执行）

cargo test fast_model::loop_model::tests::structural_floor_extreme_fillet_remains_finite --lib
# 1 passed

cargo test data_interface::cata_closure::tests::precise_config_limits_container_subtree_and_adds_desi --lib
# 1 passed

AIOS_CATA_CLOSURE_MODE=on AIOS_ON_DEMAND_TEST_REFNO=24381/180032 \
  cargo test live_generates_a_missing_model --lib -- --ignored --nocapture
# WALL: passed

AIOS_CATA_CLOSURE_MODE=on AIOS_ON_DEMAND_TEST_REFNO=24381/180703 \
  cargo test live_generates_a_missing_model --lib -- --ignored --nocapture
# GWALL: passed
```

## 尚未完成的可视化验收

E3D 和 `rs-plant-3d` 的窗口自动化在当前桌面会话连续发生 monitor capture interop 错误，无法可靠操作 E3D 修改构件或采集 rs-plant-3d 前后截图。因此：

- 本报告确认数据库增量判定、生成根、目录按需解析、模型实例与 AABB正确。
- 本报告不把数据库 AABB 证据冒充 rs-plant-3d 三维前后截图。
- 待桌面捕获恢复后，应在 E3D 对同一 PAVE/WALL 分别执行 `POS`、`FRAD/HEIG`、`SPRE`、`NAME` 修改，并用 rs-plant-3d 对模型树与三维视图各采集前后截图，完成最终 UI 验收。
