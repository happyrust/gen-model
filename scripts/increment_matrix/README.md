# 关键联动窗口验收

从仓库根目录运行。依赖本机 AMS 工程、E3D TTY、`python/.venv` 中的 aios_db，以及 `D:/Rust/target/debug` 的 gen_ams、update_ams、l3_suite、rvm_verify、rvm_compare 和 examples/rvm_facets_dump。这些工具必须由待验收源码构建；验收报告记录实际二进制指纹，不自动重建。

```powershell
python -m unittest discover -s scripts/increment_matrix -p test_verify.py
python scripts/increment_matrix/run_family.py equipment --out output/increment-matrix/equipment-new
python scripts/increment_matrix/run_family.py hvac --out output/increment-matrix/hvac-new
python scripts/increment_matrix/run_family.py pipe --out output/increment-matrix/pipe-new
python scripts/increment_matrix/run_window.py <case.json> --out <new-directory>
python scripts/increment_matrix/run_family.py equipment --project-dir <isolated-AMS-project> --live-url http://127.0.0.1:18292 --out <new-directory>
cargo test --locked --offline --lib recorded_matrix_windows_reach_expected_roots -- --ignored --nocapture
```

三个 family 命令会写入样例工程：创建随机命名 A/B 副本，依次移动 A、恢复位置、增加成员、INCLUDE 搬迁至 B、删除成员、删除 A/B。每腿只有一次 SAVEWORK；宏、TTY 日志、实际 S/T、窄口径 RVM 与数据/模型验收均留存。不要并发执行 TTY，也不要在 TTY 保存时运行窗口验收；跨库 SAVEWORK 也可能改动 8000 的 bookkeeping。输出目录必须不存在。

`run_window` 的 case 字段：db、dbnum、attlib、template、project_dirs、catalogues、base、target、before_roots、after_roots；可选 expected_changes（例如 reparented:1）、rvms（根 refno 到 RVM 文件的映射）。数据窗为 S+1..T。两端全量与 update 比较稳定 GeometryId、变换、AABB、体积和拓扑；RVM 必须有非空配对，不能缺件，配对为 exact/chord 且轴向差不超过 1 mm。缺失 RVM 明确记为 not_run，live 始终单列 not_run。

恢复位置采用本夹具固定 U 500 的逆操作 D 500，并严格对比创建时几何。不要用 Q POS 的显示文本或经过 f32 的数据接口坐标覆盖原值：曾分别产生 0.27 mm 和亚微米至亚毫米的偏差。全链净窗允许 DBMETA/DBMDAT 与 CACHID bookkeeping，任何其他业务残留失败。根测试直接执行真实窗口收集器，缺数据库/空矩阵失败；初始导入的阈值回退单独明确声明，编辑窗口不允许回退。

`bridge_preflight.mac` 是不含 SAVEWORK 的桥架只读复现，当前 runtime 访问 `=2013287686/3` 即异常退出。它不是完成的桥架编辑用例。完整记录见 `docs/evidence/2026-09-06-cascade-key-matrix/README.md`。

发生异常时先检查 summary.json 的 cleanup/cleanup_error 和 driver 日志；进程被外部终止时 finally 无法保证运行，依据本次记录的 scratch 名称核查清理，不使用通配符删除。

`--live-url` 在每腿离线验收后调用 `dbnums`、`update/pending-units` 和分页 `model/records`；不调用 ensure，不通过手改水位、ACK 或删除 pending 达成通过。要求持久记录来源为 model-database、数据/模型水位均等于 T、model_verdict=in_sync、该库无 pending/死信，并逐根对比几何成员与世界 AABB（0.01 mm；不单独证明网格拓扑）。模型类型过滤、基线未完成或其他根的真实失败可能阻止整库通过，需如实记录。删除根要求 records 为空。详见 `docs/evidence/2026-09-06-cascade-live/README.md`。

`--catalogue-7600` 必须与 live 服务实际选择的目录文件一致，当前默认是仓库配置中的 `ZDJ/ZDJ000/zdj7600_0001`。不得仅凭相同 dbnum 将它换成 `AvevaPlantSample/aps000/aps7600_0001`：后者缺少 AMS 两个 ATTA 引用的 `23984/105633`，会人为制造初始化 hold。隔离工程目录需要包含配置声明的 ZDJ 目录（只读目录链接即可）。旧验收报告的文件指纹和目录来源保持原样，不追溯改成新输入。

整库初始化验收还需保留一个未参与编辑的原始根，与独立 full@T 对照非空成员和世界 AABB；仅检查 scratch 根与水位可能漏掉错误的基线选根清理。2026-09-06 的重叠父根误删回归见 docs/evidence/2026-09-06-cascade-catalogue/README.md，并由 rozen_baseline_and_window_target_have_the_same_roots 验证基线与稳态 roots_T 的一致性。
