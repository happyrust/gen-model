# Plannator 开发计划:gen-model + e3d-io 直读生成模型

> 计划 ID:`2026-08-30-direct-read-model-generation`
> 创建:2026-08-30
> 状态:**proposed**（待 plannotator 门禁批注）
> 拍板前提:**gen-model 直读 E3D `.dat` 库文件生成模型,经 `vendor/e3d-io` + `vendor/e3d-attlib`,不对接 pdms-io**（用户 2026-08-30 拍板）
> 权威:ADR-053(生成期查询面/direct 模式)、ADR-055、
> `docs/plans/2026-08-30-e3d-io-gen-model-gap.md`(G1–G13 实测缺口体检)、
> `docs/plans/2026-08-30-e3d-io-core-aligned-rewrite.md`、
> teach/0002(core.dll 更新逻辑)、0009(批量生成管线)、0011(libgm 几何算法)
> 上下文:`上下文/会话-2026-08-30-恢复cursor会话-e3dio直读协作.md`(实时更新)

## 目标

让 gen-model 的模型生成链从「连 DB(SurrealDB/pdms-io 摄入)取数」切换为
**直接读 E3D 原生 `.dat` 库文件取数**,端到端生成与 DB 模式一致的模型。
新增的是**一种模型生成数据源**,生成算法本身(fast_model/resolve/cata_model/prim_model/loop_model)不重写。

## 底账:哪些已经解决(立计划时点)

| 缺口 | 状态 | 落点 |
|---|---|---|
| G1 跨库引用 | ✅ t-354 | `src/data_interface/direct_store.rs`:dbnum 池化(DashMap+Mutex) + `CataDbLocator` 定位,未注册 dbnum fail loud;实测跨库 92 跳目录库 5052 自动 pin |
| G2 会话时点 pin | ✅ t-354 | `e3d-io/src/engine.rs` 新增 `ReadOnlyEngine::open_at(path, sesno)`;CATA 库开库解一次冻住 |
| G9 快照守卫 | ✅ t-354 | DirectStore `FileIdentity` 守卫,文件被换 → `FileReplaced` 报错阻断 |
| G10 并发形态 | ✅ t-354 | 按 dbnum `Mutex<DbView>` 池,ADR-053 已定姿势 |
| G11 NamedAttrMap 转换 | ✅ t-354 | `src/data_interface/direct_attmap.rs`:形状权威 = DB schema `default_val`;REFNO/OWNER/TYPE 特判;词属性 db1_dehash;定不了型入侧通道不瞎猜 |
| BANG 角度定型 | ✅ 2026-08-30 本会话 | 角度表(125 条,UNIT==ANGL)烘焙进 `e3d-attlib`(`angle_attrs.rs`);`e3d-io/src/record/descriptor.rs` 单字标量非 Int/Bool 且 `is_angle_attr` → `Real(i32 百分度/100)`;e3d-io 167 lib 测全绿 |
| G3 children 顺序 | 已定约束 | 用 `ParsedElement.members` 原序,不 sort 不 dedup;对拍比**序列**不比集合 |
| G5 UDA / G6 深遍历 / G7 性能 / G12 显示名 / G13 多 extent | ✅ 实测可用 | gap 文档 §2,不排期 |

**仍开着的:G4(目录表达式求值)、G8(反向引用,待用法清单)、UDA sesno pin 残留、
以及全部改动还在工作区未提交。**

## 完成判据

- [ ] gen-model 从 `.dat` 直读生成一批真实元件模型,**cargo 依赖树内无 pdms-io**,运行期不连 DB。
- [ ] 双跑对拍门:同批元素 direct vs DB 模式,①NamedAttrMap 一致(children 比**序列**);②几何产物(CSG 参数/mesh)一致。
- [ ] 目录表达式差分门全绿:DB 模式表达式串 vs e3d-io 渲染串逐条并排,方言差异归成有限规则并收口(或升级修法 B 后求值结果一致)。
- [x] BANG/角度类属性正确(raw −9000 ↔ −90.0)。
- [ ] t-327 / t-354 / BANG 全部改动落成提交,不再裸奔在工作区。
- [ ] 覆盖矩阵(G1–G13)逐条写终态,不留「还没看」。

## Phase 1 — 落盘与对拍收口

状态:proposed。**防丢优先,最便宜,先做。**

- [ ] 与并行 agent 协调后分仓提交:e3d-io(index/diff + engine open_at + descriptor 角度)、
      e3d-attlib(角度表三件套)、gen-model(Cargo path 依赖 + direct_store + direct_attmap + 探针)。
      工作区多 agent 在飞,提交前逐文件核对归属,不打包别人的中间态。
- [ ] 复跑 `direct_attmap_probe` dbnum 8000:200 样本 + 7333 全量。预期:t-354 那 44 条 BANG
      真值冲突消失,残差只剩 CACHID/DESC 各 1(逐条归因记账)。
- [ ] e3d-attlib / e3d-io / gen-model 三仓测试基线记进 progress。

验收:对拍零未归因真值冲突;三仓提交落地;基线全绿。

## Phase 2 — 生成期查询面切直读(ADR-053 收口清单)

状态:proposed。依赖 Phase 1 的提交基线。

fast_model/resolve 消费的每个查询,给出 DirectStore 直读版(DB 版保留,开关切换):

- [ ] `get_children_named_attmaps` 直读版:成员树原序(G3 约束写进代码注释与测试)。
- [ ] `get_cat_refno`:存量引用 1–3 跳走查(CATR/SPRE/PRTREF 链收口 SCOM/SPRF/SFIT/JOIN),
      吃 DirectStore 跨库池。
- [ ] `query_group_by_cata_hash` / `get_or_create_cata_context` 直读版(目录库上)。
- [ ] `get_world_transform`:owner 链单库折叠(6 库 588k 元素实测 owner 0 跨库;
      措辞留边界——语料实测,非格式保证,跨库 owner 出现即 fail loud)。
- [ ] 深层遍历+noun 过滤直接在成员树上做(G6,1.4 µs/条不值得索引)。
- [ ] UDA sesno pin 残留:`UdaCatalog::read` 加带 sesno 重载(G2 残留,e3d-io 侧小改)。
- [ ] G8 反向引用:先盘生成链反查用法清单;若只有少数几处,一次全库走查建内存反向表
      (30 万键 ≈0.4 s),不造持久索引。

验收:查询面逐函数有直读单测 + direct/DB 双跑对拍(键集与序列)。

## Phase 3 — G4 目录表达式求值(生成正确性核心)

状态:proposed。**对拍先行,不许凭样例猜方言规则。**

- [ ] 3a 差分先行:同批目录元素(ams5052,≥3000),DB 模式读出的表达式字符串 vs
      e3d-io 五路渲染串**逐条并排**,差异归成有限几类方言规则。
      已知分歧样例:e3d-io 渲染 `ATTRIB PARA[10 ]`/`ATTRIB RPRO G`,现有求值器吃
      `DESI[1.1]`/`RPRO_CPAR`(gap 文档 §G4 表)。
- [ ] 3b 修法 A(字符串对齐):公开 `rendered_by_shape` 分派器(e3d-io 侧 ~20 行,或消费方抄)
      + 方言映射层;gen-model 继续用 `aios_core::eval_str_to_f64`(resolve + tiny_expr)。
- [ ] 3c 若 3a 显示结构性分歧大 → 修法 B:e3d-io 暴露 DBE token/树,gen-model 内建小求值器
      镜像 core.dll `DBE_Base::evaluate` 家族(算术叶复用 tiny_expr)。
      逆向锚点已备:DBE 类型化变体 @ core.dll `0x108e966c`–`0x108e96c6`,context=设计元件供 DESPARAM。
- [ ] 点号列表(`P61 P71`)不是标量表达式,按属性语义在转换器分流。

验收:表达式差分门全绿;`eval_str_to_f64` 在直读串上与 DB 模式同值(抽样 ≥1000 条逐条断言)。

## Phase 4 — 端到端直读生成试点

状态:proposed。依赖 Phase 2、3。

- [ ] 试点 1:纯 prim 元件(BOX/CYL 族,无表达式)——最短路径打通全链。
- [ ] 试点 2:BEND/ELBO 族(吃 BANG + 目录表达式)——正好压 BANG 修复与 G4。
- [ ] 试点 3:BRAN 管路(成员序敏感,压 G3;管道 CSG 对照 Core3D
      `MDR_BranchVisualisationManager::getCSGTree` 语义)。
- [ ] 双跑门:direct vs DB 几何产物逐字段比对(CSG 参数/顶点),attmap 比序列。

验收:三类试点产物一致;失败逐条归因,不留未解释分歧。

## Phase 5 — 批量收口与覆盖矩阵终态

状态:proposed。

- [ ] 全库批量生成(ams8000,6605 键)跑通,性能与失败记账
      (e3d-io 侧读+解码 0.026 ms/元素,瓶颈预期在转换与生成,拿真数)。
- [ ] 覆盖矩阵 G1–G13 逐条写终态(已复刻/记账不做/有意省略),不留空白。
- [ ] CHANGELOG + 本计划状态翻 complete;上下文档案收口。

验收:批量有数;矩阵无空格。

## 风险与依赖

- **多 agent 并发工作区。** e3d-io/gen-model 工作区多人在飞,编译会撞中间态
  (本会话实测:record 测试瞬时假红,稍后全绿)。对策:提交前逐文件核对归属;
  重验证用隔离 `CARGO_TARGET_DIR`(t-327 教训:共享 target 撞 stale rlib)。
- **方言映射靠猜必错。** G4 必须差分先行(gap 文档 §4 第 3 条),规则从并排差异里归纳。
- **AVEVA 语料会被升级重排。** 写死偏移/refno 锚点必须允许缺席跳过(plant-io 计划同款教训)。
- **owner 不跨库是实测不是保证。** 跨库 owner 一旦出现走 fail loud,不静默错读。
- **别在 e3d-io 跑仓级 cargo fmt**(会重排历史漂移文件);别用 `--tests` 全量编
  (他人在飞的未提交测试可能编不过,见 gap 文档 §5 构建提醒)。
- **BajieAsk/会话中断风险。** 所有阶段结论实时写
  `上下文/会话-2026-08-30-恢复cursor会话-e3dio直读协作.md`,断线可续接。

## Errors Encountered

| Error | Attempt | Resolution |
|-------|---------|------------|
| （立计划时点无） | | |
