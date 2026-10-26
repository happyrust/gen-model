# pipe-model 工作线移交文档

> 面向接续 Agent 的自包含交接。读完本文即可继续，无需翻历史会话。
> 最近更新：2026-10-26（undo_group + SELECT HSTU 落地后）。

## 0. 一句话任务

逆向 AVEVA E3D 3.1 的管道建模行为（影子安装 `E:\reverse\e3d\shadow_e3d31_aps_all`），在 Rust 仓 `gen-model` 里实现等价语义：独立 crate `pipe-model` 产 `MutationBatch`，经根 crate 适配器进既有 `EleOperationData`/`terminal_persist_changes` 持久化收口。

## 1. 环境与路径

| 项 | 值 |
|---|---|
| 仓库 | `D:\work\plant-code\old\gen-model`（分支 `refactor`） |
| 影子安装（取证源） | `E:\reverse\e3d\shadow_e3d31_aps_all` |
| Rust target | `D:\Rust\target`（并发 cargo 会持锁 `aios-database.exe`，遇 `os error 5` 先查僵尸测试进程） |
| 已入库主提交 | `b2f61ddcb`（pipe-model 全量，35 文件 +7262 行，2026-10-26） |
| 未提交增量 | `batch.rs`/`service.rs`（undo_group）、`pipeline.rs`/`specchange.rs`（HSTU）、本计划文档末节 |

真库夹具（仓内测试在用）：

- `ams0001_0001`（dbnum=1）、`ams8000_0001`（DESI dbnum=8000）——路径见 `tests/pipe_spec_forensics.rs`
- `acp7000_0001`（dbnum=7000 CATA 目录库）——`spec_catalog_roundtrip` 用它

## 2. 必读文档（按序）

1. **`docs/specs/pipe-modeling.md`** —— 后端功能规格（Story 1-56，A-L 组 + 决策表）。**这是契约，改实现前先对 spec。**
2. `docs/specs/pipe-modeling-ui.md` —— UI 规格（15 屏四区，Story 1-45）。UI 实现在外部 OpenCADStudio 仓，本仓只交 API 面。
3. **`docs/evidence/2026-10-15-e3d-pipe-modeling-ui-behavior.md`** —— 逆向证据主文档（§15-§18 共 18 节 + §19 属性总表，~1255 行）。**注意：此文件在 `.gitignore` 内，本地留存不入库，勿删。**
4. `docs/plans/2026-10-19-pipe-model-phase-a-skeleton.md` —— 滚动计划文档，每个里程碑有日期段落（2026-10-19 至 2026-10-22）。

## 3. 架构核心（改了会塌的四条）

1. **单向依赖**：`pipe-model` 只依赖 `aios_core` + `serde`，**不依赖 `aios-database`**；根 crate → pipe-model 单向。破环即设计事故。
2. **`MutationBatch` 是唯一服务产出物**——对应 E3D `UNDOABLE` 边界，原子提交（任一 op 失败整批拒）。字段：`name`/`ops`/`requires_confirm`/`warnings`。`undo_group(name, batches)` 是显式跨操作回滚组合器（E3D `undo.add/endUndoable`）。
3. **读面注入，写面只出批**：`PipeCatalog`（库数据读面）+ `SpecCatalog`（规格表求解）两 trait 注入服务；服务永不直接落库，只产 `MutationBatch`。
4. **显式纪律**：E3D 静默降级/失败的每一处，本实现必须显式化——warning（可继续）、`requires_confirm`（需确认）、`PipeError::Blocking`（拒绝）、`Raised`（致命）。**宁可显式失败不可静默编造**；内核黑盒一律接口桩 + Blocking/warning，不伪造推导值。

## 4. 文件地图

### pipe-model crate（`pipe-model/src/`，23 文件已入库）

| 文件 | 职责 | 语义组 |
|---|---|---|
| `service.rs` | `PipeModel` trait 全签名 + `PipeModelService<C>` 实现壳（`with_spec` 注入 SpecCatalog） | 入口 |
| `batch.rs` | `MutationBatch`/`MutationOp`/`Confirm` + `undo_group` | 基础 |
| `catalog.rs` | `PipeCatalog` 读门面 trait（`alloc_refno(under)` 属主分库路由） | 基础 |
| `spec_resolver.rs` | `SpecCatalog` trait + 三级回落 `resolve`（直查→SELE→PBOR 归一） | B/C |
| `pipeline.rs` | 建管/支管/改名级联/过程数据/端编辑/连接/断开 + **`select_hstu`** | A/D |
| `component.rs` | 建件/重选/翻转（`resolve` 调用范式在此） | B/C |
| `specchange.rs` | `change_spec` 三域级联 + `rebranch_spec`（含 HSTU 重选） | F |
| `splitmerge.rs` | `split_branch`（Feature 模式、`MoveTo` 双模、切点元件归新支管、末件放行）+ `merge_branches` | E |
| `naming.rs` | branch_name/split_name/normalise_new_name | 工具 |
| `choice.rs` | CHOICE 答卷串编解码 | 工具 |
| `weld.rs` | 建焊/重编号/`modify_welds`（WELDTY+SHOP 成对） | H/L |
| `fabrication.rs` | PSPOOL/PPIECE 生成、断点双形态、余量、FTUB 管端、校验 | I/L |
| `loader.rs` | 表格导入三段协议 | J |
| `slope.rs` / `tapping.rs` / `settings.rs` | 坡度/开孔/默认表 | G/D/J |
| `render.rs` | `render_batch → RenderedBatch`（与 EleOperationData 同构中性 DTO） | 持久化边界 |
| `context.rs`/`error.rs`/`outcome.rs` | Context/四级错误/三态结果 | 基础 |
| `testkit.rs` | `MockCatalog`/`MockSpec`/`ctx()` | 测试 |

### 根 crate 接线（`src/data_interface/`）

| 文件 | 职责 |
|---|---|
| `direct_pipe_catalog.rs` | 生产 `PipeCatalog`：架 `DirectStore` 文件直读；`waterlines: DashMap<i32,u64>` 每库水位；`probe_writable_dbnums(store, dbs, access_of)` |
| `direct_spec_catalog.rs` | 生产 `SpecCatalog`：SPEC 即根问键、SELE 下钻、SPCO 判据、`TUBI→TUBE` 别名、祖先链 `derive_answers` |
| `pipe_model_adapter.rs` | `RenderedBatch → EleOperationData` 逐字段映射 → `terminal_persist_changes` |
| `mdb_membership.rs` / `mdb_closure.rs` | `DbAccess` 证据穿管（CURD 声明元素 `ACCESSDB`/`LREFO`/`CLAI`） |

## 5. 已实现语义（spec 承诺面 = 实现面，无缺口）

- **A-D**：建管+首支管、ZONE 校验、自动命名、级联改名（只改现值匹配子项）、开孔 PTAP、过程数据级联
- **E**：支管分/合——按 `splitpipe.pmlfnc` 全文取证校正过（切点元件归新支管、`new.HREF=切点元件`、末件放行、`MoveTo` 双模、HCON/TCON 交叉标内核依赖）
- **F**：`change_spec` PIPE/BRAN/元件三域；ATTA `SPKBRK` 钉-复位；TEE off-leg `6-(arrive+leave)`
- **G-J**：坡度/建焊/PSPOOL+PPIECE/导入/校验（`datacon_blocking` 谓词表）
- **L**：断点双形态（No-weld 标志对 `ASFBR/LSFBR/CSFBR/TSFBR` ↔ WELDGAP 焊件互替）、余量 `LEXCES/AEXCES/USEFED`、FTUB 管端 `DESP[1..3]`+斜切校验、`modify_welds`（`WELDTY` 成对、`MANU/MACH/ORBI` 无落库列显式 warning）
- **`undo_group`**：显式跨操作回滚组合器
- **`SELECT HSTU`**：建支管 + BRAN 换 PSPE 两处接点；选择键 `PBOR0`（**不走** `PBOR0→PBOR1` 归一——那是元件双孔槽语义）；直查空→SELE 默认链回落；未接 spec/落选显式 warning

**关键命名纪律**：落库属性一律 attlib 短名（`ASFBR`/`LEXCES`/`WELDTY`/`DESP`…），PML 长名（`asfbreak`/`lexcess`）是别名面不落库。

## 6. 显式未做（边界，勿当缺口补）

- **ROUTER**：`route()` 是显式 Blocking 桩（E3D ROUTER 是内核黑盒，spec 本就定为接口槽）
- **连接内核**：`xCON` 推导词、GASK/FLAN 自动补件、几何对齐兜底——warning 占位
- **HCON/TCON 交叉值**：CONN 推导值不伪造，标内核依赖
- **位置重算**：断开/移动后的几何定位属内核
- **老 `spl*` spooler**：只读兼容（SPLN/SPOOL/SPLDRG 面已取证归档 §18.17），不实现
- **`pfmergespools`**：外挂批处理进程，非库内语义
- **UI**：属 OpenCADStudio 仓，本仓交付面=pipe-model API

## 7. 验证命令（全绿基线）

```powershell
cd D:\work\plant-code\old\gen-model
cargo test -p pipe-model                              # 72/72
cargo test -p aios-database --lib pipe_model_adapter  # 3/3
cargo test -p aios-database --lib mdb                 # 81/81（穿管回归面）
cargo test --test pipe_spec_forensics                 # 4/4（真库）
cargo check -p aios-database                          # 零新警告（unused-dep 是既有）
sigmap verify-plan docs/plans/2026-10-19-pipe-model-phase-a-skeleton.md  # 0 错
sigmap review-pr                                      # 审计 tracked diff
```

**已知坑**：`sigmap verify-plan` 把计划文档里的裸文件名/`<name>/B<mCount>` 当文件引用报错——写文件引用一律用 `pipe-model/src/xxx.rs` 全路径，伪路径加引号或改写。

## 8. 协作规则（仓内强制）

- `AGENTS.md`：多 Agent 协作**已暂停**（2026-10-16 用户决定）——不要按 `agent-handoff/BOARD.md` 旧表接单、不要声称文件归属；保护既有改动。
- 改代码前走 SigMap 流程：`sigmap verify-plan <plan.md>` → 实现 → `sigmap review-pr`。
- **工作树混有多条工作线的未提交改动**（`output/`、`verification/`、`db_session_fixture`、`increment_pipeline`、`.planning/` 等）——**只碰 pipe-model 线的文件**，提交时精确按路径暂存，不 `git add -A`。
- 不 push、不改 git config、不清理他人未提交改动。
- 文档/代码不写访问端点、令牌、密钥。
- 证据文档在 gitignore 内是**既有约定**，不是丢失。

## 9. 下一步候选（按价值排序）

1. **提交未入库增量**：`pipe-model/src/{batch,service,pipeline,specchange}.rs` + 计划文档末节（undo_group + HSTU，72/72 已绿）——scoped commit，别带上其他工作线。
2. **`sigmap review-pr`** 审当前 diff 后再提交更稳。
3. **`splitsegment` 段长分段引擎**：`splitsegment.pmlobj`（41KB）方法面已取证（§18.17.2）——纯 PML 位置代数非内核，可实现 `split_segment`（E 组延伸）。spec 里尚无此 Story，需先补 spec 条目。
4. **`derive_answers` 逆向校验**：真库上对更多 SPCO 类型（VALV/FLAN/ATTA 家族）跑反推→直查回环，目前只验过 TUBE。
5. **组件读面补强**：`component.rs` 的 `At3D` 定位/逆流交换当前是 warning 占位，若后续接几何内核可升级成真实现。

## 10. 取证速查（已实锤、勿重复探测）

- DESI/ISOD 数据文件**不含 DB 元素**（`refnos_of_noun("DB")` 为空，双库证实）；写权限证据在 **SYS 侧 CURD 声明元素**（`TYPE=DB`）。
- AMS 声明面 `ACCESSDB` 整列**缺席是常态**——缺席≠只读，`DbAccess::writable()` 默认声明即可写，仅 `ACCESSDB=Read`/`LREFO=true` 收窄。
- refno 高位打包 dbnum 编码——`alloc_refno(under)` 按**属主所在库**路由，每库独立水位，索引真空才退 `EDIT_FALLBACK_BASE=1_000_000`。
- SPEC 元素自身即 SELE 根问键（`SPEC.QUES="TYPE"`）；SELE 树 `TANS="ANY"` 通配、`ANSW..MAXA±0.001` 数值区间、`TDEF="EQUA"`→member[1] 等值链。
- `PMLUI\DES\pipe`（215 文件）是完整老一代 `_CD*` 界面资源层（f*/i*/m*/u*/l* 前缀分簇），无新增落库语义，已归档 §18.18——勿再逐文件深读。
- §20 待补清单**已清零**；逆向文档范围闭合。

## 11. 恢复工作时的最小检查

```powershell
cd D:\work\plant-code\old\gen-model
git status --short -- pipe-model docs/specs docs/plans   # 看本线改动面
cargo test -p pipe-model                                  # 应先于一切改动确认 72/72
```

然后从 §9 候选清单或用户新指令继续。
