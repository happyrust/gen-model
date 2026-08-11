# aios_db — aios-database 的 Python 调试绑定

给解析 / 模型生成 / 增量计算三条链路提供与生产**完全同源**的 Python 接口，
消灭「在纯 Python 里手工复刻 dabacon 解析」的重复劳动。设计决策与全函数清单见
`docs/plans/2026-08-11-python-binding-api-plan.md`。

## 构建与安装

```powershell
cd python
uv venv .venv                          # 首次
uv pip install maturin --python .venv  # 首次
$env:VIRTUAL_ENV = (Resolve-Path .venv).Path
.venv\Scripts\maturin.exe develop            # 日常调试（debug，解析/查询够快）
.venv\Scripts\maturin.exe develop --release  # 跑生成类操作前（OCC 布尔运算 debug 慢）
```

- 绑定 crate 是仓内 workspace 成员，与主 crate 共享 `Cargo.lock` / `target/` /
  `[patch]` vendor 重定向（`Toggle-LocalDeps.ps1` 同样生效），OCC 只编一次。
- abi3 wheel：一个 pyd 通吃 Python ≥ 3.10，升 Python 不用重编。
- OCCT 是动态链接：包装层（`pysrc/aios_db/__init__.py`）在导入扩展前把 PATH
  上所有目录注册进 DLL 搜索路径，正常情况无需手工干预。

## 三层初始化（与单实例锁的关系）

| 层 | 入口 | 能做什么 | 锁 | 与在跑服务共存 |
|---|---|---|---|---|
| 解析层 | `aios_db.parse.*` | 纯文件解析 | 无 | ✔ 完全共存 |
| 连接层 | `aios_db.connect()` | 只读查询 + `model.export_obj` + 窗口/队列/空间/房间观察（`incr.resolve_window` / `incr.queue_status` / `spatial.status` / `room.code|nodes|names`） | 无 | ✔ 可边跑边查 |
| 执行层 | `aios_db.full_init()` | 增量 apply / 模型生成 / 房间 / 基线 / 副作用收尾（`incr.drain_side_effects` + `spatial.reconcile|persist|rebuild`） | 拿单实例锁 | ✖ 必须先停服务 |

- **`aios_db.set_config(path)` 必须最先调用**（解析层深处也读全局 DbOption，
  配置是进程级 OnceCell，第一次被读走后不可更换）。
- mutating 函数在未 `full_init` 时直接抛 `RuntimeError`（硬守护）；`full_init`
  拿的是与服务同一把项目锁，服务在跑时会直接失败——这是防线不是缺陷。
- refno 输出统一 `a_b` 形态（与库内 `pe:` record id 一致，拿到即可拼 SurrealQL）；
  输入宽容 `a/b` / `a_b` / `pe:a_b` / `=a/b`。

## 快速上手

```python
from pathlib import Path
import aios_db

repo = Path(r"d:/work/plant-code/old/gen-model")
aios_db.set_config(str(repo / "DbOption"))     # ① 永远第一句

# ── 解析层：不连库 ────────────────────────────────────────────────
f = r"D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams7997_0001"
aios_db.parse.header(f)                        # 头/最新会话
aios_db.parse.sessions(f)                      # 会话页列表
aios_db.parse.collect_changes(f, 100, 102, detail=True)   # 增量窗口变更
aios_db.parse.element(f, "24381_100677")       # 单元素属性（文件直读）
aios_db.parse.element(f, "24381_100677", sesno=97)        # 历史版本
aios_db.parse.noun_dict(r"D:/AVEVA/Everything3D2.10/attlib.dat")  # noun 能力矩阵

# ── 连接层：只读（服务可以在跑）────────────────────────────────────
aios_db.connect(cwd=str(repo))                 # cwd=仓库根（resource/surreal 按 CWD 找）
aios_db.db.query("SELECT count() FROM pe WHERE dbnum=7997 GROUP ALL;")
aios_db.db.by_name("/-RX-CUP-001FA")           # → ["24381_100677"]
aios_db.db.watermark(7997)
aios_db.db.preview_manual_update()
aios_db.model.export_obj("24381_100677", "out_obj")  # 导 OBJ 目视（连接层即可）

# ── 执行层：mutating（先停服务）──────────────────────────────────
aios_db.full_init(cwd=str(repo))               # 拿单实例锁 + run_cli 前置段
aios_db.incr.apply_file(f)                     # 增量窗口（默认水位+1..=最新）
aios_db.model.ensure("24381_100677", force=True)   # 单根重生成
aios_db.room.drain()                           # 消化房间重算积压
aios_db.sync.baseline(7999)                    # 从未解析过的库补全量基线

# 零售组合收工前的收尾三件套（批次闭环 execute_manual 内置这些，不用手调）：
aios_db.incr.drain_side_effects()              # SystDerived / RefRevMaintain
aios_db.spatial.reconcile()                    # 空间意图收敛 + 树落盘
aios_db.spatial.persist()                      # 兜底：树脏了但没意图时落盘

# 观察面（连接层即可，服务在跑也能用）：
aios_db.incr.resolve_window(f)                 # 预览下一增量窗口（不执行）
aios_db.incr.queue_status()                    # 本进程队列 {paused, rows}
aios_db.spatial.status()                       # 空间收敛积压 {pending, stalled}
aios_db.room.code("24381_100677")              # 房间编码（无归属 None）
aios_db.room.names("24381_100677")             # 穿越的房间号列表
```

在跑服务的任务/队列/进度要用 HTTP 客户端问（`aios_client.py`，零依赖）：

```python
from aios_client import AiosClient
c = AiosClient()                  # http://127.0.0.1:8022（其他部署传 base，如 AiosClient("http://127.0.0.1:9099")）
c.health(); c.dbnums(); c.queue()
for ev in c.watch_tasks():        # WebSocket 任务事件（需 pip install websocket-client）
    print(ev)
```

分工口径：**服务在跑 → `aios_client` 观察；服务停了 → `aios_db` 深入。**

## 脚本目录

| 脚本 | 用途 |
|---|---|
| `scripts/smoke_m1..m4.py` | 各里程碑验收冒烟（M4 覆盖 4 个★新入口） |
| `scripts/smoke_m5.py` | 纯 Python 闭环缺口补齐冒烟（spatial / 副作用 / 窗口 / 房间查询；`--full` 跑执行层段） |
| `scripts/demo_noun_caps.py` | 示范：noun 能力矩阵（替代 `gm_noun_caps_probe.py`） |
| `scripts/demo_element_diff.py` | 示范：单元素「文件 vs 库」一致性 + 历史回放 |
| `scripts/demo_export_obj.py` | 示范：按名字定位构件并导出 OBJ 目视 |

## 已知代价与坑

- 长任务（整库生成等）期间 Ctrl+C 不能立刻中断 Rust 侧，只能等当前调用返回。
- 零售组合（`apply_file` / `drain_data` / `room.drain` / `model.gen*`）**不会**像
  批次闭环那样自动收尾提交后副作用——收工前依次调 `incr.drain_side_effects()`
  与 `spatial.reconcile()`（`execute_manual` 的队列闭环内置这些，无需手调）。
- `db.query` 返回干净 JSON（`Thing` 转简单形态），与 HTTP API 字段同源。
- `parse.element` 是原始解析（不处理 UDA）；`name` 字段以文件内记录为准，可能为空。
- SurrealDB 服务端必须用仓库自带的 fork 2.1.4（`scripts/Start-Surreal8009.ps1`），
  **不要**用 PATH 上 cargo install 的 3.x 打开数据目录——它的 RocksDB 会把 SST
  升级到旧版读不回的 format_version（见脚本头注释）。
