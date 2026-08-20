# 2026-08-20 BRAN / TUBI 房间计算 live 复验

## 范围

- 用例：`fast_model::room_fixture::tests::live_room_tubi_row_enters_tree_and_tracks_regen`
- 提交：`89f8b06b`
- 配置：`python/tests/DbOption-roomlive.toml`
- 数据库：fork SurrealDB 2.1.x，一次性内存实例 `127.0.0.1:8071`
- 报告：`output/bran-room-test/20260820-133556/live-batch/report.json`

## 执行

```powershell
./scripts/Start-Surreal8009.ps1 -Memory -Bind 127.0.0.1:8071

./scripts/Run-LiveBatch.ps1 `
  -Manifest scripts/live-batches/room-fixture-8071.json `
  -Only live_room_tubi_row_enters_tree_and_tracks_regen `
  -Output output/bran-room-test/20260820-133556/live-batch
```

退出状态：`0`。批次报告：`1/1 pass`，脚本计时 `2.4s`；测试本体 `1.07s`。

## 字面输出与判据

```text
房间归属重建: 1 间房 / 2 块面板
房间归属重建完成: 写入 6 条成员边
[房间增量] 构件 4000000001_30 归属: 无房间 -> K100
房间归属重建: 1 间房 / 2 块面板
房间归属重建完成: 写入 7 条成员边
test fast_model::room_fixture::tests::live_room_tubi_row_enters_tree_and_tracks_regen ... ok
test result: ok. 1 passed; 0 failed
```

本轮证明合成 BRAN 重生成后的隐含 TUBI 会进入空间树并参加房间计算，最终新增一条
`room_relate` 成员边。它不替代真实文件 staged 窗口的 release gate；后者仍需提供
`AIOS_STAGED_REGEN_DB_FILE`、`AIOS_STAGED_REGEN_DBNUM` 和
`AIOS_STAGED_REGEN_ROOT`。

## 隔离与恢复

- 运行前保存 `accel_tree_AvevaMarineSample.snapshot`，原始 SHA-256：
  `D1534C0D4160630FF2E2EE4C9399E8F596341A94363736951FFE3B514805338A`。
- 测试后停止 8071 内存实例；端口确认不再监听。
- 原空间快照已恢复，恢复后 SHA-256 与基线一致。
- 验证记录：`output/bran-room-test/20260820-133556/verification.json`。
