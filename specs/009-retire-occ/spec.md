# Feature Specification：分阶段退役 OCC 三角化

## User Stories

### US1：关掉 `occ` 不能假装生成成功

生产或 CI 在没有三角化后端时请求生成网格，系统必须响亮失败。调用方回执和日志都能看见
「没有可用的三角化后端」，不得只打 warning 后返回成功。

### US2：切洞与三角化失败能分开诊断

设计布尔失败时，系统不得把它解释成 OCC 未开启。三角化缺口也不得被解释成 manifold 切洞
失败。空切洞结果不得覆盖已有切洞网格。

### US3：墙体扫掠在无 OCC 时仍能对上 RVM

WALL / STWALL / GENSEC 的扫掠体在直线、带斜切平面、圆弧路径上生成的世界网格，必须通过
既有 RVM 对拍门；空轮廓不得产出可落盘的空网格。

### US4：未覆盖的原语不得被静默当成已迁移

圆环面、PrimLSnout、碟、锥在 Core3D 里是 libgm 一等原语。未实现对应 `gm_Create*` 之前
可回退 OCC，但必须能查出活库出现次数，且关 `occ` 时对每个未实现实例失败，不得生成空网格。

> **2026-08-23 更新**：这四类的实现都已落地，覆盖面不再是缺口，「可回退 OCC」这一条
> 随 FR-012 作废。US4 的剩余含义转为：段数口径未对齐的原语同样**不算**已迁移
> （FR-011）——能建出网格但共面抵消收不敛，比建不出来更难发现。

### US5：布尔要能收敛，段数就得跟 E3D 相等

同一条共面边界上的两层面片，只有段数与顶点相位都与 E3D 一致时，libgm 的共面抵消
（`cancelFacets` 只消全等重叠）才会生效。段数取错的后果不是网格粗糙，而是布尔结果里
留下一层本该被消掉的内壁。因此段数属于正确性，判据是相等而非足够细。

## Functional Requirements

- **FR-001**：`gen_inst_meshes` 在 `occ` 关闭且没有替代三角化后端时必须返回错误；禁止
  `Ok(())` 静默跳过。
- **FR-002**：生产布尔只走 manifold-csg 入口；OCC 布尔不得重新接到生成调度。
- **FR-003**：切洞若得到空网格，不得覆盖已有 `booled_id` 文件；ingest 空网格必须失败。
- **FR-004**：每个 `PdmsGeoParam` 应对齐一条 libgm `gm_Create*`（箱/柱/球/挤出/旋转/
  多面体 / PrimLSnout / 圆环面 / 碟 / 锥 / 斜端柱）。未实现的类型在替代器就绪前可回退 OCC；
  扫掠体走 FR-005。
- **FR-005**：扫掠体网格器必须覆盖 `DB_Gensec::do_solid_segments` 的 libgm 三支：
  `gm_CreateExtrusion`、`gm_CreateRevolution`（含 180° 半圆组合）、
  `gm_CreateRuledSolid`；斜切用延伸挤出挂到 CSG 树，不得改 ADR-026 的斜切平面 /
  BANG / PLAX / 单位网格身份。
- **FR-006**：360° 环形截面扫掠的体积与拓扑必须与既有两半合并结果对拍，不得用单次 360°
  旋转悄悄换拓扑。
- **FR-007**：空轮廓挤出（顶点不足或无三角）必须 hard fail，不得写入默认空 `PlantMesh`。
- **FR-008**：从 default/release 删除 `occ` 之前，必须完成活库 `PdmsGeoParam` 出现次数
  盘点；生产路径上仍出现的未覆盖类型要么有替代器，要么使关 `occ` 的构建失败。
- **FR-009**：浸水计算若仍依赖 OCC 形状拓扑，不得作为删除 `occ` 的阻塞项混进主生成路径；
  要么改为网格 CSG，要么独立 feature 且不进最终 release default。
- **FR-010**：后端切换不得靠放宽既有 RVM / mesh 对拍阈值过门。失败回滚只恢复 `occ`
  feature，不恢复 OCC 布尔。
- **FR-011**（2026-08-23 追加，依据 ADR-030 修订二 / ADR-044）：曲面原语的三角化段数
  必须由**真实半径**与统一弦高容差按 libgm 的权威规则算出，不得使用写死的段数常量或
  按构件自身尺度给的相对容差。回转 / collar 轮廓与挤出轮廓是 libgm 里的**两套**离散
  口径，不得互相顶替。段数规则的正确性判据是「与 E3D 相等」，不是「足够细」。
- **FR-012**（同上）：`tessellate_libgm_param` 不得再返回「回退 OCC」语义的成功值。
  libgm 表达不出的输入（如出平面回转轴）响亮失败；不是形状的占位（`Unknown` /
  `CompoundShape`）直接标 `bad`；已知形状一律在 manifold 上实现。
- **FR-013**（同上）：RVM 对拍工具的 gen 侧不得依赖 `occ` feature——量尺子的工具不能
  跟被量的对象一起被删掉。

## Success Criteria

- 无 OCC、无替代后端时，定向生成网格的调用失败且回执可见，CI 能抓住。
- GWALL extra `pe:17496_116569` 在 manifold 布尔下 gen→rvm p95 仍 ≤180；空结果不覆盖
  已有切洞网格。
- 可盖原语（箱/球/柱/挤出/旋转/多面体）在双后端下网格非空、AABB 有效，抽检 RVM 对拍通过。
- 扫掠体：直墙、带斜切平面的墙、弧墙、360° 环形截面均过既有墙体 RVM 门；空轮廓挤出失败。
- 每个曲面原语有一张「半径 → 段数」对照表单测，数值手算自 libgm 调用点表；生产路径上
  不存在默认段数常量，`tessellate_libgm_param` 里不存在「回退 OCC」语义的成功返回。
- 不带 `occ` 的 feature 组合下 `rvm_baseline/mesh_compare` 能编译并跑出对拍结果。
- 活库盘点列出圆环面 / PrimLSnout / 碟 / 锥 / 扫掠体的实例数；Phase 4 命令
  `--no-default-features --features ws,gen_model,manifold,project_hd,http_api` 不含 `occ`
  时，生产路径上每个会走到的类型都能生成或响亮失败。

## Assumptions

- ADR-029 的布尔换库继续独立完成，本规格不重做切洞入口。
- 几何参数解释与 Core3D 扫掠步骤仍以 aios-core 为准；本仓不发明新的扫掠数学。
- manifold-csg 本期不增加通用 3D 脊线扫掠 API。
- 不运行 `cargo clean`。
