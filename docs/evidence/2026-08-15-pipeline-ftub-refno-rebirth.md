# 2026-08-15 管道 FTUB Refno 重生与模型任务闭环

## 环境

- 仓库：`D:\work\plant-code\old\gen-model`
- E3D 隔离工程：`D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\pipeline-design-only-20260814-223100\projects\AvevaMarineSample`
- SurrealDB：本机 2.x 沙箱，`127.0.0.1:8009`，namespace `1516`，database `AvevaMarineSample`
- 后端：当前 debug 构建，HTTP `127.0.0.1:9099`

## 发现与修复

1. 新增 FTUB 复用了旧文件世代的 Refno，PE 被正确覆盖，但旧 `pe_owner` 入边和旧 children 槽仍存在。窗口 Add 现在在任何普通写入前，先清理该 Refno 的全部入边和 owner-id 范围。
2. staged 初始化批次注册 finalize 时无条件删掉了 `RegenRoot`。严格阶段模式又把模型生成后移，结果是水位提交成功但生成根永久丢失。现在先登记完整计划；只有模型实际成功后才由既有 settlement 删除对应项。
3. 模型页原按旧 `updated_at` 排序，1608 条历史积压会抢在新数据之后。现在按真实来源保存时刻倒序，缺时刻的 legacy 行置后，再以更新时间和 id 稳定排序。
4. F5 的旧 GENSEC 目标已不在当前 8000 文件。夹具改为当前 BRAN `24384/22402` 下复制 FTUB `24384/22403`，并在启动 E3D 前验证控制库和所有依赖 Refno。

## 自动测试

以下命令均 exit 0：

```text
cargo test --locked --lib data_interface::increment_pipeline::fold_tests --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
# 17 passed, 1 ignored

cargo test --locked --lib drain_select_leaves_dead_letters_in_the_table --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
# 1 passed

cargo test --locked --bin l3_suite --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
# 28 passed

cargo build --locked --bin aios-database --bin l3_suite --no-default-features --features ws,gen_model,manifold,project_hd,http_api
# exit 0
```

新增回归测试钉住：

- `added_refno_clears_every_previous_owner_edge_before_upsert`
- `staged_finalize_keeps_regen_roots_until_generation_settles_them`
- `drain_select_leaves_dead_letters_in_the_table` 的新保存优先顺序
- F5 当前文件依赖、工程控制库与宏语义前置检查

## Live 结果

### Refno 重生

- 会话 237 删除旧 FTUB；会话 238 新增 `/CODEX_L3_FTUB`，新 Refno `24384/26203`。
- 数据任务 `db-20260815-003640-000000` 成功应用 `237..=238`，`changed_elements=6`，水位推进到 238。
- 入库后 `pe:24384_26203` 为 FTUB、owner `24384/22402`；`pe_owner` 恰好只剩当前 owner 边，旧 children 槽为 0。
- `model_drain` 页第一根为 db8000 `24384/22402`，其后才是 2016 年来源的 db7330/db7329 积压；`units_done=1` 后该根 pending 已按 revision 收口。

原始证据：

- `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\pipeline-ftub-final-20260815\data-task-final.json`
- `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\pipeline-ftub-final-20260815\model-drain-after-root.json`
- `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\pipeline-ftub-final-20260815\surreal-final.json`

### 恢复

- 会话 239 删除 `/CODEX_L3_FTUB`。
- 数据任务 `db-20260815-004121-000000` 成功，随后 `delete_cleanup` 与 BRAN `RegenRoot` 均优先于历史积压执行。
- 最终 `pe:24384_26203.deleted=true`，关联 `pe_owner`、`inst_relate` 和根 pending 均为空，水位为 239。

原始证据：

- `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\pipeline-ftub-renderable-20260815\cleanup-239\data-task-final.json`
- `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\pipeline-ftub-renderable-20260815\cleanup-239\model-drain-after-root.json`
- `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\pipeline-ftub-renderable-20260815\cleanup-239\surreal-final-clean.json`

## 未通过项

E3D 的 `NEW FTUB` 后执行 `COPY =24384/22403` 不保留源件的 `SPRE/LSTU`，因此新增件虽然完成数据、关系、生成根与恢复闭环，但没有可渲染实例。两次显式目录引用试验均在 SAVEWORK 前崩溃，文件会话保持 239，结构化 outcome 为 `Indeterminate`，没有重放已保存变更；试验命令已从正式宏撤回。后续 Plant UI 新增几何验收应改用完整 CATA 工程镜像或另一条 E3D 原生复制命令。
