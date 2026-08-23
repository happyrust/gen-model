# Tasks 020：模型树右键「重新生成模型」

- [ ] T001 [US1..4] `specs/020-plant-ui-model-regenerate/spec.md`：评审定稿八项决定的验收语义，特别确认 FR-012（全部显示，含本来隐藏的）与「中断永久丢失」被接受。
- [ ] T002 [US2] `src/web_service/handlers.rs`（gen-model）：`/health` 增 `"delivery_unit_types": configured_delivery_unit_types()`，不套 `within_health_budget`（进程内 OnceLock，不读库）；补形状断言进既有 health 断言簇。
- [ ] T003 [US2] `../plant-ui/crates/plant-ui-app/src/model_update_api.rs`：`Health` 结构增 `#[serde(default)] delivery_unit_types: Vec<String>`；新增 `health_delivery_unit_types(base)`。老服务返空集时调用方按 FR-005 拒绝容器行。
- [ ] T004 [US1][US2] `../plant-ui/crates/plant-ui-data/src/lib.rs`：新增 `generated_subtree(root)`（`select in, anc from inst_relate where anc contains <u64>`，走 `flat_read_db()`）与 `nouns_of(refnos)`（批量读 `pe.noun`，1500/批）；两者调用前先 `inst_relate_anc_ready()` 探活，未回填响亮失败。
- [ ] T005 [US2] `../plant-ui/crates/plant-ui-app/src/main.rs`：**先落失败回归**——`regeneration_roots()` 纯函数单测。必须覆盖：交付单元祖先命中、元素自身是交付单元、无交付单元祖先落元素自身、target 自身按粗层级名词过滤、嵌套交付单元两个都进候选、**anc 顺序打乱结果不变**、整体有序去重。（函数还没写，此时必红。）
- [ ] T006 [US2] `../plant-ui/crates/plant-ui-app/src/main.rs`：实现 `regeneration_roots()`，让 T005 转绿。
- [ ] T007 [US1] `../plant-ui/crates/plant-ui-app/src/model_update_api.rs`：`delete_model_subtree(base, refno, …)`（DELETE，query 带 `confirm = refno`）与 `ensure_model(base, refno, force, …)`（POST，超时 125s，回包解 `status`）。
- [ ] T008 [US1][US3] `../plant-ui/crates/plant-ui/src/lib.rs`：`Cmd` 增 `RegenerateModels { targets: Vec<(RefU64, String)> }` / `RegenerateConfirm { accepted: bool }` / `RegenerateStop`。**不进 `ModelAction`**（那是对三维的动作，这是对模型库的动作）——补一条源码形状断言钉住这条边界。
- [ ] T009 [US3] `../plant-ui/crates/plant-ui/src/vm.rs`：`WorkbenchVm` 增 `regen_busy: bool` 与确认框 / 进度所需的 Vm 字段；`ModelLoadVm` 不改结构，只复用。
- [ ] T010 [US1] `../plant-ui/crates/plant-ui/src/workbench/tree.rs`：`element_menu` 在 `room_menu_section` 与「复制 REFNO」之间插入自成一组的「重新生成模型{suffix}」；吃 `live` 门禁；`regen_busy` 时置灰；作用对象用既有 `targets`（就地算死，不回读 `vm.selection`）。
- [ ] T011 [US3] `../plant-ui/crates/plant-ui/src/model_regenerate.rs`（新文件）：确认对话框，照 `model_update.rs` 的 Confirm 步；含 `|E|` / `|R|`（标明上限）与「中途中断的话，没重做完的那些找不回来」；`|E| == 0` 时只给关闭。配一条同 `the_confirm_step_must_say_the_button_cannot_be_taken_back` 的文案断言。
- [ ] T012 [US4] `../plant-ui/crates/plant-ui-app/src/main.rs`：**先落失败回归**——错误分档与计数收尾的纯函数单测。注入成功 / `AlreadyAvailable` / `NoRenderableGeometry` / 409 / 404 / 412 / 超时 / 生成失败 / 503 / 400-container，断言四类计数与「整趟中止」判据、400 不得静默。
- [ ] T013 [US1][US4] `../plant-ui/crates/plant-ui-app/src/main.rs`：实现 `Regeneration` 状态机——查 → 确认 → 每个 target 一条 `Unload` → 每个 target 一次 DELETE → 按候选根**串行** `ensure(force:false)` → 成功即清 `scopes` 并推 `SetVisible{visible:true}` → 刷 `ModelLoadVm` → 收尾清 `regen_busy`。让 T012 转绿。
- [ ] T014 [US4] `../plant-ui/crates/plant-ui-app/src/main.rs`：`regen_busy` 与 `get_work_busy` 互相置灰的接线 + 单测；eye 不受影响。「停在这里」只停派发，按钮文案不得含「取消」（文案断言）。
- [ ] T015 三条不变量的源码形状断言（SC-007）：ensure 调用点必须是 `force: false`；DELETE 的调用点必须晚于 deep query；重生成派发必须串行（并发常量为 1 或无并发容器）。
- [ ] T016 [US1] live 验收（小）：真机右键一根已生成 BRAN，取 `inst_relate` 行数基线 → 重新生成 → 断言先归零后重建、三维重新出现、eye 变「已显示」、全程未取回工作。证据落 `docs/evidence/2026-08-XX-plant-ui-model-regenerate/`。
- [ ] T017 [US2] live 验收（中）：真机右键一个小 ZONE，对拍客户端候选根集合与 `resolve_generation_roots_on` 对同一批元素的结果（允许客户端多出「无交付单元祖先」的元素本身，不许漏）；记录 `AlreadyAvailable` 计数佐证 SC-003。
- [ ] T018 [US3][US4] live 验收（交互）：确认框两个数字与 deep query 对得上；空集时不发任何请求；跑起来后两项置灰、eye 仍可点；「停在这里」之后不再派发。
- [ ] T019 `changelog.md`（gen-model）与 `../plant-ui/CHANGELOG.md` 各记一条。
- [ ] T020 `rustfmt`、定向 `cargo test`（T005/T012/T014/T015 全绿）、两仓 `cargo check --workspace --all-targets`。plant-ui 那道 ≤15s 增量闸门仍须成立。

## Dependencies

- T002 → T003（服务端先出字段）。
- T005 → T006，T012 → T013（都先红后绿）。
- T004 是 T013 的前置；T007、T008、T009 可与 T004/T005 并行。
- T010、T011 依赖 T008/T009。
- T014、T015 在 T013 之后。
- T016 → T017 → T018：**真机顺序不许颠倒**，第一次真机不要拿 SITE 试。
- T019/T020 在代码终态后。

## Notes

- plant-ui 工作树 08-20 当天还在改 view3d（`_run_v*.log`、`.codex-artifacts/`
  留着痕迹）。动 `crates/plant-ui-view3d/src/lib.rs` 之前先看工作树状态；
  本特性理论上不需要碰它——显示走既有的 `SetVisible` / `Unload` 两条命令。
- 建议的提交边界（各自能独立编译）：
  1. gen-model：`/health` 增字段；
  2. plant-ui：数据层查询 + 归根纯函数（无 UI）；
  3. plant-ui：菜单项 + 确认框 + 状态机。
- 归根规则在客户端有了第二份实现，这是本特性最大的长期风险（plan 的
  Constitution Check II 条）。若日后 gen-model 加了容器级 ensure 接口，
  第一件该删的就是客户端这份。
