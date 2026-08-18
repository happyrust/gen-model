# core.dll primaryList 权威快照与门控证据（2026-08-18）

## 目标

关闭 ADR-009 中 `primary_list_hint()` 恒为 `true` 的保守缺口，同时保持净窗口
Added / Modified / Deleted 三态、`children_changed` 两端数据、公开 API 与模型 Regen
判定不变。primaryList 只门控 core.dll `DB_UserChanges` 的成员/顺序事件标签。

## 逆向确认

E3D 3.1 `core.dll.i64`：

- `DB_Noun::primaryList` @ `0x58da260`：若 `this+97` 未加载则调用
  `DB_Noun::ReadDataDab`，返回 `this+136`。
- `DB_Noun::ReadDataDab` @ `0x58d7100`：调用
  `db_get_element_info(noun_hash, 297853135, &value)`，成功后把
  `this+136` 设为 `value == 1`。
- `db_get_element_info` @ `0x5aae0a0` 是导出入口；因此快照直接调用该函数，与
  core 自身读取链同源，不从普通属性字典推测。

## 数据源与采集

- core.dll：50,071,544 bytes，FileVersion `1.3.13.0`，ProductVersion
  `1.3.13.0[C13130-112]`，SHA-256
  `e4600d050a908f281d207bad52507dcbb82d4d8036c8d4d71e6e72eb290476d8`。
- noun 源：`noun_flags.json`，370,702 bytes，SHA-256
  `965ab9d34387a59b43b9f063e57d085653a880a027aa0cf0a80dade93844a768`。
- 命令：

  ```powershell
  python scripts/e3d/dump_core_primary_list.py --pid <LIVE_DES_PID> `
    --out tests/fixtures/core-primary-list-e3d31.json
  ```

先对 `ZONE,DAMP,BOX,EQUI,BRAN,SITE` 六个 noun 冒烟，6/6 成功且均为 true，live
进程继续响应；随后全量采集：

```text
count=1931
resolved_count=1879
unknown_count=52
true_count=1142
false_count=737
```

5 个成功读取返回非二进制整数（MDB/USLI/CAST/GROU/DBALL 均为 2）；严格按 core
的 `value == 1` 判定为 false。52 个读取失败的 noun 没有混入 false，逐项保存在
快照 `unknown` 中，运行时仅对这些项保守返回 true。

曾尝试在新起 TTY 中通过 `DbElementType.IsPrimaryList` 全量枚举，该进程以
`0xC0000005` 退出且未产出快照；改走已初始化 live 进程的 core 导出入口后完成，
未重启、未修改 live 数据库。

## 实现与验证

- 快照：`tests/fixtures/core-primary-list-e3d31.json`
- 可复现采集器：`scripts/e3d/dump_core_primary_list.py`
- 生产提示：`src/data_interface/model_impact.rs::primary_list_hint`
- B-EVT-03：DAMP（true）保留成员事件；TP（false）关闭成员事件；ROD（unknown）
  保守保留。断言覆盖显式 gate 与 `user_change_buckets` 实际调用。
- `core_primary_list_snapshot_is_complete_and_self_consistent`：钉住 core SHA、字段号、
  1931/1879/52/1142/737 计数、resolved/unknown 互斥和 unknown 保守策略。
- `tests/model_impact.rs`：从库外钉公开 gate 的 DAMP=true / TP=false / ROD=unknown。
- `tests/python/test_dump_core_primary_list.py`：纯函数钉 noun 归一化/去重、严格
  `value == 1`、非二进制值与 unknown 分区。

阶段性定向测试：

```text
cargo test --locked --lib primary_list --no-default-features \
  --features ws,gen_model,manifold,project_hd -- --nocapture
=> 2 passed; 0 failed; 988 filtered out
```

完整验证（全部 exit 0）：

| 检查 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | 通过 |
| `cargo check` | 0 errors / 205 warnings（既有警告） |
| `model_impact::tests` | 33 passed |
| `data_interface::net_window` | 13 passed / 2 ignored |
| `increment_pipeline` | 48 passed / 6 ignored |
| `db8000_session_pairs` | 20 passed |
| `cargo test --test model_impact` | 2 passed |
| Python 采集器纯单测 | 2 passed |
| Python offline | 84 passed / 23 deselected |
| 纯文件净差分 live | 1 passed，18.55s |
| 纯文件 payload 对拍 live | 1 passed，20.61s |
| issue-019 固定签名 + T11b | 2 passed，40.52s；原 db8000 SHA 恢复为 `2eae3055…7137` |

正式脚本重构后再次全量复跑，生成 JSON 与跟踪快照**字节完全相等**，两者 SHA-256
均为 `095b9266b2d43d23e1ac74885a0a5f7ed7e2f16cd471f719c72b484ae0ef113a`；live E3D
进程在复跑后 `Responding=True`。SigMap 与四角色回滚包结果见本轮 verification record。
