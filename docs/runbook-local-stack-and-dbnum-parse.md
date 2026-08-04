# 本机栈是哪几个进程，以及怎么解析一个 dbnum

日期：2026-07-27

## 谁是谁

**plant-ui（`D:\work\plant-code\old\plant-ui`）的后端就是本仓 gen-model**，二进制名
`aios-database`。plant-ui 只认两个地址：

| 用途 | 地址 | 出处 |
|---|---|---|
| 模型服务（手动增量更新、按需生成） | `http://127.0.0.1:8021` | `plant-ui/crates/plant-ui/src/settings.rs` 的 `DEFAULT_MODEL_API_URL`；服务端是本仓 `DbOption.toml` 的 `http_api_addr` |
| 模型本体（SurrealDB） | `ws://127.0.0.1:8009`，ns `1516` / db `AvevaMarineSample` | 三份 `DbOption.toml`（gen-model、plant-ui、rs-plant3-d）的 `v_port`，必须一致 |

ns 与库名在几个实例上完全同名，连 SITE 根层的名字都重合，**指错端口看不出来**——
`docs/2026-07-27_room-incremental-audit-report.md` §4 记过一次因此写错结论的教训。

## 工作库用本仓的 `bin/surreal.exe` 起

不要用 PATH 上的官方 `surreal.exe`：本仓用的是 **fork 版 SurrealDB 客户端**，跟官方 server
握不上手，报 `WebSocket protocol error: SubProtocol error: Server sent no subprotocol`
（同上报告 :552）。

```powershell
cd D:\work\plant-code\old\gen-model
bin\surreal.exe start --user root --pass root --bind 127.0.0.1:8009 `
  rocksdb:D:/work/plant-code/old/gen-model/.surreal/ams-8009
```

历史上 8009 是 `memory` 起的，进程一死整个库就没了（`docs/2026-07-27_increment-update-complete-test-plan.md:143`）。
落盘到 `.surreal/`（已进 `.gitignore`）之后，重启机器数据还在，plant-ui 直接连得上。

**别用 `Get-ChildItem` 判断数据落没落盘。** 实例开着的时候目录项里的大小是陈旧的：
本次解析完 `000004.log` 在 `Get-ChildItem` 下显示 0 字节、整个目录 7999 字节，看着像一条
都没写；用文件句柄读实际长度是 34,357,012 字节。要查就查真实长度：

```powershell
$fs=[System.IO.File]::Open("$dir\000004.log",'Open','Read','ReadWrite'); $fs.Length; $fs.Close()
```

## release 产物先补 DLL

`scripts/extract_3rdparty.ps1` 把 OCCT 的第三方 DLL 只铺到 `D:\Rust\target\debug`，
release 下的 exe 因此启动即退、错误码 `-1073741515`（`STATUS_DLL_NOT_FOUND`），
plant-web-server 的站点记录里那条「解析失败，退出码: Some(-1073741515)」就是它。

```powershell
Copy-Item D:\Rust\target\debug\*.dll -Destination D:\Rust\target\release\ -Force
```

## 解析一个 dbnum：空库到可用基线

下面四步都在 `gen-model` 目录下跑——这些 bin 用 `config::File::with_name("DbOption")` 读
**当前工作目录**的 `DbOption.toml`，`project_path` / `project_name` / `surreal_ns` / `v_port`
全从那里取。整个流程不需要手改 `DbOption.toml` 的开关，每个 bin 自己把需要的选项顶进去。

```powershell
cargo build --release --features console

# 1) SYS 元数据（DICT/SYST/GLB/GLOB）
D:\Rust\target\release\sync_sys_only.exe

# 2) 目标 dbnum 的基线全量解析 + 水位收口
D:\Rust\target\release\initialize_ams_dbnums.exe 8000

# 3) 预览扫描，登记文件身份与 file_latest_sesno
D:\Rust\target\release\manual_scan_probe.exe AvevaMarineSample

# 4) 验收
powershell -File scripts\Test-AmsDbnumIntegrity.ps1 -Dbnums 8000
```

四步各自不可省的理由：

1. DESI 解析要靠 `MDB`/`WORL` 定位世界根。库里没做过 SYS 同步，每个 dbnum 都会解析出
   0 个元素——`src/bin/sync_sys_only.rs` 的文件头注释写着这件事。
2. `initialize_ams_dbnums` 走的是 `initialize_project_dbnum_baseline`，也就是手动增量更新
   确认执行时用的同一个初始化入口，不是另起的一套：内部 `baseline_sync_options` 固定
   `total_sync=true`、`replace_dbs=false`、`included_db_files=[该文件]`、
   `gen_model/gen_mesh=false`；PE 条数、`dbnum_info_table` 统计、本次解析条数三者对不齐
   就不推进 `applied_sesno`。
3. 第 2 步的 `finalize_baseline` 只写 `applied_sesno`/`sesno`，`file_latest_sesno` 是扫描
   观察值、由 `DbnumState::record_scan_observation` 写。跳过这步，验收脚本会报
   `watermark mismatch: applied=34 latest=0`——那是缺一次扫描，不是解析出了问题。
4. 验收脚本查三项：`pe` 条数、`dbnum_info_table` 统计、水位 `applied == file_latest`。

已经有基线的 dbnum 重跑第 2 步是幂等的：`baseline_needs_full_parse` 看到
`pe_count > 0 && applied_sesno > 0` 就不再解析。

## 2026-07-27 ams 8000 实测

空库（本文档这套 rocksdb 实例）上跑完四步：

```
BASELINE|AvevaMarineSample|8000|ok|14178
dbnum pe_count info_count applied_sesno file_latest_sesno
 8000    14178      14178            34                34
```

解析出来的是真实工程数据，不是 `runbook-sys-reparse-for-model-tree.md` 里说的那份手写夹具：
`WORL pe:16192_0`，四个 SITE `/1RX03-EQUI`、`/1CSV-HVACHB`、`/6KA-ELECHB`、`/1RX03-PIPEBJ`，
refno 前缀 24384，BRAN 600 / BEND 825 / ATTA 335。基线按 SITE 排入 4 个待生成根，
`model_update_pending` 里 4 条 `pending`——几何要等模型生成那一步，本流程只解析。

端到端也对上了：`GET http://127.0.0.1:8021/api/v1/dbnums` 里 8000 是 `34/34`，
同项目的 7997（`0/84`）、7999（`0/41`）仍是未解析状态——这一轮只做了 8000。
