# Python 绑定测试沙箱（testbed）

专门给 `aios_db`（PyO3 绑定）做**隔离测试**的独立环境：不用停 9099/8022 的在跑
服务、不碰 8009 正式库，就能放心测执行层的 mutating 链路（基线 / 增量 / 生成 /
房间）。

## 隔离原理（三条独立，谁也踩不到谁）

| 资源 | 生产 | 沙箱 |
|---|---|---|
| E3D 项目文件 | `D:/AVEVA/Projects/E3D3.1`（9099 服务监控中） | `testbed/projects` 副本（只镜像 `*000` 库文件目录） |
| 单实例锁 | 真实项目根下 `.gen-model.instance.lock` | 项目**副本**根下的锁——`full_init` 拿的是自己的锁 |
| SurrealDB | 8009（`.surreal/ams-8009`） | 8019（`testbed/.surreal/pytest-ams`），可与 8009 同时跑 |
| mesh 产物 | 仓库 assets | `testbed/meshes` |

`projects/`、`.surreal/`、`meshes/`、`out/` 都被 .gitignore 排除——坏了、想从头
再来，删掉重灌即可，零心理负担。

## 使用步骤

```powershell
# ① 灌项目副本（一次性；想刷新/还原也是它，/MIR 镜像语义）
.\python\testbed\Sync-TestbedProjects.ps1

# ② 起沙箱 SurrealDB（独立终端常驻；用仓库自带 fork 2.1.x，版本守卫继承 8009 脚本）
.\python\testbed\Start-TestSurreal.ps1

# ③ 跑全链路冒烟：解析 → 基线(按需CATA) → 单根生成 → 导出OBJ → 房间/收尾
cd python
.venv\Scripts\python.exe testbed\run_full_loop.py
```

首跑会做 7997 的全量基线（分钟级）；之后水位在位，重复跑只走生成与收尾
（`--force-baseline` 可强制重基线，`--parse-only` 只测解析层）。

## 常用变体

```powershell
# 换库/换样本构件
.venv\Scripts\python.exe testbed\run_full_loop.py --dbnum 7998 --name /SOME-EQUI

# 交互调试：所有 aios_db API 直接对着沙箱用
.venv\Scripts\python.exe -i -c "import aios_db; aios_db.set_config(r'testbed\DbOption-pytest')"
```

现有 `scripts/smoke_m1..m5.py` 仍指向真实项目与 8009（历史验收基线），沙箱
不动它们；要在沙箱里复跑，把脚本里的 `set_config` 指到 `testbed/DbOption-pytest`
即可。

## ams8000 空间树启动初始化 + 增量回放（spatial_tree_8000.py）

用真实 ams8000 数据测三件事：**增量部署下空间树的启动初始化裁决**（快照新鲜
reused / 缺失 rebuilt / 库侧 epoch 漂移 rebuilt / 携带待重放意图 replayed /
字节损坏 rebuilt——每种现场一个新进程，因为 `full_init` 每进程只能一次）；
**逐会话窗口的增量回放**（以 issue-019 夹具的 db8000 sesno-24 快照为基线，拿
真实文件逐窗 `apply_file`，sesno 25/26 是已知的两次真实删除，每窗做**值级**
树校验——`spatial.tree_dump()` 与库指针值逐条双向比对，之后再重启断言
reused）；以及**双库对拍**（第二个内存实例 @8073 拿夹具 final-26 直接
baseline@26 建库，与「基线@24 + 增量回放到 26」的产物比 `(refno, aabb哈希)`
集合与树内容——增量 == 全量的空间树版本，`--skip-oracle` 可关）。
自起自杀两个一次性内存 SurrealDB（8072/8073），全程不碰 8009/8019/8071。

```powershell
# 主 crate 变过要先重建 pyd（cd python; $env:VIRTUAL_ENV=...; maturin develop）
.venv\Scripts\python.exe testbed\spatial_tree_8000.py                  # 默认 6 个窗口
.venv\Scripts\python.exe testbed\spatial_tree_8000.py --skip-windows   # 只跑启动矩阵
.venv\Scripts\python.exe testbed\spatial_tree_8000.py --max-windows 4 --gen-roots 2
```

报告在 `testbed/.spatial8000/report.json`，逐阶段日志在 `.spatial8000/logs/`。
驱动会临时把项目副本里的 `ams000/ams8000_0001` 换成基线快照（结束后逐字节
还原，异常退出后下次启动自动补还原）——**跑它期间勿并行跑 run_full_loop.py
或 pytest 房间档**（同一把项目锁 + 同一个库文件）。

## 注意

- SurrealDB 必须用仓库自带 fork 2.1.x（`Start-TestSurreal.ps1` 已带版本守卫），
  PATH 上的 3.x 会把 RocksDB 数据目录写坏（见 `Start-Surreal8009.ps1` 头注释）。
- **`.surreal/ams-8009`（正式库）已被 3.x 写坏，且决定不修**：测试一律用新建的
  独立数据目录——本沙箱的 8019，或房间增量 pytest 那个进程退出即丢的内存实例
  （`python/tests/`，8071）。真要恢复正式库，用 `sync.baseline` 重建基线即可，
  但那是另一件事，不该挡住测试。历史验收脚本 `scripts/smoke_m1..m5.py` 里还写着
  8009 与真实项目路径，在沙箱里复跑要先把 `set_config` 指到 `DbOption-pytest`。
- 沙箱数据目录若被写坏：停库 → 删 `testbed/.surreal` → 重起重基线，几分钟的事。
- `DbOption-pytest.toml` 的键面与仓库根 `DbOption.toml` 保持同步（差异只有
  project_path / v_port / meshes_path / http_api_addr 四处），根配置增删必填键
  时这里要跟着改，否则 config 反序列化报 missing field。
