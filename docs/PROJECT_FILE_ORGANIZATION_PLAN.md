# gen-model-fork 文件整理方案（草案）

> 目标：在不立即移动/删除文件的前提下，给出后续整理 gen-model-fork 仓库的结构化建议，便于分批实施。

---

## 1. 背景与范围

- 当前仓库根目录堆叠 60+ 个 Markdown / TOML / Shell / JS 文件，查阅成本高。
- `docs/`、`issues/`、`开发文档/` 三套文档体系并行，主题交叉但缺少统一索引。
- `scripts/`、`cmd/` 与根目录脚本重叠，实际入口位置不明确。
- 本方案 **只输出规划**，不做任何物理移动；后续执行可按“按目录分批”策略推进。

## 2. 当前结构概览（需重点整理的区域）

| 区域 | 现状特点 | 痛点 |
| --- | --- | --- |
| 根目录 | README、CHANGELOG、Color/Room/CSG 等专题文档，且混有若干 `*.sh` / `*.js` / `DbOption*.toml` | 文档入口无法区分“指南/架构/报告”；脚本散落难以复用 |
| `docs/` | 已有 `architecture/`, `database/`, `deployment/`, `guides/`, `xkt-generator/` 等分类 | 仍有大量专题文档留在根目录 / `开发文档/`，难以统一索引 |
| `issues/` | 分主题存放问题排查记录 | 与根目录孤立的“大型问题文档”重复 |
| `开发文档/` | 中文专题/计划/分析文档集合 | 与 `docs/` 并存；命名是中文目录，不易在 CI/脚本中引用 |
| 脚本 | `scripts/`, `cmd/`, 根目录 `*.sh/*.js/*.cjs/*.mjs` | 执行入口分散；路径变动风险高 |
| 配置 | 多个 `DbOption*.toml`、`ColorSchemes.toml`、`web-test/DbOption.toml` 等 | 用途差异缺少汇总说明，易误用 |
| 代码 | `src/`, `src/bin/`, `examples/`, `web-test/`, `frontend/` | 结构尚可，但需要配套索引文档说明入口 |

## 3. 整理原则

1. **根目录精简**：仅保留项目入口文档（`README*`、`CHANGELOG*`、顶层架构说明）、构建配置、工作流脚本入口。
2. **文档分区**：按“用途（指南/架构/报告）+ 主题（房间/CSG/颜色等）”统一归档至 `docs/`，`开发文档/` 收编为 `docs/dev-notes/`。
3. **脚本集中**：以 `scripts/` 作为唯一入口目录，再根据用途（db/debug/export/ci）划分子目录；历史 `cmd/` 逐步迁移。
4. **配置显式说明**：不急于移动文件，先在 `docs/config/` 中写清每个 `DbOption*.toml` 的用途、覆盖方式、默认路径。
5. **小步执行**：每次只迁移一类文件并更新引用/README，移动后立即确认 `cargo check` / `scripts/*` 可运行。

## 4. 分区域建议

### 4.1 根目录文档

| 动作 | 示例 | 说明 |
| --- | --- | --- |
| 保留入口 | `README.md`, `CHANGELOG.md`, `README_WEB_UI.md`（可改为链接） | 若 `README_WEB_UI.md` 下沉到 `docs/guides/`，需在根 README 添加指向 |
| 移入 `docs/architecture/` | `ARCHITECTURE_COMPARISON.md`, `SPATIAL_VISUALIZATION_IMPLEMENTATION.md`, `ROOM_API_DESIGN.md`, `ROOM_CALCULATION_API_IMPLEMENTATION.md`, `CSG_*_IMPLEMENTATION.md` 等 | 统一归档架构/实现类文档，避免散落 |
| 移入 `docs/status/` or `docs/reports/` | `FINAL_REPORT.md`, `DELIVERABLES.md`, `REFACTORING_STATUS.md`, `REFACTORING_SUMMARY_zh.md`, `FULL_NOUN_OPTIMIZATION_PLAN.md`, `EXECUTION_ORDER_FIX.md` 等 | 形成“阶段报告”目录 |
| 移入 `docs/guides/` | `SPATIAL_VISUALIZATION_QUICKSTART.md`, `ROOM_CALCULATION_UPDATE.md`, `README_WEB_UI.md`（可改名 `web_ui_guide.md`） | 区分操作指南与架构文档 |
| 迁往 `docs/color-scheme/` | `COLOR_SCHEME_INTEGRATION.md`, `COLOR_SCHEME_QUICKREF.md`, `COLOR_SCHEME_USAGE.md` 等 | 专题目录 + README 汇总 |
| 迁往 `docs/task-management/` | `TASK_MANAGEMENT_DEVELOPMENT_PLAN.md`, `TASK_MANAGEMENT_IMPLEMENTATION_SUMMARY.md` | 与 `docs/development/` 衔接 |

### 4.2 `docs/` 体系

1. **新增/调整子目录建议**：
   - `docs/architecture/`
   - `docs/guides/`
   - `docs/reports/`（或 `docs/status/`）
   - `docs/csg/`
   - `docs/room/`
   - `docs/color-scheme/`
   - `docs/task-management/`
   - `docs/dev-notes/`（用于收编现有 `开发文档/`）

2. **`DOCS_ORGANIZATION.md` 更新**：
   - 列出上述子目录及示例文件
   - 标注“根目录不再直接存放专题文档”
   - 对 `development/`（当前英文目录）与 `开发文档/`（中文目录）做整合说明

### 4.3 脚本与工具

| 目标 | 实施动作（后续分批） | 注意 |
| --- | --- | --- |
| 统一入口 | 在 `scripts/README.md` 列出脚本分类与调用方式 | 先建立 README，迁移脚本后逐步更新 |
| db 专区 | 将 `cmd/run_surreal_*.sh`, `cmd/connect_surreal_*.sh`, `check_database_status.cjs` 等迁至 `scripts/db/` | 迁移后需更新文档/脚本中的路径引用 |
| export/debug 专区 | `debug_manifold_issue.js`, `debug_xkt_*`, `test_xkt_generation.sh`, `./run_model_gen_test.sh` 等迁至 `scripts/debug/` 或 `scripts/export/` | 可保留顶层代理脚本指向新路径以兼容旧命令（可选） |
| CI/部署 | 将 `deploy_*.sh`, `apply_lod_fix.sh`, `check_refno_*` 等分类到 `scripts/deploy/` or `scripts/ci/` | 与 GitHub Actions / 文档保持同步 |

### 4.4 配置与数据

- **短期（文档）**：新增 `docs/config/DB_OPTIONS_OVERVIEW.md`，内容包括：
  - 所有 `DbOption*.toml` 文件清单（根目录 + `web-test/`）
  - 各自适用环境 / 端口 / 凭据说明
  - 如何通过 `DB_OPTION_FILE` 环境变量切换配置
- **中期（可选）**：建立 `config/` 目录放置所有 TOML，但需评估代码引用路径与部署脚本，建议作为“第二阶段”处理。

### 4.5 源码、示例与 web-test

- 在 `docs/architecture/SOURCE_LAYOUT.md`（或已有文档中）补充：
  - `src/api/`, `src/data_interface/`, `src/fast_model/`, `src/web_server/`, `src/bin/` 的职责
  - `examples/`、`web-test/`、`frontend/` 的用途与运行方式
- 不做物理移动，仅补“索引 + 入口指南”，方便新人快速定位。

## 5. 推荐执行顺序（供后续实施参考）

1. **文档分类**：先将根目录 Markdown 分批移动到 `docs/` 对应子目录，并更新 `DOCS_ORGANIZATION.md`、根 `README.md`。
2. **开发文档整合**：把 `开发文档/` 重命名/迁移到 `docs/dev-notes/`，必要时增加英文简介。
3. **脚本迁移（分批）**：编写 `scripts/README.md`，先迁移不在 CI 中使用的调试脚本，再迁移关键脚本并更新引用。
4. **配置说明**：撰写 `docs/config/DB_OPTIONS_OVERVIEW.md`，在 README / 构建指南中引用。
5. **索引补全**：添加源码/示例/工具的导航文档，确保 `README.md` / `docs/README.md` 的链接正确。

## 6. 风险与验证

| 风险 | 对策 |
| --- | --- |
| 文档移动后链接失效 | 使用 `rg` 排查相对路径；必要时保留短期跳转文件 |
| 脚本迁移影响自动化流程 | 每次迁移后立即运行相关脚本/CI；必要时提供旧路径→新路径映射 |
| 配置文件位置调整导致运行失败 | 第一阶段仅写说明；若未来迁移，需在代码中支持新路径或提供环境变量覆盖 |
| 中文目录影响跨平台脚本 | 将 `开发文档/` 内容迁到英文命名的 `docs/dev-notes/` |

## 7. 后续工作建议

- 维护“整理进度表”（如 `docs/cleanup_progress.md`）记录每次迁移的文件列表、提交哈希、验证步骤。
- 对仍需留在根目录的特殊文件添加注释说明，避免反复讨论。
- PR 中附带更新后的目录结构截图/表格，方便审核。

---

> 本文档为行动计划，可根据后续讨论和实际需求迭代。待确认后，可据此逐步执行文件迁移与脚本整理。
