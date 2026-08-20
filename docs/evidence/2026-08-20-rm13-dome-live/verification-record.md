# RM13 dome / Plant UI 与 E3D 对拍修复验证

## 结论

- 对象：PANE `24381/36945`；生成根 `24381/36944`；负实体 NREV `24381/36946`。
- 真正的显示差异不在半球网格本身，而在**实例选择**：`booled_id=24381_36945_63` 已存在时，旧 Plant UI slim 回退仍返回正体 `14738304298809260922`，并施加 `scale=[1,1,234]`，所以界面看到的是挤出正体而非布尔半球。
- 修复后的读写约定：有 `booled_id` 时只显示一个 `geo_hash=booled_id` 的单位变换实例；Manifold、OCC、平表补扫与 Plant UI 回退查询均遵守该约定。

## 基线行为（exit 0）

命令：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\Invoke-Surreal8009.ps1 -Sql "SELECT booled_id, insts_flat FROM inst_relate:24381_36945;"
```

基线文件 `baseline-db.json` 中的字面结果：

```text
booled_id="24381_36945_63"
insts_flat=[{geo_hash:"14738304298809260922", transform.scale:[1,1,234]}]
```

旧 slim 回退也从 `out->geo_relate ... geo_type='Pos'` 得到同一正体，忽略 `booled_id`。这就是 Plant UI 里看起来不像半球的旧行为。

## 修改后数据库行为（exit 0）

生产补扫使用的同型条件表达式已在 SurrealDB 2.x 上直接执行：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\Invoke-Surreal8009.ps1 -Sql "UPDATE inst_relate:24381_36945 SET insts_flat = IF booled_id != NONE THEN [{ geo_hash: booled_id }] ELSE (SELECT trans.d AS transform, record::id(out) AS geo_hash FROM out->geo_relate WHERE visible && out.meshed && trans.d != none && geo_type='Pos') END; SELECT booled_id, insts_flat FROM inst_relate:24381_36945;"
```

字面输出（`conditional-flat-update.json`）：

```json
{"booled_id":"24381_36945_63","insts_flat":[{"geo_hash":"24381_36945_63"}]}
```

成品网格不再重复乘原语的 `Z×234` 变换。

## 网格形状回归（exit 0）

```powershell
cargo test --locked --lib rm13_dome_pane_minus_nrev_is_a_hemisphere --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
```

```text
running 1 test
test fast_model::manifold_tessellate::tests::rm13_dome_pane_minus_nrev_is_a_hemisphere ... ok
test result: ok. 1 passed; 0 failed
```

该回归不只检查体积：每个曲面顶点必须满足半径 `23400 mm` 的球面方程，底面必须位于 `Z=0`；AABB 为 `[-23400,-23400,0]..[23400,23400,23400]`（容差 1 mm）。诊断读取成品得到 `16848` 顶点、`5616` 三角形，最大球面径向误差 `0.061 mm`。

## 读写选路回归

```text
cargo test --locked --lib booled ...                       exit 0; 2 passed
cargo test -p aios_core display_insts_tests -- --nocapture exit 0; 2 passed
cargo test --locked --lib empty_difference_is_bad_bool_not_a_silent_swallow ... exit 0; 1 passed
cargo check                                                  exit 0
cargo check -p plant-ui-app                                  exit 0
cargo build --release -p plant-ui-app                        exit 0
```

完整输出：`final-targeted-tests.txt`、`plant-ui-tests-and-check.txt`、`final-cargo-check.txt`、`final-plant-ui-release-build.txt`。

## Plant UI / E3D 外观对拍

- E3D：`e3d-direct-comparison.png` 中模型树与命令窗确认同一 `Ref =24381/36945`。
- Plant UI：`plant-ui-fixed-right-view.png` 中属性面板为 `24381_36945`，右视图轮廓为宽 `2R`、高 `R` 的半圆，日志为：

```text
PANE PANE 1  查询到 2 个元素、1 个网格实例
模型显示完成：1 个目标
ERROR 0
```

这验证了界面当前加载的是布尔半球，而不是正体挤出。

## 部署

```text
built SHA256=042161CCEDD5532FEA7C643F60E82432F072769241039BF2F3C072081261BBD5
live  SHA256=042161CCEDD5532FEA7C643F60E82432F072769241039BF2F3C072081261BBD5
pid=12344
```

- 修改后程序：`D:\work\plant-code\old\test-worklspace\bin\plant-ui-app.exe`
- 修改补丁：`bool-display-fix.patch`
- 原件/修改件哈希：`bool-display-artifact-hashes.json`
- Plant UI 右视图：`plant-ui-fixed-right-view.png`
- E3D 同对象截图：`e3d-direct-comparison.png`

## 回滚

脚本：`rollback-bool-display-fix.ps1`。实际回滚会校验当前修改件哈希、停止 Plant UI、恢复四份源码与旧程序、把目标 `insts_flat` 恢复为 `NONE`，再启动旧程序。

验证命令（exit 0）：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\rollback-bool-display-fix.ps1 -VerifyOnly
```

```text
verified ...\plant-ui\vendor\rs-core\src\rs_surreal\inst.rs
verified ...\gen-model\src\fast_model\manifold_bool.rs
verified ...\gen-model\src\fast_model\occ_generate.rs
verified ...\gen-model\src\fast_model\pdms_inst.rs
verified ...\test-worklspace\bin\plant-ui-app.exe
ROLLBACK_INPUTS_OK
```
