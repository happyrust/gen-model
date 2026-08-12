# db8000 会话快照夹具 · 增量对比测试计划（2026-08-12）

> 目标：把「E3D TTY 造一次真实变更 → 抽出前后数据库文件 → 离线做增量对比、还原历史」
> 固化成可重复的录制/打包/回归管线。以 dbnum=8000 起步，回归测试进 GitHub CI
> （`.github/workflows/windows-tests.yml`），不依赖 E3D 环境。
> 参照物：issue-019/020 夹具（`tests/fixtures/issues/`）、`tests/db8000_two_delete_fixture.rs`、
> `scripts/e3d/increment_fixture/`（9 场景族）、ADR-019（TTY 无人值守通道）。

## 0. 现状盘点（三种既有造例方式，本计划的输入）

| 方式 | 载体 | 依赖 | CI 可移植 |
|---|---|---|---|
| A. 手工宏对 | `scripts/e3d/*_apply.mac` + `*_restore.mac`（稳定命名、`$P` 哨兵、`ALPHA LOG`、每阶段恰好 1 个 `SAVEWORK`） | 真实 E3D + TTY 注入（`launch_detached.ps1`） | 否 |
| B. l3_suite 自动化 | 内置 SCENARIOS + `--fixture-manifest`（`increment_fixture/fixture-manifest.json`：data/transform/geometry/boolean/owner/add/delete/room-member/room-structure 九场景，setup/teardown 自建 SITE） | 真实 E3D + RocksDB + HTTP 服务 | 否 |
| C. issue 夹具 | 一次 TTY 录制 db8000 sesno 24→26，生成器按 session 链切快照 → zip + manifest + SHA256，`cargo test` 离线断言 | 仅录制时需要 E3D；回归零依赖 | **是**（现行 CI 唯一入口） |

**关键既有能力**（本计划直接复用，不重造）：

- `src/bin/db8000_two_delete_fixture.rs` 的 `session_chain()` + `write_snapshot()`：
  PDMS DB 文件是 append-only 会话链，沿头部 40 偏移的 session page 指针回溯，
  按每个 sesno 的 `latest_page` 截断文件并回写头指针，即可**从一个最终文件切出任意
  历史 sesno 的完整快照**。这就是「还原历史情况」的机制本体，已被 issue-019 验证。
- `src/bin/db8000_two_delete_fixture/archive.rs`：zip-deflate-9 打包、SHA256 校验、
  6 MiB 预算、路径安全解压（`aios-issue-fixture-v1`）。
- `IncrementPipeline::collect_changes(file, from..=to)`：对任意窗口离线采集
  `EleOperationData`（Add/Modified/Deleted + children_changed）。
- `manual_update::merge_net_changes`：窗口内净变化折叠（Added/Modified/Deleted/Cancelled）。
- `tests/db8000_two_delete_fixture.rs` 的四类性质断言（见阶段 3，全部平移泛化）。

现状缺口：以上全部硬编码 issue-019（3 个 sesno、3 个 refno、固定断言），
每新增一个案例就要复制一套 bin + test。**本计划把它泛化成数据驱动的通用管线。**

## 1. 阶段一 · 通用快照切割与打包工具（纯 Rust，无 E3D 依赖）

1. 抽出 `session_chain` / `write_snapshot` / archive 逻辑为可复用模块
   （建议 `src/bin/db_session_fixture/` 或 tests common），参数化：
   `--source <db文件> --dbnum <n> --recording <recording.json> --out <fixture目录>`。
2. 定义 manifest v2：`aios-session-fixture-v1`，一条会话链承载 N 个案例：

```json
{
  "format": "aios-session-fixture-v1",
  "dbnum": 8000,
  "baseline_sesno": 26,
  "archive": { "path": "db8000-sesno26-40.zip", "sha256": "…", "max_bytes": 6291456 },
  "snapshots": [ { "role": "final", "sesno": 40, "path": "…", "sha256": "…" } ],
  "cases": [
    {
      "id": "equi-add-box",
      "apply_sesno": 27, "restore_sesno": 28,
      "refs": { "target": "24384/xxxxx", "owner": "24384/yyyyy" },
      "expected": {
        "apply_ops":   [ {"refno": "target", "op": "Add", "noun": "BOX"} ],
        "net_window":  [ {"refno": "target", "net": "cancelled"} ]
      }
    }
  ]
}
```

3. **只入库最终文件**：中间历史快照全部在测试运行时切割重建（issue-019 存 3 份
   快照 zip 4.5 MiB；只存 final 约 1.5 MiB/链，同预算能装下约 4 倍案例量）。
   manifest 为每个案例的 apply/restore sesno 记录切割后快照的 SHA256 作还原对账。
4. 每次切割后强制验证闸：`PagedDbSession::open(snapshot).sesno == 期望值` +
   关键 refno 存在性探针（沿用 `require_presence`）。
5. issue-019 现有夹具不动（继续作为回归），新工具需能重放它作自检。

**验收**：对现存 issue-019 zip 里的 final 文件运行新工具，能切出 sesno 24/25 快照
且 SHA256 与 manifest 记录一致。

> **阶段一 as-built（2026-08-12）**：已落地。模块 `src/bin/db_session_fixture/`
> （`session_cut` / `format` / `archive_util`，10 个单测）接上 bin 入口
> `src/bin/db_session_fixture.rs`：`pack --recording --out [--source --dbnum --force]`
> （台账逐切过 sesno+存在性验证闸、只入库最终文件、6 MiB 预算、收尾即复验）与
> `verify --fixture`（解 zip → 逐台账现切 → SHA256/大小对账 + 验证闸，与阶段三
> 回归同一套裁决）。管线主体在 `db_session_fixture/pipeline.rs`，bin 只剩 CLI 壳，
> 测试按同名声明同级模块经 `crate::` 复用**同一份实现**（不是复制品）。
> 验收由 `tests/db_session_fixture_selfcheck.rs` 实测通过（13 passed / 0 failed，
> CI 特征集同款参数）：
>
> - 切割重放：从 issue-019 final（sesno 26）现切 24/25/26，散列与其 manifest 台账
>   逐一相等；
> - **pack 往返**：把 issue-019 的删除序列改写成录制单，以其 final 为源跑完整
>   pack → 复验全绿，台账 {24,25,26} 的散列与 issue-019 当年从**源文件**独立
>   切出的那三份逐一相等（两条录制路径产出同一批字节），夹具形状核对
>   （zip 单条目 + manifest + SHA256SUMS，cuts/final 不入库），并做防伪：
>   台账改一个十六进制位后复验必须以「历史还原对账失败」变红。
>
> 这条 pack 覆盖是**阶段二的前置**：录制是一次性的，pack 有 bug 就要再占一个
> 生产空窗重录，所以先在真实 db8000 数据上把它跑通。issue-019 冻结未动。
> 备注：`recording.json` 的 dbnum 为权威，CLI `--dbnum` 仅交叉核对；存在性闸按
> 方案原文只做 sesno + presence，noun 声明留给阶段三断言消费。

## 2. 阶段二 · db8000 录制会话链（一次性，需真实 E3D + TTY）

1. 录制脚本（PowerShell，复用 `launch_detached.ps1` 通道与 ADR-019 约定）：
   - 前置校验：目标文件确属 dbnum=8000、已入 MDB、登录用户可写（同
     `Run-E3DFixtureSuite.ps1` 预检）；记录录制前 sesno 作 `baseline_sesno`。
   - 逐案例执行 apply 宏（恰好 1 个 `SAVEWORK`）→ restore 宏（恰好 1 个 `SAVEWORK`）。
     每案例 sesno 窗口由「baseline + 执行顺序」确定性推出，事后用切割验证闸复核。
   - 产出 `recording.json`：案例序列、每案例 refno（宏日志里的 `Q REF` 回读）、宏路径。
   - 录制期间禁止 MERGE/PURGE/DB 压缩类操作（会破坏 append-only 假设）。
2. 首批案例（≥6 类变更形态，宏全部已有或微调即可）：

| 案例 id | 变更形态 | 宏来源 |
|---|---|---|
| equi-add-box | 新增（Add→净 Cancelled） | `db8000_equi_add_box_{apply,restore}.mac`（已有） |
| equi-copy | 复制子树 | `db8000_equi_copy_{apply,restore}.mac`（已有） |
| data-rename | 纯数据（改名） | 参照 `increment_fixture/cases/data_*.mac` 移植到 db8000 靶元素 |
| transform-move | 位移 | 参照 `transform_*.mac` 移植 |
| geometry-resize | 几何尺寸 | 参照 `geometry_*.mac` 移植 |
| delete-box | 删除（末位执行，restore 即重建或不还原） | 参照 `delete_apply.mac` |
| owner-move | 跨属主搬移 | 参照 `owner_*.mac` 移植（可选，首批可缓） |

3. 录完运行阶段一工具打包，夹具落
   `tests/fixtures/issues/issue-021-db8000-session-pair-suite/`（编号以实际 issue 为准）。

**验收**：`recording.json` + fixture 目录生成完毕，所有案例 sesno 验证闸通过，
zip ≤ 6 MiB。

## 3. 阶段三 · 离线回归测试（`tests/db8000_session_pairs.rs`，CI 主体）

对 manifest 里每个案例，数据驱动地跑七类性质断言（a–d 平移自
`db8000_two_delete_fixture.rs` 已验证的写法）：

- **a) 档案完整性**：zip 尺寸/SHA256/条目数与 manifest 一致，final 快照 sesno 正确。
- **b) 窗口切片**：`collect_changes(final, apply..=restore)` 的会话分区键 == 声明窗口，
  每条操作的 `op.sesno` 落在自己的分区。
- **c) 时点一致性**：从 final 采集历史窗口 == 从该 sesno 切割快照上直接采集
  （后续会话不得改写历史）。
- **d) 并集律**：组合窗口结果 == 逐会话切片之并。
- **e) 净变化折叠**：`merge_net_changes(apply..=restore)` 符合 `expected.net_window`
  （典型：add+restore→Cancelled；delete→Deleted；data/transform→Modified）。
- **f) 快照差分对账（新增的通用 oracle）**：切出 before/after 快照，
  用 `PagedDbSession::read_raw_records` 逐元素 diff（存在性/属性/children），
  要求与 `collect_changes` 的净结果一致——增量流与文件真实状态互证，
  新案例不必手写完整期望即可获得基础保障。已知噪声属性（如 CACHID）建白名单。
- **g) 历史还原**：对 manifest 声明的每个 sesno 切快照，SHA256 与记录值对账——
  证明「任意历史可从最终文件精确还原」。

**验收**：本地
`cargo test --locked --test db8000_session_pairs --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture`
全绿；故意翻转一个 expected 能红（防伪通过）。

## 4. 阶段四 · CI 接入

1. `windows-tests.yml` 的 `db8000-model-increment` job 增加一步，跑
   `--test db8000_session_pairs`（feature/locked 参数与现有步骤完全一致）。
2. 失败时 `actions/upload-artifact` 上传断言输出与切割快照的 hash 清单（便于远程比对）。
3. 保留现有 issue-019 两步不动；新旧并跑一个版本周期后再评估合并。

**验收**：GitHub Actions 上该 job 全绿一次；人为破坏 zip 的 PR 能红。

> **提前落地的一部分（2026-08-12）**：阶段一的自检
> （`--test db_session_fixture_selfcheck`）已按第 1 条同款参数接进
> `db8000-model-increment` job，排在 issue-019 步骤之后——两者共用同一份夹具，
> 通用切割工具一旦回归就在这里红，而不是等到阶段二录出一份坏夹具才发现。
> 同批把一直漏在门外的离线解析边界用例 `--test pdms_record_boundary` 也接了进来
> （它与 db8000 无关，纯粹是"离线却没进门禁"的存量缺口；job 名字因此比实际范围窄
> 半格，等 `db8000_session_pairs` 进来后一并考虑更名）。
> 第 2 条 upload-artifact 仍留给 `db8000_session_pairs`：自检的失败信息就是断言
> 文本，日志里看得全，没有值得上传的产物。

## 5. 阶段五 · 滚动扩展（常态机制）

- 新 bug 修复 → 用阶段二脚本追加录制该场景 → fixture 重新打包、manifest 版本化
  （目录名带批次日期或递增 issue 号），旧 zip 不覆盖。
- 覆盖面按 `scripts/e3d/ams_model_type_cases.json`（41 类模型类型）择高价值 noun 推进，
  与 `docs/plans/2026-08-07-e3d-model-type-increment-verification-expansion-plan.md` 的
  T1 管道目录件名单对齐。
- 仓库体积红线：`tests/fixtures/` 总量超 ~30 MiB 时启动 Git LFS 或 release-asset 下载方案评估。

## 6. 风险与对策

| 风险 | 对策 |
|---|---|
| 会话切割依赖 append-only 假设，E3D 压缩/合并会话会破坏 | 录制流程禁 MERGE/PURGE；每次切割强制 sesno+presence 验证闸；g) 的 SHA256 对账兜底 |
| restore 不完全还原（CACHID 等派生属性漂移） | f) 差分 oracle 维护噪声属性白名单；净折叠断言以 e) 的显式期望为准 |
| 夹具体积膨胀 | 只存 final 文件；单 zip 6 MiB 预算不放松；阶段五体积红线 |
| 录制顺序与 sesno 错位（宏里意外多/少 SAVEWORK） | 宏审查沿用「每阶段恰好 1 个 SAVEWORK」预检（`--fixture-check-only` 同款）；验证闸兜底 |
| dbnum=8000 被并行开发改写导致重录 | 录制产物一次性入库即冻结，后续变更走新批次追加，不改旧链 |

## 7. 执行顺序与依赖

```
阶段一（纯 Rust，可立即开始）
   └─→ 阶段二（需 E3D 环境，一次性录制）
          └─→ 阶段三（离线测试，依赖夹具产物）
                 └─→ 阶段四（CI 接线）
                        └─→ 阶段五（常态滚动）
```

阶段一与阶段二的宏移植（data/transform/geometry 三个宏对 db8000 化）可并行。
