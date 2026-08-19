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

## Success Criteria

- 无 OCC、无替代后端时，定向生成网格的调用失败且回执可见，CI 能抓住。
- GWALL extra `pe:17496_116569` 在 manifold 布尔下 gen→rvm p95 仍 ≤180；空结果不覆盖
  已有切洞网格。
- 可盖原语（箱/球/柱/挤出/旋转/多面体）在双后端下网格非空、AABB 有效，抽检 RVM 对拍通过。
- 扫掠体：直墙、带斜切平面的墙、弧墙、360° 环形截面均过既有墙体 RVM 门；空轮廓挤出失败。
- 活库盘点列出圆环面 / PrimLSnout / 碟 / 锥 / 扫掠体的实例数；Phase 4 命令
  `--no-default-features --features ws,gen_model,manifold,project_hd,http_api` 不含 `occ`
  时，生产路径上每个会走到的类型都能生成或响亮失败。

## Assumptions

- ADR-029 的布尔换库继续独立完成，本规格不重做切洞入口。
- 几何参数解释与 Core3D 扫掠步骤仍以 aios-core 为准；本仓不发明新的扫掠数学。
- manifold-csg 本期不增加通用 3D 脊线扫掠 API。
- 不运行 `cargo clean`。
