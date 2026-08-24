# AMS 8000 页式读取 / CATA 预加载修复证据

## 对象

- 源库：`.scratch/occ-retire-sphere-projects/AvevaMarineSample/ams000/ams8000_0001`
- SurrealDB 副本：`ws://localhost:8049`，RocksDB `.surreal/site-8000`
- 修复后二进制：`E:/codex-target/occ-retire-paged/debug/aios-database.exe`
- SHA-256：`05f4db498e41965a289de6f3f1ffc5f449828a1bc14ffdc4b3c8a26c72b41a59`
- 运行 PID：`72348`

## 基线失败

命令：

```text
E:\codex-target\occ-retire-paged\debug\aios-database.exe
```

输入：`.sites/8000/DbOption.toml`，环境
`AIOS_STARTUP_AUTORUN=true; AIOS_SKIP_STARTUP_ROOM_BUILD=1; AIOS_ROOM_INCREMENTAL=0`。

字面输出：

```text
thread 'tokio-rt-worker' panicked at
D:\work\plant-code\old-pdms-io-record-boundary\src\defines.rs:99:20:
attempt to multiply with overflow
空闲轮 panic，已隔离，worker 继续（同因第 4/5 轮）: attempt to multiply with overflow
```

进程由 panic 隔离器保持存活，故无进程退出码；该轮未消费模型工作单。

## 修复

- `core.dll 3.1` 的数据库信息函数 `0x53F544A` 明确按
  `page_count * 512 words * 4 bytes` 计算容量，并固定输出
  `Page size 2048 bytes`。因此文件头 `0x34` 的原始值 `512` 是 32 位
  word 数，不是字节数。页引擎字段改名为 `page_size_words`，只通过
  `page_size_bytes()` 做受检的 `* 4` 换算；外部 hint、快照和日志统一使用
  `*_bytes` 后缀。
- `OnDemandDbSession` 与 Ref0 扫描只打开 `PagedDbSession`，不再二次调用
  `DabaconSnapshot` 解析同一会话页。
- 页式引擎在打开时验证 page size、session 边界和 index root；主仓另校验
  打开前后的文件长度与 mtime，变化时失败闭合。
- CATA 目录发现只读 60 字节文件头，不解析会话页。
- 本地联调图中 `aios_core` / `parse_pdms_db` / `pdms_io` /
  `pdmsdb_engine_v2` 均为 path 依赖；主仓与 Python 特性图未启用
  `legacy_session_replay`。

## 纯函数与实文件门

```text
cargo +nightly-2026-08-02 test --locked --lib data_interface::on_demand_db::tests \
  --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
test result: ok

cargo +nightly-2026-08-02 test --locked --lib data_interface::cata_closure::tests \
  --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
test result: ok. 29 passed; 0 failed

production_acp7000_locator_opens_authoritative_paged_session
test result: ok. page_size_bytes=2048 sesno=272

production_cata_locator_uses_paged_snapshot_below_io_budget
paged locator: ref0s=6 bytes_read=38858752 file_len=431941632 physical_pages=18974 index_pages=18969 record_pages=0
test result: ok
```

上述命令退出码均为 `0`。

## AMS 8000 修复后运行

启动后字面输出：

```text
[paged_db] ...\ams8000_0001 snapshot_sesno=233 page_size_bytes=2048 ... record_pages=16 parsed_records=48
[cata_closure] 按需预加载完成: parsed=950 missing=0
[cata_closure] 按需预加载完成: parsed=1908 missing=0
空闲模型积压消化完成 16 个任务
```

`stderr` 中搜索 `panic|overflow|SessionPageData|failed|失败`：空输出，命令退出码 `0`。

SurrealQL 检查：

```text
RETURN {
  pending: array::len((SELECT VALUE id FROM model_update_pending WHERE status IN ['pending','failed'])),
  dead: array::len((SELECT VALUE id FROM model_update_pending WHERE status IN ['pending','failed'] AND (attempts?:0) >= 5)),
  inst_relate: array::len((SELECT VALUE id FROM inst_relate)),
  inst_geo: array::len((SELECT VALUE id FROM inst_geo)),
  pe: array::len((SELECT VALUE id FROM pe))
};

{"dead":0,"inst_geo":366,"inst_relate":344,"pe":16519,"pending":1700}
```

与启动前 `pending=2228, inst_relate=0, inst_geo=0, pe=12979` 比较，工作单已消费
528 条，持久模型产物已生成。初始化仍在 PID 72348 中继续收敛，本记录不将
尚未清空的工作单标记为整体完成。

13:06 的延长观察为
`{"failed":0,"inst_geo":1519,"inst_relate":1293,"pending":1220}`；连续 20 次、
30 秒间隔的查询中 `failed` 恒为 0，几何产物持续增长。监视命令因达到
10 分钟观察上限且工作单尚未清空而返回 `2`，服务 PID 72348 保持运行继续收敛。

## 变更与回滚产物

- 主补丁：`docs/evidence/artifacts/ams8000-paged-cata/main.patch`
  (`8046b918c16104e8142e1b77e1c3b14ab0353224d44b2bcc3703088391f0331a`)
- 解析包本地依赖补丁：
  `docs/evidence/artifacts/ams8000-paged-cata/parse-pdms-db.patch`
  (`7fce7a2ba49fa6faa54eb4c7727d49641244a21a63592447a3d9f8ad92ef3565`)
- IO 包本地依赖补丁：
  `docs/evidence/artifacts/ams8000-paged-cata/pdms-io.patch`
  (`2ea47529cdaf6f84a863125ff7b3229f874b5c4fc2ded40ea83c658dcd7182f1`)
- 回滚：`docs/evidence/artifacts/ams8000-paged-cata/rollback.ps1`
  (`e46d199c328b22992a2a029bcecf4512524cbda097682b4e18de892178722270`)。
  无参数执行已返回「回滚预检成功」；传 `-Apply` 才实际回滚。
