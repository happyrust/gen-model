# gen-model #85 收口：90 件 off 已归零，剩归因登记与验收发布

> 日期：2026-11-03。背景 issue：`happyrust/gen-model#85`（AMS 剩余 90 件 off 的小族归因）。
> 代码：`vendor/e3d-model`（main，HEAD `32f8c21`，**已推送**，与 github/main 同步）。
> **重要：写本方案时核实，九库终态回归（2026-11-03 00:45 summary.json）已全 exact——off/chord/film/reference_artifact 四档全 0。所以本方案不是「怎么修」，而是「怎么收口」：把剩余归因补全、遗留账登记、证据发布、请求关闭 issue。**

## 0. 证据基线（写方案时核实过的事实）

| 项 | 现状 | 出处 |
|---|---|---|
| 终态九库回归 | 2026-11-03 00:45 重跑（E12 后二进制）：**9 库 19 roots、both 238,860、exact 238,860、off 0、chord 0、film 0**；7356 单库 1064/1064；7999 四 roots 13,674/13,674（含 ATTA 501+689、BRAN 192+235+257 全 exact） | `verification/ams-rvm-20261030/summary.json`（重写于 11/3 00:45）；`gen/ams*/compare.json`（11/2–11/3） |
| e3d-model HEAD | `32f8c21` docs: E12 成功记录（§47）；前一提交 `dc416e6` fix(datum) 倾斜宿主轴 CUTB 补足——即 E12，收掉 7356 最后 2 条 chord | `git log` |
| 推送状态 | `main...github/main` 无 ahead/behind，已推送；工作树只有用户未跟踪文件 `.cursor/`、`tests/relocation_noise_real.rs`（不动） | `git status -sb` |
| ATTA 残留件 refno | `/GENERATED/ATTA 1 of BRAN /1RCV0188-88.9-VMC-R23-R343`（root `=24383/73927`），旧快照 Y 短 10.07mm、size_ratio [1.0, 0.967, 1.0]、center_shift [0,-5.036,0]、FacetGroup→FacetGroup | `off_attribution.json`（10-30 快照） |
| BRAN 残留件 refno | `BRAN /1RCV0199-60.3-VMC-R23-R343/B1` 名下隐式管，旧快照 6 vs 7 Cylinder、差 0.00037mm | 同上 |
| ATTA/BRAN 消失区间 | 10-30 快照仍 off → 11-03 终态 exact。区间内相关提交：`ebcacfe`（SEXT BANG 旋进目录件标架，10-31）、`82b96d8`（HELE CLFL 出白名单，11-01）、`9b9fb3e`（torus 粗环密度，11-02）、`e81381c`（共享 TEE 贯通管身归位，11-02）、`dc416e6`（E12 CUTB，11-03）。**CLFL 白名单从未含 ATTA，82b96d8 排除；E12 只碰 SJOI datum，排除** | `git log`、`git show 861c811`（白名单只剩 HELE 一行） |
| 其他遗留账 | 7999 only_baseline 1475（PJOINT 894 + SCOJOINT 533 + BRANCH 40 + BEND 6 + CTORUS 1 + PYRAMID 1）、only_generated 118（全是 `/GENERATED/TUBI`）、kind_differ 6（73927：CONE 5 + VALV 1） | `summary.json` roots[].by_noun、`off_attribution.json` one_sided |
| 隐式管口径 | `container_tubes` 逐根配对（`fa72312` 10-22、`db34054` 10-23）下 7999 三个有容器组的 root 全 exact；only_generated TUBI 是 `/GENERATED/...` 键名差异，**不是几何缺失** | compare.json `container_tubes.tally` 全 exact |
| E12 遗留风险 | `cutb * lateral` 是**经验拟合不是推导**：GJTORG 反编译里 CUTB 乘的是方向向量 v44 并带 `-(v47·CUTP)/(v44·CUTP)` 项，v44 由 `sub_103B0354`/`sub_103AFF74` 填充未翻；全库仅 2 件命中门控，外推边界未验证 | notes §47.5、`e3d-model#15`（OPEN，标题已更新为「待替换为推导式」） |
| kind_differ 6 | 73927 的 CONE 5 + VALV 1：几何 exact 但记录写法不同（矩阵/参数等价写法） | `e3d-model#7`（OPEN） |
| gen-model 工作树 | `refactor` 分支 ahead 1；大量用户未提交改动（changelog.md、CONTEXT.md、docs/adr/*），不在本方案范围、不动 | `git status` |

## 1. 目标与非目标

**目标**

1. 把最后两件（ATTA `1RCV0188`、BRAN `1RCV0199`）的消失归因**钉到具体 commit**，避免「悄悄好的、不知道为什么」。
2. 7999 的 only_baseline 1475 / only_generated 118 / kind_differ 6 逐类写出**一句有证据的归因**（记录级口径差异 vs 真漏几何，必须分清）。
3. 对 E12 经验拟合（`e3d-model#15`）做出处置决定：接受现状收口，或继续反编译求 v44 闭式。
4. 汇总成 issue #85 终态报告（修复 commit 清单 + 九库终态表 + 遗留账归因 + 关闭依据）。

**非目标**

- 不改生成代码（除非 S1 抽样发现 only_baseline 里真有漏几何，那时另立分支处理、不在本方案内）。
- 不跑新的 E3D TTY 取证——现有快照、反编译笔记、对拍数据已够归因。
- 不动 gen-model 工作树里用户未提交的 changelog/ADR/CONTEXT.md 改动；不动 vendor 未跟踪文件。
- 不追求把 only_baseline 清零（记录级差异，不是缺陷口径）。

## 2. 问题清单

| # | 问题 | 影响 | 当前证据 | 处理方向 |
|---|---|---|---|---|
| P1 | ATTA `1RCV0188` 转 exact 的 commit 未钉 | 归因完整性 | 候选：`ebcacfe`（ATTA 目录件若含双 SEXT + BANG 即中）、`9b9fb3e`（torus 面片密度影响 facet AABB——**可能只改 chord/off 判档不改几何语义**）、`e81381c` | S1 二分：候选 commit 前后各跑该单件 gen+compare |
| P2 | BRAN `1RCV0199` 隐式管 6 vs 7 转 exact 的机制未钉 | 归因完整性 | 最可能 `e81381c`（共享 TEE 贯通管身归到指向的 BRAN——生成侧真修了管身分段）；也要排除是 `db34054`/`fa72312` 对拍口径修的（10-22 已存在，10-30 快照仍 off，**口径侧排除**） | 同 S1 二分 |
| P3 | only_baseline 1475 是否纯记录级差异 | **若基准记录带几何而我们没出，是真缺陷** | PJOINT/SCOJOINT 此前 SCTN 工作已证「关节组零图元」；BRANCH/BEND/CTOR/PYRA 45 件未抽样 | S2 抽样核：每类取代表件看基准 RVM 该记录是否带几何图元 |
| P4 | only_generated 118 TUBI | 已解释，缺文字 | container_tubes 配对全 exact；`/GENERATED/` 键名是生成侧隐式管命名规则 | 写进归因，零代码动作 |
| P5 | kind_differ 6（CONE 5、VALV 1） | 已知，有跟踪 | 几何 exact、写法等价 → `e3d-model#7` | 归因文字指向 #7 |
| P6 | E12 `cutb*lateral` 经验拟合 | 外推风险：倾斜量更大的库可能失效 | notes §47.5；`e3d-model#15` OPEN | Q1 拍板：留 #15 追 v44 闭式 vs 现在继续反编译 |

## 3. 阶段与步骤

### S1 ATTA/BRAN 归因二分（只读+生成，不碰 E3D）

- 从旧 `off_attribution.json` 拿到两件 refno（§0 已抄录）。
- 对 ATTA：在 `ebcacfe` 前/`ebcacfe` 后/`9b9fb3e` 后三个点各构建 `gen_ams`，跑 `=24383/73927` root（10.4s/次）+ `rvm_compare`，看该件判档翻转点。若时间成本高，先用 `element_probe` 对单件输出 AABB 即可定位翻转 commit。
- 对 BRAN：同法，`e81381c` 前后对比该 BRAN 名下隐式管根数（elements.json / generated.rvm 记录数）。
- 产出：两行归因结论（commit + 机制一句话），写进 §9 执行记录和 issue 报告草稿。

### S2 only_baseline 抽样核（只读）

- 在 `ams7999_24383_80211.rvm`（基准）里各抽 PJOINT、SCOJOINT、BRANCH、BEND、CTORUS、PYRAMID 代表记录各 2–3 件，确认记录内**有无几何图元**（FacetGroup/Cylinder/…）。
- 零图元 → 「基准记录级差异，几何无缺失」；有图元 → 逐件列出，转 P3 真缺陷处理（另立分支）。
- 顺带确认 BEND 6 件的 only_baseline 是否同名不同键（`/GENERATED/` 前缀之类）。
- 产出：`only-baseline-census.md` 小表，写进执行记录。

### S3 E12 处置（拍板后执行）

- **Q1-a（推荐）接受现状**：`cutb*lateral` 带窄门控（`lateral<1e-2 && |axis.z|>0.999`，全库只中 2 件）+ `#15` 留开追 v44 闭式（`sub_103B0354`/`sub_103AFF74` 反编译）。理由：门控窄、样本零回归、九库全绿；`#15` 已如实记录风险。
- **Q1-b 继续反编译**：翻那两个子函数求闭式替换经验式，完成后重跑九库回归。成本不可估，收益是把 #15 关掉。
- 若选 b：工作全部在 `docs/sjoi-frame-reverse-notes.md` 既有流程里继续，替换式通过九库回归才允许替换经验式。

### S4 归因落账与发布

- 写 issue #85 终态评论（草稿先经用户过目）：修复 commit 全清单（按小族）+ 九库终态表 + ATTA/BRAN 归因 + only_baseline/only_generated/kind_differ 逐类归因 + E12 caveat 指向 #15 + 关闭建议。
- `vendor/e3d-model` changelog / notes §48 补一行终态指针（若仓库有此惯例，先看再写）。
- gen-model 侧 `docs/verification/` 如需更新汇总 JSON，单独列出文件不动用户改动。

## 4. 验收标准

1. ATTA、BRAN 各有一行「commit + 机制」归因，且二分证据（翻转前后判档）存档。
2. only_baseline 六个类型都有抽样结论；若全部零图元则登记为「记录级口径差异」，否则缺陷件单列。
3. issue #85 终态评论草稿经用户确认后发出；关闭与否由用户决定。
4. **零代码改动**为本方案默认路径；任何因抽样发现的新缺陷一律另立分支/方案，不在本方案内顺手修。
5. 全程不碰 `D:/AVEVA/Projects/E3D3.1` 原项目、不跑 TTY 写宏、语料只读。

## 5. 风险

| 风险 | 应对 |
|---|---|
| R1 ATTA/BRAN 二分发现「转 exact 是副作用而非正修」（如 `9b9fb3e` 只改了面片密度掩盖判档） | 如实写归因（含「判档口径变化」字样）；必要时补一个钉住该件的回归测试 |
| R2 only_baseline 抽样发现 PJOINT/SCOJOINT/BRANCH 有真几何漏出 | 停止收口，把缺陷件列成新子项，另立修复方案（不拦 #85：off 已零，漏出属 only_baseline 口径） |
| R3 E12 若继续反编译推翻现有拟合 | 新推导式必须九库回归全绿才可替换；替换前经验式不撤 |
| R4 issue 报告发布后 owner 要求补证 | 预留 TTY 只读导出为兜底取证路径（沿用既有隔离副本边界，需逐次批准） |

## 6. 待拍板（请直接批注）

- **Q1** E12 经验拟合：**接受现状收口 #85**（推荐：门控窄、零回归、#15 留开追踪）还是**继续反编译**求 v44 闭式后再收？
- **Q2** only_baseline 1475 件的归因粒度：S2 抽样到「类型级零图元」即可收口，还是要求逐件解释？
- **Q3** issue #85 终态评论发出后：由我起草请你发 / 我直接发（若 token 可用）/ 只存草稿？
- **Q4** 归因二分若发现转 exact 是靠判档口径（R1），是否接受「口径修复」作为该件的归因？

## 7. 安全边界（沿用）

- 不碰原项目 `D:/AVEVA/Projects/E3D3.1`；不启动/关闭 E3D GUI、不杀 des.exe。
- 语料 DB 只读；gen_ams/rvm_compare 输出只写 `verification/` 新子目录，不覆盖既有快照。
- 不跑任何写库宏（RECONNECT/SAVEWORK/FLUSH 一律不用）。
- gen-model / e3d-model 工作树里的用户未提交、未跟踪文件一律不动。
- commit / push 每次请示；凭据不写进任何文件。

## 8. 拍板记录（2026-11-03）

- 用户口头批准（「已同意开始修复」），按推荐项执行：Q1-a 接受 E12 经验拟合现状、`e3d-model#15` 留开追 v44 闭式；Q2 抽样到类型级即可；Q3 issue 评论由我起草、用户发布；Q4 接受「口径修复」作归因。
- Plannotator gate 命令已发（`annotate --gate --json --result-file`），结果文件若生成见 `plannotator-result.json`。

## 9. 执行记录

### S1 归因（已完成，证据确凿）

- **P2 BRAN `1RCV0199/B1`（6 vs 7 管）→ `e81381c`**：commit message 直接点名该件——`TEE 24383/74391`（CREF=24383/75551，SPAMAP 带 INT_MIN+1 哨兵）的贯通管身按 GTTUBE 归到对方 BRAN；修复后该 BRAN off→exact、库 off 2→1。
- **P1 ATTA `1RCV0188`（Y 短 10.07mm）→ `9b9fb3e`**：二分实证。`e81381c` 后仅剩 1 off 且窗口内唯一非 datum 改动是 9b9fb3e；该 ATTA 目录件 `/NPSTE080`（GMSE 23984/98771）= 正体 SCTO+SCYL、SCYL 下两只 NSCT 负体——正是「torus 参与布尔」场景。bisect worktree 在 `9b9fb3e^`(e90d052) 重跑 =24383/73927：该件 `local_aabb.y_max 46.3807 → 56.4532`，**差 10.0725mm 与 issue 记录的 10.07mm 精确吻合**；面数 88→34 符合细网格→粗环。

### S2 only_baseline 抽样（已完成，详见 `only-baseline-census.md`）

- **真缺失 ≈1,429 件**：PJOINT 894 + SCOJOINT 533（基准 RVM 里是管架节点八角环 FacetGroup，生成侧零产出）+ CTORUS/PYRAMID 零散 2。→ 新 issue 草稿 `new-issue-pjoi-scoj-draft.md`。
- **口径差异 158 件**：BRANCH 40（E3D 管写容器组、我们写 TUBI 键）+ TUBI 118（/GENERATED/ 键名），几何均由 container_tubes 全 exact 覆盖。
- **微缺失 6 件**：BEND（~10mm 微 torus）。

### S3 E12 处置（按 Q1-a）

- 接受 `cutb*lateral` 现状收口；`e3d-model#15`（OPEN，标题已是「待替换为推导式」）留开追 `sub_103B0354`/`sub_103AFF74`。

### S4 落账与发布（进行中）

- `cargo test --lib` @`32f8c21`：**425 passed / 0 failed / 5 ignored** ✅
- issue #85 终态评论草稿：`issue85-final-comment-draft.md`（含终态表 + 逐族归因 + 遗留账 + 关闭建议），**待用户发布**。
- vendor/e3d-model `changelog.md` 追加 2026-11-03 条目。
- 未做任何代码改动；未 commit/push（按边界每次请示）。
