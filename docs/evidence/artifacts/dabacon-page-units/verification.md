# Dabacon 页长单位修正验证记录

## 权威结论

- `core.dll` 3.1，函数 `0x53F544A`：`(double)header_value * 512.0 * 4.0 / 1000000.0`。
- 同一函数将物理页长设为 `2048`，并以 `Page size ... bytes` 输出。
- 因此文件头偏移 `0x34` 的 `512` 是 32 位 word 数；物理页长是 `512 * 4 = 2048` 字节。

## 验证命令与字面结果

### V2 引擎单元测试

命令：

```powershell
rtk cargo +nightly-2026-08-02 test --manifest-path D:\work\plant-code\pdms-io-fork-engine-v2\crates\pdmsdb_engine_v2\Cargo.toml --lib -- --nocapture
```

目录：`D:\work\plant-code\pdms-io-fork-engine-v2`

字面结果：

```text
running 14 tests
test db2::header::tests::header_page_length_is_stored_in_u32_words ... ok
test db1::page_store::tests::rejects_a_header_size_false_positive_when_the_session_bounds_are_impossible ... ok
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

退出码：`0`

### 分页解析器测试

命令：

```powershell
rtk cargo +nightly-2026-08-02 test --lib paged::tests -- --nocapture
```

目录：`D:\work\plant-code\old-parse-pdms-db-paged`

字面结果：

```text
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 31 filtered out
```

退出码：`0`

### 主仓离线测试

命令：

```powershell
rtk cargo +nightly-2026-08-02 test --locked --lib data_interface::on_demand_db::tests --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
```

目录：`D:\work\plant-code\old\gen-model-occ-retire-endgame`

字面结果：

```text
test result: ok. 2 passed; 0 failed; 2 ignored; 0 measured
```

退出码：`0`

### 真实 Dabacon 分页读取

ACP7000 字面结果：

```text
[paged_db] path=D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000\acp7000_0001 snapshot_sesno=272 page_size_bytes=2048 physical_pages=8730 bytes_read=17879040 cache_hits=8978 cache_misses=8475 prefetched_pages=255 index_pages=8727 record_pages=0 parsed_records=0
test ...production_acp7000... ok
```

ACP7320 字面结果：

```text
paged locator: ref0s=6 bytes_read=38858752 file_len=431941632 physical_pages=18974 index_pages=18969 record_pages=0
[paged_db] ... snapshot_sesno=306 page_size_bytes=2048 ...
test ...production_acp7320... ok
```

两条命令退出码均为 `0`。

## 结果

- 文件头原始字段统一命名为 `page_size_words`。
- 派生物理长度统一命名为 `page_size_bytes` / `page_size_bytes_hint`。
- 旧的 `512` 字节候选不会再通过最新 session 页的结构和边界校验。
- 生产路径使用本地 V2 引擎依赖；V2 的旧比较入口已删除，fixture 对照模块命名为 `FixtureOracle`。
