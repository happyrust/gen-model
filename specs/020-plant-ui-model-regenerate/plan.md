# Plan 020：模型树右键「重新生成模型」

**Date**: 2026-08-20  
**Spec**: `specs/020-plant-ui-model-regenerate/spec.md`

## Summary

给 plant-ui 模型树（与三维视口共用的 `element_menu`）加一项「重新生成模型」，
把「删掉已生产的产物 → 重新生成 → 当场显示」串成一条链。改动落在四处：

1. **gen-model 一处**：`/api/v1/health` 增 `delivery_unit_types` 字段，
   让客户端不用硬编码交付单元名词表。
2. **plant-ui-data 一处**：新增按 `inst_relate.anc` 解「已生成元素 + 其祖先链」
   的查询，供客户端归并候选生成根。
3. **plant-ui 绘制层**：`ModelAction` 增 `Regenerate` 语义入口、`element_menu`
   增菜单项、新增确认对话框、`WorkbenchVm` 增 `regen_busy`。
4. **plant-ui-app 宿主**：`model_update_api` 增 `delete_model_subtree` /
   `ensure_model` 两个客户端函数；`handle_cmds` 增重生成状态机
   （查 → 确认 → 删 → 逐根 ensure → 逐根显示 → 收尾）。

服务端不新增任何接口——`DELETE /model/subtree` 与 `POST /model/ensure` 都是现成的，
`force` 那个参数本来就是给这个场景留的。

## Technical Context

- **Language/Version**: Rust edition 2024，nightly-2026-08-02（两仓同工具链）
- **Primary Dependencies**: egui 0.35、ehttp、fork SurrealDB 2.1.4、aios-core（vendored 于 plant-ui）
- **Storage**: SurrealDB；本特性只读 `inst_relate` / `pe`，写操作全部经 gen-model HTTP 面
- **Testing**: 纯函数单测（归根、计数、错误分档、文案）+ 源码形状断言 + live 真机验收
- **Target Platform**: Windows / PowerShell
- **Constraints**: 不得 `cargo clean`；plant-ui 工作树里另有在飞改动
  （`_run_*.log`、`.codex-artifacts/` 显示 08-20 当天还在改 view3d），按路径暂存

## Constitution Check

- **I 水位承诺**：不碰水位。删除与 ensure 都走非增量路径，`ensure_regen_pending`
  写的 `RegenRoot` 行 `dbnum = 0 / source_end_sesno = 0`，本来就不认领会话号。
- **II 单一规则**：**归根规则出现了第二处实现**，这是本特性最大的宪法风险。
  缓解按三条走：(a) 名词表只有一份权威（服务端 `configured_delivery_unit_types()`），
  客户端经 `/health` 读，禁止硬编码（FR-005）；(b) normal-root 兜底**不抄**，
  解不出交付单元的元素原样交服务端解（FR-004 第 3 项）；(c) 两边对不上时的信号
  不许静默——400「容器不能做生成根」必须响亮报错（FR-015）。
  客户端那份只是「上限估计 + 派发去重」，正确性的最终裁决仍在服务端。
- **III 静默失效**：五处出错分档明确（FR-015），无 `_ => 放行`；
  `AlreadyAvailable` / `NoRenderableGeometry` 分别计「跳过」「无可画几何」而不是
  混进「成功」；deep query 空集给明确空态而不是静默不做事。
- **IV 队列收口**：不新增队列 action。失败复用既有 `RegenRoot` pending 行 →
  `/api/v1/update/pending-units` → 界面「待重试单元」→ `pending-units/retry` 复活，
  三条出路齐全。
- **V 标识真值**：候选根来自库里真实的 `inst_relate.anc` 与 `pe.noun`，
  不猜、不按名字前缀推断；名词表来自服务端而非本地 `DbOption.toml`。
- **VI 可执行守护**：归根纯函数、错误分档、计数收尾、确认文案四类各配单测；
  `force:false`、「删除必须晚于 deep query」、`regen_busy` 三条不变量配源码形状断言
  （SC-007）；真机部分按 SC-001/002/005 留证据。

结论：通过。II 条的双份实现已按「权威单一 + 兜底不抄 + 分歧响亮」缓解，
不需要 Complexity Tracking 例外。

## Referenced Decisions

- `../plant-ui/docs/adr/0009-generate-on-show-goes-through-the-service.md`：
  显示补齐走服务、容器要客户端展开、ensure 必须异步限并发有进度、
  「客户端多养一套判据要一直跟服务端对齐」的警告。本特性把它那条「展开一层」
  换成 anc deep query（一层对 SITE 不够），其余照办。
- `../plant-ui/docs/adr/0010-visibility-ledger-records-the-instruction.md` 与
  `0016-eye-reflects-rendered-visibility.md`：eye 只读实际渲染回执；
  删除后先 `Unload` 让 eye 诚实回「未加载」。
- `../plant-ui/docs/adr/0021-get-work-clears-and-reloads-the-loaded-set.md`：
  「清场 → 重装 → 回放方向」的三段式。本特性**刻意偏离**它的第三段
  （不回放隐藏，全部显示，FR-012），偏离理由写在 spec Assumptions 里。
- `../plant-ui/docs/plans/model-tree-context-menu-and-viewport-toolbar.md`：
  行菜单的既有纪律——`push_id(row.refno)`、作用对象在菜单里就地算死不回读 vm、
  成批才报数、不接线就不显示。新菜单项一条不落地照办。
- `../plant-ui/docs/adr/0011-execution-progress-moves-to-the-task-queue.md`：
  任务队列装的是**数据批次**。重生成不是数据批次，因此不进那里（FR-010）。

## Project Structure

```text
specs/020-plant-ui-model-regenerate/
├── spec.md
├── plan.md
└── tasks.md

# gen-model
src/web_service/handlers.rs                     # /health 增 delivery_unit_types + 形状断言

# plant-ui
crates/plant-ui-data/src/lib.rs                 # anc → (元素, 祖先链) 查询 + pe.noun 批量读
crates/plant-ui/src/lib.rs                      # ModelAction / Cmd 新变体
crates/plant-ui/src/vm.rs                       # regen_busy、确认框与进度的 Vm
crates/plant-ui/src/workbench/tree.rs           # element_menu 新增项（自成一组）
crates/plant-ui/src/model_regenerate.rs         # 新文件：确认对话框
crates/plant-ui-app/src/model_update_api.rs     # delete_model_subtree / ensure_model / health 读名词表
crates/plant-ui-app/src/main.rs                 # 重生成状态机、归根纯函数、错误分档
CHANGELOG.md（两仓各一条）
```

## Implementation

1. **服务端名词表出口**（`handlers.rs`）：`health` 的 JSON 增
   `"delivery_unit_types": configured_delivery_unit_types()`。它是进程内 `OnceLock`、
   不读库，不套 `within_health_budget`。补一条形状断言进既有的 health 断言簇。

2. **数据层查询**（`plant-ui-data/src/lib.rs`）：新增
   `pub async fn generated_subtree(root: RefU64) -> Result<Vec<(RefU64, Vec<RefU64>)>>`，
   语句 `select in, anc from inst_relate where anc contains <root_u64>`（与
   `query_inst_refnos_by_root_anc` 同一条索引，只多取一列），走 `flat_read_db()`；
   再加 `pub async fn nouns_of(refnos: &[RefU64]) -> Result<HashMap<RefU64, String>>`
   批量读 `pe.noun`，分批口径沿用 `model_instances_anc` 的 1500/批。
   查询前先 `inst_relate_anc_ready()` 探一次，未回填响亮失败。

3. **归根纯函数**（`main.rs`，可单测、不连库）：
   ```rust
   fn regeneration_roots(
       targets: &[(RefU64, String)],            // 右键那几行及其 noun
       generated: &[(RefU64, Vec<RefU64>)],     // deep query 结果
       nouns: &HashMap<RefU64, String>,
       delivery_units: &HashSet<String>,        // 来自 /health
   ) -> Vec<RefU64>
   ```
   按 FR-004 的三部分并集 + target 自身（非粗层级名词才进），有序去重。
   **不依赖 anc 顺序**——单测里故意把 anc 打乱，结果必须不变。

4. **HTTP 客户端**（`model_update_api.rs`）：
   - `delete_model_subtree(base, refno, project, mdb, namespace)` → DELETE，
     query 串带 `confirm = refno`（服务端强制相等，不等就是 400）。
   - `ensure_model(base, refno, force, …)` → POST，超时设 125 秒（略大于服务端 120，
     让服务端的超时语义先生效），回包解出 `status` 供计数分档。
   - `health_delivery_unit_types(base)` → 复用既有 `get::<Health>`，`Health` 结构增
     `#[serde(default)] delivery_unit_types: Vec<String>`（老服务返空 → 客户端按
     FR-005 拒绝容器行并告警）。

5. **命令契约**（`plant-ui/src/lib.rs`）：`Cmd` 增
   `RegenerateModels { targets: Vec<(RefU64, String)> }`（带 noun，绘制层手上就有，
   免得宿主再查一次）与 `RegenerateConfirm { accepted: bool }`、`RegenerateStop`。
   **不塞进 `ModelAction`**——`ModelAction` 是「宿主对三维做什么」，重生成是
   「宿主对模型库做什么」，混进去会让 view3d 那侧收到一个它处理不了的变体。

6. **确认对话框**（新文件 `model_regenerate.rs`）：照 `model_update.rs` 的
   Confirm 步做，含两个真实数字与那句「中途中断的话，没重做完的那些找不回来」。
   `model_update.rs` 已有先例测试 `the_confirm_step_must_say_the_button_cannot_be_taken_back`，
   这里配一条同型的。

7. **菜单项**（`tree.rs::element_menu`）：在 `room_menu_section` 与「复制 REFNO」
   之间插一组。作用对象用既有的 `targets`（就地算死，不回读 `vm.selection`）。
   `live` 为假或 `vm.regen_busy` 为真时不画 / 置灰。

8. **状态机**（`main.rs`）：一个 `Regeneration` 结构持有
   `roots: VecDeque<RefU64>`、`done / skipped / failed / background` 四个计数、
   `stopping: bool`。串行推进：取队首 → `ensure_model` → 按 `status` 与 HTTP 码分档
   （FR-015）→ 成功即清 `scopes` 相关项并推 `SetVisible` → 刷新 `ModelLoadVm` →
   下一个。整趟中止的两类错误直接清空队列并置 `Failed`。收尾时清 `regen_busy`。

9. **顺序**：先落会红的回归（归根纯函数、错误分档、`force:false` 形状断言），
   再实现；`rustfmt` → 定向 `cargo test` → 两仓 `cargo check --workspace --all-targets`
   全过后才上真机。真机按 SC-001（单 BRAN）→ SC-002（一个小 ZONE）→ SC-005/006 顺序走，
   **不要拿 SITE 做第一次真机验证**。
