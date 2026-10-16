# 房间重建第五笔：收尾落定之后才放下「在飞」

- 日期：2026-10-15（第五轮）。
- 提交：`a519d2c9`（`feat/issues-36-50-34`），基线 HEAD：`f9b3af190`。
- 缺陷：`run_rooms_rebuild` 的准入闸重开早于收尾落定——`note_edge_recompute_finished()` 排在
  `refresh_room_update_record_with_model`（收尾写 `room_update_record`）之前。循环一跑完闸就开，
  第二趟「重新生成房间」可被放进来先清账（两张边表全删 / 水位整表 NONE / 记录 NONE）；
  composite 没变时它第一段只是一次零重建探针，很快就能走到清边，而第一趟随后写下的收尾记录
  是清边之前那一刻的账。第二趟若没跑完（`all_ok=false` 或强杀），库里只剩「边没了、水位 NONE、
  记录却说已最新」——下次启动检查只信记录，判 `UpToDate`，那批库从此不补。

## 改动文件与 SHA256

| 文件 | 改前 | 改后 |
| --- | --- | --- |
| `src/web_service/handlers.rs` | `357954ea9b505771b845cb8a1e263d97324b2cbc011201094e363de657496dac` | `7d00ba22012623201a878ea5d9e2b60b0422744966c20ffc46ec768f83ad87ef` |
| `src/web_service/room_rebuild_regression_tests.rs` | `9ca53d444f012bf87e1ab1973aca4c7a069898d24ed9b8512fd3f2a3982c1244` | `8df2421c1c8661c6304b7f28f6518b0207651a0ac4d75b9b6b1c424b92b37213` |
| `docs/verification/2026-10-15-room-rebuild-regressions.md` | `e1bbf3bead423b957aa763db1d63043791b2617f732775b75b9c865817a37b11` | `76b242955d4e4f0827b7d04273a85f2ad4a5f3e0e856060a23cc043b17615c9c` |

改动量：`handlers.rs` +7 −3（三处：注释与 `note_edge_recompute_finished()` 的位置、`rooms_rebuild`
文档一条）；测试文件 +30（新红钉一处）；文档 +31（第五轮小节）。

## 复现命令与实测

```text
cargo test --offline --lib room_rebuild_regression_the_flight_is_held_until_the_settle_lands
# 保留态（把 note_edge_recompute_finished() 放回 if/else 之前）：exit=101
#   test result: FAILED. 0 passed; 1 failed; 0 ignored; 2651 filtered out; finished in 0.00s
#   断言把 run_rooms_rebuild 整个函数体打出来，可对读两处调用的先后
cargo test --offline --lib room_rebuild_regression
# 修复后：test result: ok. 8 passed; 0 failed; 0 ignored; 2644 filtered out
cargo test --offline --lib
# 全量：test result: ok. 2520 passed; 0 failed; 132 ignored; 0 filtered out; finished in 15.64s
```

编辑器诊断（`get_problems`）：`handlers.rs` 与 `room_rebuild_regression_tests.rs` 均 errors 0 / warnings 0。

## 证据等级与范围

- 红钉是**源码契约**（接线位置：收尾那一笔在放下「在飞」之前），不是并发行为复现；两趟真交错的
  现场要真机——窗口是第一趟收尾那几笔 DB 写（毫秒量级），对面只有走「composite 没变」的探针路
  才进得来，本轮不碰。
- 本轮未跑任何真机 / HTTP 端到端；不碰 `SUL_DB`、不碰项目配置、不写进程级镜像（红钉只读源码文本）。
- 范围外：`changelog.md` 仍未改（工作区里是另一条线未提交的改动）；③「循环中途 panic 把
  `edge_recompute_in_flight` 留在原处」仍在，未加 RAII / 超时自愈；②（第三段部分失败对外与全成功
  同形）与 ①（清边整表 vs 重算清单）未动。
- 临时件（已删）：`.scratch/tt5-red.txt`、`.scratch/tt5-green.txt`、`.scratch/tt5-full.txt`、
  `.scratch/tt5-msg.txt`。
