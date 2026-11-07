# 模型生成剩余问题修复计划（汇总贴里的 12 项待解决）

日期：2026-10-21；状态：**已批准，推荐项已收口（2026-10-25）**（Plannotator `approved`，2026-10-21 23:29，无批注，D1–D9 全按推荐；2026-10-22 审核后 D6 改判、新增 D10 / D11，用户「全按推荐」，见 §3、§9）；M1–M6 已完成（M5 的 186 只 FIXING 按 D6 收口），M7 交付：10-22 23:13 净距那条线已把 gen-model `d3145d26a` 推上 `publish` 并部署、vendor 发布分支未推（线上混版本），10-23 按 D18 补交付：16:54 `publish` 又到 `384e67151`，配套 vendor 改为 main 上的 e3d-model `b758b28` / e3d-io `8a342cb`，干净检出补验全绿，待你推送（W6），M8（APS 剩余，W7）已收口（W7g–W7e、W7h、W7k、W7j、W7t 完成；2026-10-25 19:37 线上 e3d-model `da5aeb7`，库 8000 重生成 2683/2683，#70 已关。ISSUE_09、#34、FN09 按原决定不动）；2026-10-22 第二次送审（Plannotator 17:28 `approved`，无批注，见 §8）；起草：Cursor 代理；批准位：用户（Plannotator）。

> 目标：把「模型生成问题汇总（截至 2026-10-21）」里的 12 项待解决逐项落到可执行的步骤、验收标准与决策点上，
> 定下执行顺序。汇总贴草稿：`output/github-issues-20261021/03-gen-model-issue-summary.md`。
> 请在 Plannotator 里对 §3 的决策点批注（不批注即按推荐项），再整体批准或驳回。
> 2026-10-22 第二次送审看这几处：§2 的状态列、§3 新增的 D12 / D13（已按你的回复定）、§4 W6 的推送顺序、§4 W7 的剩余子项、
> §7 的风险，§9 的「M7 · 交付准备」，第二次审核（§9 末）新增的 D14–D16，以及 §9 的「M8 · W7h」「M8 · W7k」（aps7200 对拍与余下各堆的根因）。

## 1. 出发点

| 范围 | 件数 | 现状 |
|---|---:|---|
| 土建（ams1112 + ams7997 全部土建站） | 9982 | 9971 exact、4 chord、4 reference_artifact、3 film、0 off |
| ams7997 `/SSC测试` 整站（土建 + 暖通） | 4523 | 4522 exact、1 film；记录种类不同 4 |
| MDS BS 支架（ams7324） | 1477 | 1473 exact、4 chord、0 off；种类全一致 |
| FIXING 真值（MDB `/ALL` 内） | 2405 | 出体 910/910 一致 |
| GENSEC datum 真值 | 3678 | 3588 一致，90 个差一个管半径 |
| 10-11 其他专业基准 | 2872 | 2870 exact、1 film、1 off（基准本身的问题） |

代码状态：e3d-model 本地领先 18 个提交（最新 `9228a1b`），e3d-io 领先 1 个（`3bed2ba`），都未推送；e3d-model 全量测试 445 通过。
取证口径：TTY 宏固定 `LEVEL 6` / `LEVEL HVAC 6`、`export holes on`；生成器缺省同口径（`E3D_EXPORT_HOLES` 缺省 on）。

## 2. 待解决清单与归属

| # | 问题 | 工作包 | 影响 | 跟踪 | 状态（2026-10-22） |
|---|---|---|---|---|---|
| 1 | 记录种类 4 处：两只 STWALL（E3D `Pyramid`）、风管 THRE / BEND（E3D 只合被切的图元） | W1 | RVM 结构与 E3D 不同，几何一致 | ISSUE_05 | 已修（M1，`9d08daa`、`729cb50`） |
| 2 | MDS FT44 的 90 个 PLDATU 差一个管半径（VV p-line） | W5 | 当前无可见几何差别 | ISSUE_02 | 已修（M6，`5a9582e`） |
| 3 | C-IY 槽弯在 holes on 导出里差约 100 mm | W2 | 桥架几何只对上了 holes off 一种口径 | ISSUE_06 第 2 步 | 已修（M2，`ed3aa13`） |
| 4 | MDS AISC / DIN / SPECIALS、ams8000 整库、APS 项目未对拍 | W4 | 未知问题 | ISSUE_06 | MDS 三套框、ams8000 已对完（M3、M4）；APS 普查完，剩余问题与 aps7200 整库进 W7（W7g、W7b、W7c、W7a、W7d、W7i、W7e 已修，W7f 归因，W7h 对完 aps7200，W7k 修完矩形风管弯头、吊架、踢脚板、板上套管、SJOINT 与梁端负体门、SNODE 点标记、挂关节的弧墙、圆转方风管变径、零尺寸 CYLI、PIPE 下的矩形风管支管、桥架路径点记号，W7k 收口（aps7200 只剩 3 件圆弧切分 → ISSUE_09），W7j 头管 UDA 已并回、托架已随 `da5aeb7` 上线；aps250209 4623/4623、aps250212 6/6（`gates-w7v7.txt`）） |
| 5 | ams2510xx 的 186 只 FIXING 没有真值 | W4 | 未知问题 | ISSUE_06 第 4 步 | 按 D6 (b) 收口：不属于任何项目的文件，验收 2405 / 2405 |
| 6 | e3d-model#1 剩隐式 TUBE 滚角（67/585）与 3 条 SREV Z 反号 | W3 | 只影响记录矩阵，几何不变 | e3d-model#1 | 拆到 ISSUE_09（低优先级），#1 关单评论草稿 10 |
| 7 | e3d-model#5 外项目库不能自动纳入 | W3 | 漏给 `--project-dir ZDJ000` 就生成失败 | e3d-model#5 | 已修（M2，`7d9c943`） |
| 8 | e3d-model#6 修复已在 e3d-io，待复核关单 | W3 | 无 | e3d-model#6 | 复核完（M1 `390a6d6`、M2 `3f9f3c7`），#6 关单评论草稿 11 |
| 9 | gen-model#34 结构连接级联默认关 | 本计划不执行 | 改关节不重算杆 | #34 | 未执行；验收补充评论草稿 14（D9） |
| 10 | gen-model#59 范围生成崩溃残余 | 本计划不执行 | 生成流程 | #59 | 主体已随 0.1.43 上线（`aa6ccb44e`）。2026-10-25 库 8000 初始化没有中断批次，启动续跑没有复核条件 |
| 11 | FN09 负体 NXTR `24381/42378` 的退化环 | W6 | 数据问题 | 汇总贴 | 修正清单已出，待你在 E3D 里改 |
| 12 | 推送与服务端重建部署（#58 遗留） | W6 | 修复未到部署环境 | #58 | 10-22 23:13 净距那条线已把 gen-model `d3145d26a` 推上 `publish` 并部署（run 39803133375），但三个 vendor 发布分支没推，线上是 0.1.43 配 09-22 的 vendor（混版本，本计划的生成修复都不在线上）；2026-10-23 按 D18 补交付：16:54 `publish` 又被推到 `384e67151`（已同步到 W7k），配套 vendor 改为 main 上的 e3d-model `b758b28`、e3d-io `8a342cb`，干净检出补验全绿（§9「M7 · 补交付（D18）」续）；**2026-10-24 已交付**：`publish` 到时是 `cb0fbd8a5`，干净检出重验后按提交号推了三条 vendor 发布分支与 `ci/linux-build-20261024`，CI run 40108117494 部署成功，线上 `0.1.43+gcb0fbd8a5d3d…`（不带 `.dirty`）配 e3d-model `b758b28` / e3d-io `8a342cb` / old-aios-core `c9cc8bb`；两个 vendor 的 main 也推到最新；6 条 issue 评论已发；10-25 e3d-model `main` 又推到 `da5aeb7`（M8 ISSUE_09 四步，发布分支未动、随下一版）、#7 评论已发；W6 第 6 步 10-25 18:18 已做：线上 7998 / 8000 此前从未初始化，按运维面板同款 `update/execute` 初始化，8000 的 953 根全部生成、失败 0、已放弃 0；抽查 953 根 2683 条本地记录 2681 条对上，余 `/M-OR-1RX-3081D`（W7j 修复未部署）与 `/C-OR-1RX-3040A`（多 3 根隐式管身，gen-model #70）；**10-25 19:37 只换 vendor 再交付**：e3d-model 发布分支 `b758b28` → `da5aeb7`，gen-model 仍 `cb0fbd8a5`，CI run 40186936076 成功，线上 `0.1.43+gcb0fbd8a5d3d.1792927251`；8000 `model/rebuild` 953 根 0 失败，抽查 2683 / 2683 全对上（两处差异都消失，#70 已于 2026-10-25 19:48 关闭），见 §9 M7 末 |

## 3. 决策点（请批注；不批注即按推荐）

| ID | 问题 | 选项 | 推荐 |
|---|---|---|---|
| D1 | 执行顺序 | (a) 先修已知差异，再扩覆盖（§5 的 M1→M7）；(b) 先扩覆盖把问题都找出来，再统一修 | (a)：已知差异小而确定，修完再铺开覆盖时判档更干净 |
| D2 | W1b「哪些图元算被切」 | (a) 图元自己名下有负体就算被切，元素级负体（NGMS / 设计负体）仍整只一条；(b) 元素级负体也按与图元 AABB 相交拆 | (a)：先只做有证据的那一半；(b) 等普查出样本再议 |
| D3 | W5 VV p-line 何时做 | (a) 放在覆盖之后；(b) 现在做；(c) 只记账不做 | (a)：需要反查 core.dll，当前没有可见几何差别 |
| D4 | e3d-model#1 剩余的滚角 / Z 反号 | (a) 拆成一条低优先级 issue，关 #1；(b) 留在 #1 | (a)：#1 的标题项已经做到 |
| D5 | e3d-model#5 外项目库 | (a) 现在做自动纳入；(b) 继续靠 `--project-dir` | (a)：漏参数就静默少件，隐患大于工作量 |
| D6 | ams2510xx 186 只 FIXING 的真值 | (a) 建 AMS 项目副本、在副本里把库挂进 `/ALL` 再取；(b) 不做 | 原推荐 (a)；2026-10-22 改判 **(b)**：七个库不在任何项目的 SYS 库里，副本里挂 `/ALL` 的前提不成立，按「不属于任何项目的文件」记账，验收 2405 / 2405 |
| D7 | 推送与重建 | (a) 你来推送，推完我做服务端重建与抽查；(b) 你明确同意后由我推送 | (a) |
| D8 | FN09 退化环 | (a) 我给出修正清单，你在 E3D 里改；(b) 不改，保持 reference_artifact | (a)：我不写库 |
| D9 | #34 / #59 | (a) 不在本计划执行，只给 #34 补一条「改 MDS 关节→被截的杆重算」的验收；(b) 纳入 | (a) |
| D10 | vendor 发布分支（2026-10-22 新增） | (a) 推送后把 e3d-model、e3d-io 的 `ci/gen-model-linux-20260918` 快进到新 main；(b) 把 `linux-binary.yml` 的默认 `vendor_ref` 改成 main | **(a)**：工作流不改，回滚只要把分支指回 `f2ecfe9` / `5245654`；(b) 让 main 上任何推送直接进部署，没有发布闸门 |
| D11 | vendor 工作区未提交改动（2026-10-22 新增） | (a) 推送前由所属工作线提交或暂存，干净工作树重跑 V1–V9；(b) 照现状推送 | **(a)**：保证验证过的就是要部署的 |
| D12 | gen-model `refactor` 落后 `origin/refactor` 3 个（今天发布线 0.1.42）（2026-10-22 新增） | (a) merge；(b) rebase | **(a)**：历史本来就是 merge 的；18 个本地提交哈希不变，汇总贴草稿里的引用照旧有效；两边只共改 `handlers.rs`，试合并无冲突 |
| D13 | 这次部署的版本号（2026-10-22 新增） | (a) 照发布线惯例升 0.1.43（`Cargo.toml` / `Cargo.lock` / `changelog.md` / `python/aios_client.py`）；(b) 不升，靠版本串里的哈希区分 | **(a)**：服务器版本号与今天的 0.1.42 分开，客户端的期望版本跟进 |
| D14 | 干净代码上的回归门（2026-10-22 第二次审核新增） | (a) 推送前用干净检出的 `gen_ams` 重跑各门的生成，与 w7c 那轮的生成输出逐文件逐字节比（V5 比真值核对的输出），全部相同即视同 V1–V9 在干净代码上通过；(b) 接受现状：以 V6 472 通过、aps250110 逐字节相同和「未提交改动不进生成路径」的推理为准 | **(a)**：D11 批的就是干净工作树重跑 V1–V9；逐字节比只需重跑生成，aps250110 已经证明生成是确定的 |
| D15 | 部署回滚（2026-10-22 第二次审核新增） | (a) §7 写明回滚步骤：两个 vendor 发布分支强推回 `f2ecfe9` / `5245654`，再推一个 `ci/linux-build-*` 分支到 `220a6ec87` 触发重建（不动 publish），回滚后整库再重生成一次；(b) 不准备回滚，只前向修复 | **(a)**：发布分支一推，0.1.42 就没法原样重建；步骤先写好，用不用得上再说 |
| D17 | 两处载荷性的 vendor 未提交改动（2026-10-23 D14 补验发现，见 §9） | (a) 推送前把它们提进交付：old-aios-core 在 a438029 上提交 `pdms_shape.rs` 与 `Cargo.toml` 那一行（别把整文件的换行符改动带进去），推 `ci/gen-model-linux-20260918`；e3d-model 从 230d20b 开一条发布分支只加 `element_diff.rs` 那一个提交（不带 W7a 起的其他改动），推到 `ci/gen-model-linux-20260918` 与 main；然后在干净 worktree 里重跑 gen-model `cargo test --lib` 至全绿，再推 gen-model；(b) 照现状交付，把这 5 条红测试和两处行为差异记为已知；(c) 改 gen-model，让 0.1.43 不依赖这两处 | **(a)**：这两处不是测试玩具——坏 `.mesh` 的原地修复和匿名属性的增量判定都是线上行为；(b) 等于明知部署的不是测过的；(c) 倒退。用户「按推荐」，2026-10-23 已做：old-aios-core `c9cc8bb`（本地分支 `ci/gen-model-linux-20260918` 已指过去）；e3d-model 新分支 `release/0.1.43` = 230d20b + `aa567e4`，main 上同内容 `65210cc`；干净 gen-model `cargo test --lib` 2588 通过 / 0 失败 / 133 忽略。推送见 W6 第 2–3 步 |
| D16 | 容器组判档（2026-10-22 第二次审核新增） | (a) `rvm_compare` 在容器组内再按管身逐根配对、逐根判档，在 W7j 之前做；(b) 维持并集包围盒，只在 §7 记风险 | **(a)**：并集盒会掩盖单根管的位置错；W7j 剩下的三条正好要逐根看管身。用户选 (a)，2026-10-22 已做：e3d-model `db34054`（`src/bin/rvm_compare.rs`，见 §9「第二次审核 · 续」） |
| D18 | 部署组合（2026-10-23 补交付新增：线上已是 `d3145d26a` 配 09-22 的 vendor） | (a) 部署 `2639eff43`（= `7fdc4c15c` + 净距修复的 cherry-pick，配 `aa567e4`；`cargo test --lib` 2588 / 0）；(b) `d3145d26a` 原样配 `aa567e4`（2 条测试的期望已对齐 W7e，比发布版 vendor 超前；线上安全：读取集合多 EZDI / PTS / SLODIR / SZDI 四项只会多重生成，SUPC 运行时问 e3d-model）；(c) vendor 前移到 W7e：e3d-model `845f4e5` + element_diff（`df2aa7d`）去配 `d3145d26a` | 原推荐 (a)；**用户选 (c)**（2026-10-23「vendor 前移到 W7e」）：W7a / W7d / W7i / W7e 随这次部署上线，干净检出补验 2588 / 0、44 份生成与 `w7e` 一致，见 §9「M7 · 补交付（D18）」。18:55 补记：`publish` 16:54 又被推到 `384e67151`（已同步到 W7k），配套 vendor 随之改为 main 上的 e3d-model `b758b28` / e3d-io `8a342cb`（补验全绿，见 W6 与 §9），待用户确认 |

2026-10-22 审核（§9）后用户回复「全按推荐」：D6 取 (b)，D10、D11 取 (a)。交付准备时用户回复「全按推荐：merge + 升 0.1.43」：D12、D13 取 (a)。第二次审核（§9 末）新增 D14–D16；用户「按推荐继续」，D14、D15、D16 都取 (a)（D16 已做，D15 已写进 §7，D14 的补验要在 `D:\work\plant-code\deliver` 的干净检出上跑）。

## 4. 工作包

### W1 RVM 记录种类（待解决 1）

**W1a 两只 STWALL 写 `Pyramid`**

- 证据：`verification/regress-20261021/compare_ams7997_ssc_lv6-kinds2.json` 里 `/1SL-WF08-W-C-S8019` 与
  `/1SL-WF08-W-C-S8038`（`24381/15586`）的 STWA 1，E3D 一条 `Pyramid`、我们一条 `FacetGroup`，几何 exact
  （0.0023 / 0.0008 mm），都没有负体。同站其余 STWALL 两侧都是 `FacetGroup`。
- 假设：E3D 对截面是单个矩形目录图元（SREC）的型材走 DRPYR，与 SCTN 的 SREC 成员同理；我们的墙截面没有
  把目录出身交给写出端，走了拉伸体族「一律面片」。
- 步骤：
  1. 用 `element_probe` 读这两只与同站一只写 FacetGroup 的 STWALL：SPRE → 目录截面 GMSE 成员的 noun、参数。
  2. 是 SREC：在 `elmodl` 的墙截面路径把 SREC 出身与放置标架交给写出端（照 SCTN 的做法），`gen_ams`
     按 `Pyramid` 出身写。不是 SREC：用 TTY 取这两只的 `Q GTYP` 等属性找 E3D 的判据，再定。
  3. 加单测（截面是单个 SREC 的 STWALL 写 `Pyramid`）。
- 验收：这两只种类一致；土建、`/SSC测试`、MDS、S1 其余的种类与判档不变。

**W1b 只合被切的图元**

- 证据：`/-VES-E-4-L-2501` 的 THRE 1：E3D `[FacetGroup, Pyramid×4]`，我们 `[FacetGroup]`；BEND 1：E3D
  `[FacetGroup, Pyramid×2]`。我们在 holes on 下对扣过负体的件整只写一条（`9228a1b`）。IJOINT1 两块都被
  自己名下的 NSCY 切，E3D 是一条。
- 假设：E3D 只把被负体切到的图元合成一条布尔结果面片，其余图元各写解析记录。
- 步骤：
  1. 普查：在 `/SSC测试`、MDS、S1 的 E3D RVM 里列出「FacetGroup + 解析图元」混写的元素，配上我们的负体
     明细（哪个图元带负体、哪一级），确认规则。
  2. `catalogue.rs`：建每个 part 时记下它自己的负体有没有切到它（图元级 cutter 已经是逐 part 上的），
     `GeneratedElement` 带出逐 part 的「被切」标记。
  3. `gen_ams`：holes on 且只有图元级负体时，被切的 part 合成一条 FacetGroup，其余 part 走原来的逐块识别；
     元素级负体按 D2 的口径。
- 验收：这两只种类一致；IJOINT1 仍是一条（真库门 `rvm_record_kinds_real`）；其余不变。

**W1c 暖通分段弯头与零半径目录圆柱的记录种类（M1 复核 #6 时新查到，排进 M2）**

- 证据：#6 的三个区正式对拍后 212 对全 exact，记录种类不同 62。分段弯头 61 件（BEND 33、ELBO 28，目录
  `/C5SEGB` 等）的 `SSLC` 斜切节 E3D 写带斜切角的 `Snout`，我们写面片；FLEX `/1CAM501FL`（目录 `/CFLEX`）
  E3D 多落 27 条 `Snout[0, 0, 2, …]`。草稿 `12-e3d-model-hvac-record-kinds.md`（ISSUE_12）。
- 步骤：`SSLC` 块按属性直出带斜切角的 `Snout`（出身与标架已逐块带着）；零半径 `SCYL` 先用 TTY 查清
  E3D 为什么写成 `Snout`、高 2 mm 从哪来，再补 `DegenerateCatalogueShape`。
- 验收：三个区种类全部一致、判档不变；其余回归门不变；真库门加一只 5 节弯头与 `/1CAM501FL`。

### W2 C-IY 槽弯的 holes on 口径（待解决 3）

- 证据：e3d-model `evidence/20260902-repre-level-tty-experiments/data/ciy-1r330b/tbr100-C-IY-1R330-B-tty.rvm`
  （PIPE 6、obst on、holes on）对我们的生成，12 只 BEND 差约 100 mm；对 addin 导出（holes off）是 exact。
  2026-09-02 时生成对 TTY 曾是 0.001 mm（`gen-vs-tty-vs-addin-maxdev.txt`），之后生成器按 addin 收敛。
- 步骤：
  1. 按 PIPE 6 / obst on / holes on 生成 ams8000，对 TTY 重导逐件，用 `rvm_facets_dump` 看这 12 只差在哪一侧、哪一块。
  2. 查 TBR-100 槽弯目录的拼接板与负体（GMSE / NGMR、NAPP、OBST、LEVE）。
  3. 如果差异就是负体是否参与：让 `E3D_EXPORT_HOLES` 同时管几何（holes off 不扣图元级负体、holes on 扣）。
- 验收：C-IY TTY 重导在 holes on 下 35/35 exact；addin 基线在 holes off 下仍 35/35；ams8000 其余桥架不变。

### W3 已有 issue 收尾（待解决 6–8）

- e3d-model#6：用 `rvm_compare` 复核 issue 里的三个区（`/-KX-CAM-E01`、`/-KX-CAM-S01`、`/-RX-CAV-E01`）——当时因为
  大小头 / 棱台判帧拒读，`c504338` 之后可以正式对拍。结果写回 #6 后关单。
- e3d-model#1：按 D4。拆出「记录矩阵滚角」一条（隐式 TUBE 67/585、C-IY 3 条 SREV Z 反号，几何不变），关 #1。
- e3d-model#5：按 D5。`db_discovery` 读主项目系统库里的外项目表，自动纳入外项目目录；系统库同号（8189 / 8190 / 8191）
  以主项目为准。验收：只给 `ams000` + `acp000` 时 `/1PTU-INST23` 20/20 exact，`24383/66515` / `66585` 两只 ATTA
  与给 `ZDJ000` 时逐字节相同；其余回归门不变。

### W4 覆盖扩展（待解决 4、5）

1. MDS AISC（ams7329）、DIN（ams7350）、SPECIALS（ams7330）：列出各自 `DATA/FT` 下的 ZONE，
   `python output/make_tty_macro.py <标签> <ZONE...> --per-root` 导出，整库生成后 `mds_compare.py` 对拍。
2. ams8000 整库：按 ZONE 逐个导出（TTY 口径），整库生成对拍；与 W2 一起看桥架。
3. APS：aps250110 等按 SITE 导出（APS TTY 已通），aps7200 走副本 `run_copy_tty.ps1`。（2026-10-22：aps7200 整库没做，
   M5 只核过 2 只 SBFI，移到 W7h。）
4. ams2510xx 的 186 只 FIXING：按 D6 建 AMS 项目副本，在副本里把库挂进 `/ALL`，跑 FIXING 真值宏，用
   `zz_tmp_fixing_truth_check` 对拍。（2026-10-22：前提不成立，D6 改判 (b)，不做。）

每一套的新问题各开 issue 草稿。验收：off 为 0 或逐件归因；FIXING 真值 2405 / 2405（ams2510xx 的 186 只按 D6 (b) 记为不属于任何项目的文件；原写「ams000 全部 2591 只都有真值」）。
风险：TTY 在个别元素上会原生崩溃（gen-model#22 那类），宏已逐行落盘并带 cursor 日志，崩了就跳过那一件重跑。

### W5 圆管「圆周」p-line（待解决 2，时机按 D3；M6 已完成，落地走的是第 3 步的兜底，见 §9）

1. 反查 core.dll 里 p-line 位置的求值：PX / PY 为空表达式时怎么得到圆周上的点。
2. 普查目录里 PX / PY 为空的 PLIN，以及用到它们的设计件（POSL / JUSL / PLDATU）。
3. e3d-io 区分「空表达式」与「常量 0」；`datum::section_pline` / `section::member_frame` 对空表达式沿 PLAX 求与截面
   外形的交点。反查没有结果时，先只对圆管落地。
4. 验收：GENSEC datum 真值 3678/3678；FIXING 真值与其余回归门不变。

### W6 数据与交付（待解决 11、12）

- FN09（D8）：修正清单已出——`verification/civil-tty-20261021/fn09-nxtr-24381-42378-fix.md`。E3D 自己的导出里 FLOOR 2
  除主体外还有 4 块毫米级断开残片，落在 42378 与相邻负体 42389（10° 线）、42362（33° 线）共用的分界线上，以及
  42383 那个放不下的倒圆附近（切线长要 6178 mm，邻边只有 6123 / 5623 mm）。改法：把 42378 的环换成 5 点、不带倒圆
  的多边形，两侧越过板边 100 mm、与相邻负体各重叠 150 mm，删 4 个点。你在 E3D 里改后我重导 FN09 对拍，预期
  FLOOR 2 由 reference_artifact 转 exact。
- 交付（D7、D10、D11）。2026-10-22 核实：服务器由 `.github/workflows/linux-binary.yml` 构建，publish 推送时按分支
  `ci/gen-model-linux-20260918` 克隆 vendor；0.1.41 部署（10-19，run 39189885547，真实编译）用的是 e3d-model `f2ecfe9`、
  e3d-io `5245654`（都是 09-22），这条分支 09-23 后没推进过。只推 main 服务器拿不到任何修复，按下面顺序做：

  **2026-10-23 18:55 起以这一段为准（D18 更新，取代下面那段与第 2–5 步；第 6、7 步照旧）**：16:54 净距那条线又把 `refactor` / `publish` 推到 `384e67151`（CI run 39928011740，线上自报 `0.1.43+g384e67151336….dirty`，vendor 仍是 `f2ecfe9` / `5245654` / `a438029`）。`384e67151` 的读取集合与测试期望已同步到 e3d-model `28d0402`，再配 W7e 那套 vendor 测试不全绿，部署 `d3145d26a` 又会回滚三个净距修复，所以配套 vendor 改为 main 上的 e3d-model `b758b28`（含 element_diff `65210cc`；读取集合 163 项与 `384e67151` 逐项相同）、e3d-io `8a342cb`，干净检出补验全绿（§9「M7 · 补交付（D18）」续）。都按提交号推、已核快进、不加 `--force`：
  ① e3d-io `git push github 8a342cb:main`（4 个）、`git push github 8a342cb:ci/gen-model-linux-20260918`（152 个）；
  ② e3d-model `git push github b758b28:main`（60 个）、`git push github b758b28:ci/gen-model-linux-20260918`（116 个；在 main 上，下一版推进仍是快进）；
  ③ old-aios-core `git push origin c9cc8bb:ci/gen-model-linux-20260918`；
  ④ gen-model `git push origin 384e67151:refs/heads/ci/linux-build-20261023`（`publish` 已是 `384e67151`、不动）。推之前先 `git ls-remote` 核 `publish` 仍是 `384e67151`，变了就停、回来重新配。
  核对：CI 应打印 `e3d-model -> b758b28`、`e3d-io -> 8a342cb`、`old-aios-core -> c9cc8bb`，`e3d-attlib -> f37be2e`、`plant-mbd -> f888a50`、`plant-ui -> 490da62` 不变；编译日志里不再有 `Downgrading`；新二进制自报 `0.1.43+g384e67151336…` 且不带 `.dirty`。
  **2026-10-24 23:41 已按此推送**，只是 `publish` 到时已是 `cb0fbd8a5`（在干净检出上重验后改推 `cb0fbd8a5:refs/heads/ci/linux-build-20261024`，版本串应为 `0.1.43+gcb0fbd8a5d3d…`），见 §9「M7 · 补交付」续（2026-10-24）。

  **（已作废，被上一段取代）2026-10-23 17:50 那版（D18 (c)，配 `d3145d26a`）**：10-22 23:13 净距那条线把 gen-model `refactor` / `publish` 快进到 `d3145d26a`（= `7fdc4c15c` + W7 线 4 个读取集合 / 测试同步 + 净距修复）并部署（CI run 39803133375），三个 vendor 发布分支都没推：线上自报 `0.1.43+gd3145d26a71a….dirty`，配的是 e3d-model `f2ecfe9`、e3d-io `5245654`（构建时 e3d-io 0.5.0 被降成 0.3.0，`.dirty` 就是改了锁文件）、old-aios-core `a438029`。下面第 4–5 步的 `7fdc4c15c:refactor` / `:publish` 已非快进（远端 `refactor` 10-23 16:25 又被推到 `384e67151`）。按 D18 (c)，都按提交号推、已核快进、不加 `--force`：
  ① e3d-io `git push github 3bed2ba:main`、`git push github 3bed2ba:ci/gen-model-linux-20260918`；
  ② e3d-model `git push github 845f4e5:main`（50 个）、`git push github release/0.1.43-w7e:ci/gen-model-linux-20260918`（= `df2aa7d` = `845f4e5` + element_diff 的 cherry-pick，107 个；不在 main 上，见 §7）；
  ③ old-aios-core `git push origin c9cc8bb:ci/gen-model-linux-20260918`；
  ④ gen-model `git push origin d3145d26a:refs/heads/ci/linux-build-20261023`（`publish` 已是 `d3145d26a`、不动；一次性部署分支同样触发构建与部署）。
  核对：CI「Checkout vendor path dependencies」应打印 `e3d-model -> df2aa7d`、`e3d-io -> 3bed2ba`、`old-aios-core -> c9cc8bb`，`e3d-attlib -> f37be2e`、`plant-mbd -> f888a50`、`plant-ui -> 490da62` 不变；编译日志里不再有 `Downgrading`；新二进制自报 `0.1.43+gd3145d26a71a…` 且不带 `.dirty`——版本串与现在线上的只差 `.dirty` 和构建时间，以 CI 打印的 vendor 短哈希为准。干净检出补验见 §9「M7 · 补交付（D18）」。

  1. vendor 未提交改动（D11）：e3d-model 的 `src/element_diff.rs`（+117 行，10-18 起，增量 diff 那条线）、
     `tests/relocation_noise_real.rs`、`examples/zz_tmp_source_chain.rs`，以及 old-aios-core、plant-mbd、e3d-attlib 的
     未提交改动，由所属工作线提交或暂存；之后在干净工作树上重跑 V1–V9。
     **2026-10-22 已处理**（不动它们）：另开 git worktree 按将推送的提交检出一套干净的 gen-model + 五个 vendor +
     plant-ui（`D:\work\plant-code\deliver`）验证，结果见 §9「M7 · 交付准备」。这些未提交改动留在本地、不随这次上线。
     **推送前还差两件**（第二次审核）：① 按 D14 补验；② 在 `deliver/gen-model`（`7fdc4c15c`）跑 gen-model 的 `cargo test --lib`——这次带上的 13 个其他工作线提交与 `handlers.rs` 的自动合并只过了 `cargo check`，CI 也不会补（`windows-tests.yml` 不在 `refactor` / `publish` 上触发）；W7d 记下的 `partition_splits_pose_from_regen_and_respects_cancellation` 整组失败，一并确认它在 `7fdc4c15c` 上的状态（第二次审核已查明根因并修了测试，gen-model `376ce770d`，见 §9「第二次审核」；只改测试、不进 0.1.43，所以干净检出上这一条整组跑仍会红，单跑或 `--test-threads=1` 过即可）。
  2. 推送 main，一律按提交号（第二次审核起改为正文；本地 main / refactor 都已越过要交付的提交，见本节末）：e3d-io `git push github 3bed2ba:main`（1 个）、e3d-model `git push github 230d20b:main`（39 个，含 W7g 的 `e59dfb7`、W7b 的 `1cc594a` / `8f37bc5`、
     W7c 的 `0184cb6` / `f21e7a1` / `faf975e` / `230d20b`），远端名是 `github`。推之前各仓先 fetch，确认 gen-model `origin/refactor` 仍是 `220a6ec87`、两个 vendor 的 `github/main` 与发布分支都没动。
  3. 推进 vendor 分支（D10）：e3d-io `git push github 3bed2ba:ci/gen-model-linux-20260918`、e3d-model `git push github release/0.1.43:ci/gen-model-linux-20260918`（= `aa567e4` = 230d20b + element_diff，D17；不是 230d20b）、**old-aios-core `git push origin c9cc8bb:ci/gen-model-linux-20260918`（D17，从 a438029 快进）**（都是快进，不加
     `--force`）。从 09-22 算起，e3d-model 一次带上 95 个提交、e3d-io 149 个（含 #34 D 线、#35、e3d-io 0.5.0 写路径等）。
     e3d-attlib、plant-mbd、plant-ui 的发布分支已等于各自本地提交（plant-ui 的 3 个本地提交不在这次交付里），不动；old-aios-core 按上面那条推（D17 之前写的「不动」不再成立）。第 5 步核 CI 打印的 vendor 短哈希时，应为 `e3d-model -> aa567e4`、`e3d-io -> 3bed2ba`、`old-aios-core -> c9cc8bb`。
  4. 推送 gen-model：`git push origin 7fdc4c15c:refactor`（21 个：18 个本地提交——本计划的 `430451b33`、`21721c4d0`、`a34d58647`、`704f5d2f7`、
     `a67fc44f9`，其余 13 个是 #59、ISSUE-029、#50、房间等工作线，会一起上线；加 `c16b99c6b` `Cargo.lock` 随 vendor、
     `b0477ffd7` merge `origin/refactor`（D12）、`7fdc4c15c` `chore(release): 0.1.43`（D13））。
  5. `git push origin 7fdc4c15c:publish`（`220a6ec87` → `7fdc4c15c` 快进；不能写 `refactor:publish`，本地 `refactor` 已越过 `7fdc4c15c`）触发部署；必须在第 3 步之后，CI 构建时才按发布分支
     克隆 vendor。用 CI 部署步骤打印的后端版本与 `/api/v1/health` 确认是新构建：应为 `0.1.43+g7fdc4c15c…`（工作流里还留着按
     09-28 两个旧哈希跳过编译、复用缓存二进制的逻辑，`BUILD.txt` 的 commit 不足以证明二进制是新的）。
     版本串只含 gen-model 的提交（`build.rs`：`<版本>+g<12 位提交>.<构建时间>`），vendor 另看 CI「Checkout vendor path dependencies」一步打印的短哈希：应为 `e3d-model -> 230d20b`、`e3d-io -> 3bed2ba`，e3d-attlib / plant-mbd / old-aios-core 以及「Checkout plant-ui web assets」打印的 plant-ui 与 0.1.42 那次（run 39726501255）相同。第 3 步推完之后不要对旧的 Linux binary run 点 Re-run（见 §7）。
  6. 服务端 `model/rebuild` 整库重生成，抽查不只 ams7997（#58 遗留第 2 条）：土建、MDS、暖通、桥架各抽一组，覆盖
     09-22 以来的 vendor 改动；在 #58 收尾；顺带在服务器上复核 #59 的启动续跑。#58 收尾前先列出重生成回执里的 `failed_root_count` / `failed_roots` 与 ISSUE-029「已放弃」的根（0.1.43 起同一根同一错误连败 5 次会判死并撤掉旧模型），非零先归因。服务器地址由你在会话里给，不写进文档。
  7. 推送之后再跑 `submit.ps1`：草稿引用了未推送提交的链接，先提交会是 404。（2026-10-24 用户改主意：4 个新 issue 已先建——e3d-model #7 / #8、
   gen-model #62 / #63，提交链接等推送后生效；剩 5 条评论仍按本条等推送后跑，见 §9 W7t。）

  2026-10-22 W7a / W7d / W7i / W7e / W7k 之后：本地 e3d-model `main` 又多了 16 个（W7a `5ee3cb8` / `09949be` / `704c632` /
  `bae472b`，W7d `1f02613` / `8102052` / `fa72312`，W7i `d5e343d` / `69dea3f` / `d8d4af7`，W7e `845f4e5`，W7k `3a5dadb` / `efa8573` / `8974275` / `28d0402` / `3e1b1e2`）、
  e3d-io `main` 在 `3bed2ba` 之上多了 `ca949ef`、`9440bae`（W7k）、gen-model
  `refactor` 多了 `9216e2f72`、`c6cd27a85`、`635bf7a90`、`59349d66b`（读取集合同步）、`749681d8f`（SUPC 测试同步）与
  `13ffb716d`（结构连接窗的期望），都不在
  已验证的 0.1.43 里。第 2–5 步一律按提交号推，不会带上它们（照分支名推才会）。要原样交付
  0.1.43，按提交号推：e3d-model `git push github 230d20b:main`、`git push github 230d20b:ci/gen-model-linux-20260918`；
  e3d-io 照旧按 `3bed2ba` 推；gen-model `git push origin 7fdc4c15c:refactor`、`git push origin 7fdc4c15c:publish`。W7a 起的这些随下一版。

### W7 APS 剩余与覆盖缺口（2026-10-22 新增，ISSUE_13）

V7–V9 已并进 `output/run_gates.ps1`（2026-10-22）。W7a 要把 `transform::handrail_post_world` 里的特例换成一条规则，最容易把
aps250110 已对上的件打坏，每一步都要过 V9。

| 子项 | 内容 | 所在库 | 规模 |
|---|---|---|---|
| W7a | 详图式扶手：端头 `HRTERM` / 立柱 `HRPOST` / 扶杆 `RAIL` / `HANDRA` 的站位、朝向与目录几何，单开一轮 | aps250112、aps250110 | 已修（e3d-model `5ee3cb8`、`09949be`、`704c632`、`bae472b`）：off 108 → 0、只在 E3D 95 → 0；余 10 根带 `FRAD` 圆角的栏杆 E3D 逐段写，记为种类不同（几何 exact / chord ≤ 0.15，不改，见 §9） |
| W7b | 设备 `GENPRI` 不出：几何是 `GENPRI → ARCHIV → ATTDAT.ELDATA` 里的 SMLib B-rep（`IwBrepData`，B 样条），E3D 用 SMLib `IwTess` 三角化。取证见 `verification/civil-tty-20261021/genpri-eldata-notes.md` | aps250162 | 已修（e3d-model `1cc594a`、`8f37bc5`），75 只全出：70 exact、5 chord（GENPRI 13） |
| W7c | 围护板 `BPANEL`、楼梯踏步 `TREAD` 不出；单 `DESP` 的 WALL 按 SPRE 目录截面求值 | aps250110 | 已修（e3d-model `0184cb6`、`f21e7a1`、`faf975e`、`230d20b`），52 / 25 / 2 全出、全 exact |
| W7d | `TRUNNI` 名下隐式管多出、本体多一条 Cylinder；泵出口隐式管多出 | aps250104、aps250110 | 已修 / 归因（e3d-model `1f02613`、`8102052`、`fa72312`；gen-model `749681d8f`）：TRUNNI 16 根与泵房 4 条 BRAN 的管身 E3D 写在容器那一组里，对拍工具配上后全 exact；SUPC 多补的 7 根去掉；锚固件 23 只不再另写零径 LCYL。只在我们 33 → 0、种类不同 23 → 0（见 §9） |
| W7e | 阀门少写的 `Line` 记录（几何 exact）：目录 `LINE`（`TUFL` 为真）E3D 写 `Line[5, 长]` | aps250132、aps250160 | 已修（e3d-model `845f4e5`）：26 只阀门的阀杆线补齐，种类不同 26 → 0；记录标架按线段的世界方向定（见 §9） |
| W7f | GENSEC（弯的型钢）两端各比 E3D 短 0.54 mm | aps250110 | 归因、不改（见 §9）：不是弯梁，是 1:620 放坡的直梁；两侧端面一致，差在退刀底面（水平）与斜梁底面之间一片 ≤ 0.016 mm 厚的楔形薄片，它的尖端位置把 0.001 mm 级的高差放大成 0.5 mm |
| W7g | `rvm_read` 认不出 aps250128 导出里 6 条记录的声明盒，整库拒读 | aps250128 | 已修（e3d-model `e59dfb7`），190 对全 exact |
| W7h | aps7200 整库（副本，`run_copy_tty.ps1`）；aps250209 / aps250212 不在 `/ALL`，同样走副本或记账 | aps7200 等 | aps7200 已对拍（项目副本，见 §9）：2294 对、2261 exact、chord 3、off 30、只在 E3D 58、种类不同 27，归堆进 W7k；aps250209 / aps250212 已对拍（2026-10-24，副本的 SYS 库用 e3d-io 写路径登记两库后导出，见 §9 W7t）：aps250209 4583 对 4564 exact、off 19、只在 E3D 40、种类不同 2；aps250212 0 对、只在 E3D 6（地形网格 GENPRI），归堆进 W7t |
| W7k | aps7200 新露出的：结构节点 SJOINT 不出（34）、桥架路径点 POINTR 不出（20）、矩形风管弯头少写一条 `RectangularTorus`（off 125 mm，11）与矩形风管其余 3 件、吊架 PCLA / PCLAMP（off 6、chord 3、只在 E3D 3）、踢脚板 FIXING 与一堵 WALL 大偏（5，0.5–1.9 m）、其余 off 5、只差记录条数 9 | aps7200 | 矩形风管弯头已修（e3d-io `ca949ef`，e3d-model `3a5dadb` 真库门）：命名角轴的角度字是 `DECARG` 操作数，11 只弯头与跟在后面的 1 只 REDU 对上，exact 2261 → 2273、off 30 → 18、种类不同 27 → 16；余 off 18、只在 E3D 58，各堆根因与修复方案已查明（见 §9 与 ISSUE_13 草稿第 9 类）；第二批（e3d-io `9440bae`，e3d-model `efa8573`、`8974275`）修了吊架、踢脚板、板上套管：2297 对、2286 exact、off 8、只在 E3D 55、种类不同 9；第三批（e3d-model `28d0402`）修了 SJOINT 与两根梁端：2331 对、2322 exact、off 6、只在 E3D 21、种类不同 9；SNODE 点标记（e3d-model `3e1b1e2`）：191 只节点的三条线照 E3D 写进 RVM、逐只对上（对拍工具不计线）；弧墙（e3d-model `445d634`）：挂着关节的那一端不按 SPINE `DRNS` / `DRNE` 斜切，`WALL 1 /C1101-BASE` off 1742 mm → exact：2331 对、2323 exact、off 5、只在 E3D 21、种类不同 9；圆转方风管变径（e3d-io `8a342cb`，e3d-model `80123a0` 真库门）：目录 PML 的 `ATANT` 按 atan2(纵, 横) 求值、不再拒答，`TRNS /HVAC2-B1/ITEM8` off 375 → exact：2331 对、2324 exact、off 4、只在 E3D 21、种类不同 9；零尺寸 CYLI（e3d-model `b758b28`）：设计 CYLI 尺寸为 0 时照 E3D 写 `Cylinder[0, 0]` 空记录（只进 RVM），只在 E3D 21 → 20：2332 对、2325 exact、off 4、种类不同 9；PIPE 下的矩形 TUBI（e3d-model `22c5e71`）：管道学科先按风管路解断面、矩形就用，`BRAN /HVAC/RECT/M1/B2` 管身 off 49.5 → exact：2332 对、2326 exact、off 3、种类不同 8；桥架路径点记号（e3d-model `fb72d65`）：CWBRAN 的 RPATH 每个 POINTR 照 E3D 写 10 mm 世界轴 Box（只进 RVM），只在 E3D 20 → 0：2352 对、2346 exact、chord 3、off 3（圆弧切分，ISSUE_09）、种类不同 8。**W7k 收口** |
| W7i | 容器组配上后露出的管身条数不同（W7d 带出）：aps250142 `/SUPPLY_WORKSHOP-001` 多一根 25 mm、`/RETURN_LEVEL02-002/01` 少 1 根、`/SUPPLY_LEVEL02-001/02` 少 4 根；aps250124 `/100-LAYOUT300-307/B1` 少 2 根；ams8000 `/M-OR-1RX-3081D` 多 2 根 | aps250142、aps250124、ams8000 | 修 2 条（e3d-model `69dea3f` 放坡 TEE 按 `SLODIR` 判共轴、`d8d4af7` 风管分流片 SPLR 穿过；gen-model `c6cd27a85`）：off 5 → 3、种类不同 5 → 3；余 3 条转 W7j（见 §9） |
| W7j | W7i 剩下的 3 条，各要一块新能力：`/RETURN_LEVEL02-002/01` 头管的管径是设计侧 UDA（`/HCTUBE` 的 `PDIA = ( :0x17EF4B61[1] )`）；`/SUPPLY_LEVEL02-001/02` 扁圆风管 `/FOTUBE2` 的管身是 BOXI + 两只 TUBE（E3D 一条 Box、四条 Cylinder，我们按圆管写一条）；ams8000 `/M-OR-1RX-3081D` 托架支撑 ANCI 的 DATUM / SREF 成对、正反向槽位的规则没定 | aps250142、ams8000 | 3 条支管（off 3）；头管 UDA 已修（e3d-model `w7j-uda` 上 `5e1649e`、`e11d359`，2026-10-24 并回 main `e9d6cc5`）；扁圆风管已修（e3d-model `b899445`：隐式管身按管身目录件的 GMSE 逐图元建，aps250142 521 对全 exact、off 0、种类不同 0）；ams8000 `/M-OR-1RX-3081D` 托架已修（e3d-model `d0fe6d1`：不是成对 / 正反向的形态规则，是 ATTA 自己的 `SPKBRK`，ams8000 2614 对全 exact）。**W7j 收口** |
| W7t | aps250209 / aps250212 露出的：梯段上的布局式 HANDRA 顶端比 E3D 长 9.07 mm（off 16）、带圆弧踢脚板的布局式 HANDRA 整只不出（只在 E3D 22，生成报 `layout kickplate does not support arc segments`）、梯段 STRFLT 自身那条面片不出（只在 E3D 18）、设备模板里带 3 个负体的 CTORUS 布尔结果不同（off 409.6，2）、整圈 REVO 拆两条写（种类不同 2 + off 1.18，ISSUE_09 既有类）；aps250212 地形模型 TRNMOD → TRNCC → TRNCEL 下的 6 片 GENPRI 不出（TRNDEF 下的 3 个定义图元 E3D 的 ZONE 导出里没有，不可比） | aps250209、aps250212 | 2026-10-24 取证完，当天五堆全修（见 §9 W7t 修复）：e3d-model `df70971` 扶手两端按 RPATH 端向切 + 踢脚板在圆角拐角按拐点斜接（off 16 → 0、只在 E3D 22 → 0）、`eb7e633` 布局式 STRFLT 自己的斜梁 / 踏步 / 平台板（只在 E3D 18 → 0）、`87c0acb` 进了布尔的正 CTOR / RTOR 按 10 mm 粗格建（off 2 → 0，顺带 aps7200 两只 RTOR off 0.87 → exact）、`0b7b1b5` 地形 GENPRI 的归档版本 4 缓存面片照 E3D 写成面片（aps250212 只在 E3D 6 → 6 对全 exact）。收口：aps250209 4623 对 4622 exact、off 1（整圈 REVO，ISSUE_09 既有类）、只在 E3D 0、种类不同 2（同一类）；aps250212 6 / 6；aps7200 2352 对 2348 exact、off 1 |

2026-10-25（用户「继续」）：对拍表里剩下的 ISSUE_09 类也开始修——扫角 > 180° 的回转图元拆两段写（ams8000 28 + aps250209 2 + aps7200 2 + aps250110 10 件种类不同、
  aps250209 最后 1 件 off 归零）、多面体逐连通体写（aps7200 POHE 2）、变截面型材放样（aps7200 TAPER-SCTN 4，几何本身也错着），见 §9「M8 · ISSUE_09 记录写法」。

建议顺序：W7g（解锁一整库的覆盖，已完成）→ W7b（已完成）→ W7c（已完成）→ W7a（已完成）→ W7d（已完成）→ W7i（已完成）→ W7e（已完成）→ W7f（归因）→ W7j / W7k（2026-10-22 用户「继续修复」，按影响面 × 好修程度：矩形风管（已修）→ SJOINT → 踢脚板 / WALL 大偏 → 吊架 → 头管 UDA（已修、已并回）→ 桥架路径点 → 扁圆风管（已修）/ ams8000 托架（已修）/ 只差条数的（ISSUE_09 既有类，不改））；W7h 已对完 aps7200（W7k 是它带出的）与 aps250209 / aps250212（W7t 是它们带出的，按顺序梯段扶手顶端 9 mm → 圆弧踢脚板 → STRFLT 面片 → CTORUS 负体布尔 → 地形 GENPRI，2026-10-24 全部完成）。每项先取证 E3D 的写法
再动代码。验收：ISSUE_13 表里的 off、只在 E3D、只在我们归零或逐件归因；V1–V9 不变差。

### 不在本计划执行的（待解决 9、10，按 D9）

- #34：只补一条验收——改 MDS 关节 FIXING（JOIN 的 NGMR 截杆，`4d5d949`）后，被截的 GENSEC 所在根进 regen。评论草稿
  `output/github-issues-20261021/14-gen-model-34-comment.md`（2026-10-22 补，D9 的这份交付物此前漏了）。
- #59：由 #59 自己跟进。逐根容错、强制重生成先落留痕、进程中断后启动续跑已在 gen-model 本地 `aa6ccb44e`（未推送）；
  剩 `0xc0000409` 崩溃根因。

## 5. 执行顺序与里程碑（D1 取 (a) 时）

| 里程碑 | 内容 | 产出 |
|---|---|---|
| M1 | W1a、W1b；W3 的 #6 复核与 #1 拆分 | e3d-model 本地提交；ISSUE_05 更新或关闭；#6 / #1 评论草稿 |
| M2 | W3 的 #5；W2；W1c | e3d-model 本地提交；C-IY 两种口径都 exact；#6 三个区种类一致 |
| M3 | W4.1 MDS 其余三套框 | 对拍报告；新问题的 issue 草稿与修复 |
| M4 | W4.2 ams8000 整库 | 同上 |
| M5 | W4.3 APS、W4.4 ams2510xx | 同上 |
| M6 | W5 VV p-line（已完成） | datum 真值 3678/3678 |
| M7 | W6 交付与 FN09 | 部署环境抽查记录；#58 收尾；FN09 重导对拍 |
| M8 | W7 APS 剩余与覆盖缺口（V7–V9 已并入 `run_gates.ps1`） | 对拍报告；ISSUE_13 更新；新问题的 issue 草稿 |

每个里程碑结束都回一次报告，并更新汇总贴草稿。

## 6. 回归门（每一步合入前）

| ID | 断言 | 命令 |
|---|---|---|
| V1 | 土建 9982 件判档与种类不变差 | `python output/civil_regen.py <标签> --sites core,ssc,ptu --prev-tag <上一轮>` |
| V2 | 10-11 基准与 MDS 整份不变差 | `python output/s1_regress.py <标签> --prev-tag <上一轮>` |
| V3 | MDS 80 个 ZONE 不变差 | `python output/mds_compare.py <标签> <gen-ams7324>/generated.rvm "mds_bs_lv6_*.rvm"` |
| V4 | `/SSC测试` 整站不变差 | `rvm_compare --baseline verification/civil-tty-20261021/ams7997_ssc_lv6.rvm --generated <gen-ams7997>/generated.rvm` |
| V5 | FIXING / datum 真值不变差（判据：逐行位姿、包围盒、三角形数不变，体积相对差 < 1e-12 视为不变；超过就是变差。w7l2 那 45 只 MDS FIXING 的 1e-15 级体积变化按这条放行，来源要写进执行记录） | `zz_tmp_fixing_truth_check` + `verification/fixing-tty-20261021/analyze.py` |
| V6 | e3d-model 全量测试 | `cargo test --release`（0 failed；w7c 476 通过，含工作区未提交的 `element_diff.rs` 与 `relocation_noise_real.rs`；干净检出 472 通过，见 W6） |
| V7 | MDS AISC / DIN / SPECIALS 不变差（1409 / 1469 / 959 对） | `powershell -File output/mds_xfr_regress.ps1 -Tag <标签>` |
| V8 | ams8000 整库不变差（以 `-Prev` 那轮为准；w7d 改了容器组配对后是 2614 对、1 off（W7j）、记录种类不同 29，此前 2562 对全 exact、种类不同 28；w7u2 起 2614 对全 exact、种类不同 28） | V2 之后 `python output/mds_compare.py <标签> verification/regress-20261021/gen-ams8000-<标签>/generated.rvm "ams8000_lv6_*.rvm"` |
| V9 | APS 不变差（以 `-Prev` 那轮为准；w7d 起含容器组配对，与 w7c 时的 aps250110 3427 对、其余各库 4870 对不能直接比） | `powershell -File output/aps_regen.ps1 -Database aps250110 -Tag <标签>`；`powershell -File output/aps_survey_compare.ps1 -Tag <标签> -Generate` |

任一变差就停：保留现场（命令、输出、标签），不放宽判据，回到对应工作包重议。

对拍工具自身改动（如 W7d 的 `fa72312` 容器组配对）之后新配上的件不算变差，但要逐件归因、写进执行记录，下一轮的 `-Prev` 以工具改动后的那一轮为准。

V7–V9 是 2026-10-22 审核补的（M3–M6 里一直是手动复核，见 `verification/regress-20261021/extra-*.txt`、`ams8000_lv6-*.txt`），
已并进 `output/run_gates.ps1`：每项打印本轮的 TOTAL 行，与 `-Prev` 一致时标 `same as`，不一致时把上一轮的行一起打出并标
`CHANGED`；V8 在 V2 被跳过时自己用 `s1_regress.py --runs ams8000` 生成整库。

## 7. 风险与不做事项

- W1b、W2 都建立在「E3D 按什么切、按什么写」的假设上；先普查、再动代码，普查对不上就回到决策点。
- E3D TTY 全程只读（没有 SAVEWORK）；要挂库的只在项目副本里做。
- 本地验证与部署代码不一致：vendor 工作区有未提交改动、vendor 发布分支停在 09-22（W6、D10、D11）。交付前按 W6
  处理，否则线上结果与回归门结论对不上。2026-10-22 已用干净检出验证（§9 M7）；发布分支待推送后快进。
- 部署工作流仍会对两个旧哈希复用缓存二进制：部署后以后端版本串（应为 `0.1.43+g7fdc4c15c…`）为准，不以 `BUILD.txt` 为准。
- e3d-io 已入库的 `tests/spamap_recompute_real.rs` 用到 e3d-attlib 未入库的 `dict.rs`（`AttrDict`）：干净检出里 e3d-io 自己的
  测试编不过。不影响部署（CI 只编服务端二进制），要 e3d-attlib 那条工作线把 `dict.rs` 提交上去。
- vendor 发布分支是活的：CI 按分支名 `--depth 1` 现克隆 vendor，版本串只含 gen-model 的提交。第 3 步推完之后，对旧的 Linux binary run 点 Re-run 会拿旧 gen-model 配新 vendor 构建并部署，版本串照旧，看不出混版本；第 3–5 步连着做，部署核验看 CI 打印的 vendor 短哈希（W6 第 5 步）。
- 0.1.43 起 ISSUE-029 会把同一根同一错误连败 5 次判死并撤掉旧模型；第 6 步整库重生成后先看失败根与「已放弃」的根（W6 第 6 步）。
- W7d 起容器组按并集包围盒判档，会掩盖单根管的位置错（落在并集盒内就看不出来），见 D16。
- 推 vendor 的 `main` 会改变 gen-model `windows-tests.yml` 的输入（它对 e3d-io / e3d-attlib / e3d-model / plant-mbd 取默认分支，所有 PR 都触发），其他仍基于旧 vendor 的 PR 可能变红，属预期。
- 不做：改 E3D 数据（项目副本的 SYS 库除外，W7t 经用户指示登记了两库）、未经同意的推送、gh 写操作（issue 草稿原由你跑 `submit.ps1`；2026-10-24 经你指示由我跑了 4 个 issue 的创建，评论仍等推送）、#34 / #59 的实现。
- 两处 vendor 未提交改动（old-aios-core `pdms_shape.rs` 的校验式网格反序列化、e3d-model `element_diff.rs` 的匿名属性 diff）是 0.1.43 已提交代码所依赖的行为：干净组合上 gen-model 5 条 lib 测试红、坏 `.mesh` 不会被修复、匿名属性改动不进增量判定（D14 补验，2026-10-23）。不按 D17 (a) 处理就推，线上就是这个组合。
- **回归门的输入库在被别的线写**（2026-10-23 查明，§9 W7k「圆转方风管变径」里那件「偏发」）：`ams000\ams8000_0001` 当天从会话 651
  被写到 659（各轮门的 gen_ams 日志钉到的会话：w7n / w7o 651、w7p 653、w7p2 起 659；整个 ams000 目录的库文件 13:12:50 被批量改写），
  会话 652–653 把 `FTUB 4 of BRAN /C-OR-1R345-C`（`24384/23262`）挪高了 500 mm、654 挪回（`gen_ams --sesno 651/652/653/654/659` 逐个会话核过），
  所以 12:52 的 `w7p` V2 / V8 各多 1 件 off 500——**不是生成器偏发**（同一二进制在 659 上重跑 91 次全正常）。库里还多出了测试用的
  `BRAN /CODEX_DB8000_*`、`/test`（含这根 FTUB 的副本）。这跟「E3D TTY 全程只读」的前提冲突：写路径测试应改在库副本上做；回归门里
  单件的 off 若与改动无关，先看 gen_ams 日志里「钉在会话」的号有没有变；要不要给门脚本加 `--sesno` 钉会话（TTY 导出时的会话号没记，
  ams8000 按 651 钉是安全的）待你定。
- 回滚（代码）：每个工作包独立提交，`git revert` 即回。
- 回滚（部署，D15）：两个 vendor 发布分支强推回 `f2ecfe9` / `5245654`，再推一个 `ci/linux-build-*` 分支到 `220a6ec87` 触发构建（`workflow_dispatch` 只认默认分支上的工作流，这里用不了；这样也不用强推 publish）；第 6 步重生成过的模型要再整库重生成一次。D18 之后回滚还要把 old-aios-core 的发布分支强推回 `a438029`。
- gen-model `publish` 不止本计划在推（2026-10-23 补交付时发现）：净距那条线 10-22 23:13 把 `refactor` / `publish` 推到 `d3145d26a` 并部署，当时 vendor 发布分支还没推，线上成了混版本；10-23 16:25 它又推了 `refactor`（`384e67151`）。vendor 发布分支推进之后，任何一次 `publish` 或 `ci/linux-build-*` 推送都按新 vendor 构建：下一版之前推 `publish`，先核 gen-model 里的读取集合同步提交与 vendor 发布分支是同一档（W7 线的同步提交只对与之对应的 vendor 成立，`refactor` 已同步到 W7k 的 `28d0402`），并和本计划对齐。
- e3d-model 发布提交 `df2aa7d` 不在 main 上（element_diff 在 main 上是另一个提交 `65210cc`）：下一版把发布分支推进到新 main 就不是快进，届时先把 `df2aa7d` 合进 main（内容相同，`-s ours` 即可）再推，或按 D15 强推。

## 8. 批准栏

- [x] 批准：按 §4–§6 执行（决策点未批注的按推荐项）——Plannotator `approved`，2026-10-21
- [x] 第二次送审（2026-10-22）：Plannotator `approved`，2026-10-22 17:28，无批注。送审稿（Plannotator 历史 `002.md`，16:19 存档）含 D10–D13、M7 交付准备、W7a–W7h；第二次审核新增的 D14–D16、之后的 D17 / D18 与 W7i / W7j / W7k 都是用户在会话里拍板的，没有再送 Plannotator
- [ ] 驳回 / 改口径：无

## 9. 执行记录

### M1（2026-10-22 完成）

e3d-model 本地提交 3 个（未推送，现领先 21 个）：

| 工作包 | 提交 | 结果 |
|---|---|---|
| W1a STWALL → `Pyramid` | `9d08daa` | 截面成员只有一个也交出身，并带元素局部系标架。两只 STWALL 的种类、七个参数、记录矩阵都与 E3D 相同（只交出身不给标架时轴会挑错） |
| W1b 风管 THRE / BEND | `729cb50` | 「只合被切的图元」的假设不成立：这两只没有负体，多出来的 `Pyramid` 是高为 0 的目录 LPYR 板。按无体记录写，参数逐位相同（含 `-0`）；D2 不再需要 |
| W3 #6 复核 | `390a6d6` | 斜切大小头倒着写的声明盒也能判帧；三个区 96 / 34 / 82 对全部 exact。关单评论草稿 `11-e3d-model-6-comment.md` |
| W3 #1 拆分 | — | 剩余项拆到 ISSUE_09（另加记录级普查查到的 6 只 `Pyramid` 等价写法），关单评论草稿 `10-e3d-model-1-comment.md` |

回归门（标签 `m1` / `m1b`，对 `kinds2`）：V1 土建 9982 件判档不变、记录种类不同 0；V2 S1 基准与 MDS 整份 0 对变化；
 V3 MDS 80 个 ZONE 1473 exact / 4 chord / 0 off、种类不同 0；V4 `/SSC测试` 4522 exact / 1 film、种类不同 4 → 0；
V5 FIXING 255 + 2150 行、datum 3678 行逐行不变；V6 e3d-model 全量测试 448 通过、0 失败。

新发现：#6 三个区的记录种类不同 62（W1c，排进 M2）。汇总贴草稿更新为「已修复 26 项，待解决 11 项」。

### M2（2026-10-22 完成）

| 工作包 | 提交 | 结果 |
|---|---|---|
| W3 #5 外项目库 | e3d-model `7d9c943`（+ `f1d4096` / `5c1e758` 读取集合）、gen-model `430451b33` | 读主项目 SYS 库 `DB.PROJ` 的外项目表，只补登记、给定目录解析不出的库号。只给 ams000 + acp000：两只 ATTA 与给 ZDJ000 时逐字节相同、`/1PTU-INST23` 20/20。顺带补回 S1 整库生成里被 ZDJ 同号库挤掉的 ACP 7009 / 7011 / 7015。系统库同号不再因补库而新增 |
| W2 C-IY 槽弯 | `ed3aa13` | TTY 重导（当前库，holes on / off 各一份）显示 E3D 照画拼接板，只有 addin 基线按 OUTA / OUTB 藏了；这两个标志只在 `E3D_EXPORT_HOLES=off` 下算数。C-IY 对 TTY holes on 35/35（此前 23/35），对 addin 基线仍 35/35。「让 holes 管负体」那条假设不成立 |
| W1c 暖通记录种类 | `3f9f3c7` | SSLC 斜切节按目录参数直出带斜切角的 `Snout`；零径、有高的 LSNO 记无体 `Snout`。#6 三区种类不同 62 → 0，`Snout` 记录逐条与 E3D 相同 |

回归门（标签 `m2a` / `m2b` / `m2c`，逐步对上一步）：V1–V4 全部 0 对变化、记录种类不同 0；V5 FIXING / datum 逐行不变；
V6 全量测试 453 通过、0 失败。#5 提交前漏跑了 V6，读取集合护栏红了两条，已由 `f1d4096` / `5c1e758` / gen-model `430451b33` 补上。

### M3（2026-10-22 完成，无代码改动）

MDS 其余三套框：`/SUPPORT/FRAMES/AISC/DATA/FT`（`23713/18125`，72 个 ZONE）、`/SUPPORT/FRAMES/DIN/DATA/FT`
（`23734/17969`，80 个 ZONE）、SPECIALS（ams7330 没有 `DATA/FT`，导 `/MDS/SPECIALS` 与 `/SUPPORT/FRAMES/SPECIALS/BS`
两个根）一次 TTY 只读会话逐根导出 154 份（`mds_xfr_lv6_*.rvm`，用 shadow_e3d31_gen_model_test 运行时）。整库生成
对拍：AISC 1409 对 1405 exact / 4 chord，DIN 1469 对 1465 exact / 4 chord，SPECIALS 959 对全 exact；0 off、种类全部
一致、生成 0 失败。chord 是与 BS 同一种框的 FT44 11-V5–V8（0.11 mm）。没有新问题。

### M4（2026-10-22 完成）

ams8000 的 3 个 SITE、10 个 ZONE 一次 TTY 只读会话逐个导出（`ams8000_lv6_*.rvm`，宏 `ams8000_lv6.mac`），整库生成
对拍。首轮（`m4`）2528 对 2506 exact、off 22、只在 E3D 34、记录种类不同 14，都在生成器一侧：

| 问题 | 提交 | 结果 |
|---|---|---|
| `/1CSV-HVACHB` 暖通支架 22 只 FITT 端板绕梁轴差 90° | e3d-model `1e3864a` | 竖杆角钢的 `SPRE` 在 db 5052 里已不存在，E3D 自己也没有截面（梁只画 `Line`），径向落在截面 x。`section_fitting_placement` 区分「`SPRE` 在开着的库里查无此号」（径向 0）与「库没注册」（照旧 π/2，`/-KX-CAM-S4001` 那档）。off 22 → 0 |
| 设备名下 34 只 `ELCONN` 不出 | e3d-model `b590b28` | `enters_catalogue_by_catref` 加 `ELCONN`（`CATR → TABITE → PRTREF → GPART → SCOM`），34 对全 exact |
| 竖梯 FTUB 的圆板 SEXT 认成 `Cylinder`、接线盒矩形 SEXT 认成 `Pyramid` | e3d-model `e3b1928` | 目录 SEXT 只走出身（E3D `DRWEXT` 只写面片），记录种类不同 45 → 28 |

修后 2562 对全 exact。剩 28 只接线盒的记录条数差一：GMSE 里一块 `PANG = −360` 的 SREV，E3D 拆成两条各 180° 的面片
写，我们写一条（两半合起来与我们那条包围盒相同）。拆半与 C-IY 那 3 条 SREV 的负回转相位是同一笔账，并进
ISSUE_09，低优先级。

另：gen-model `21721c4d0` 补上 `430451b33` 漏改的读取集合条数钉（151 → 153），`model_impact::tests` 50 条过。

回归门（`output/run_gates.ps1`，标签 `m4b` 对 `m2c`、`m4d` 对 `m4b`）：V1 土建 9982 件判档与种类不变；V2 S1 基准与
MDS 整份 0 对变化、种类不同 0；V3 MDS 80 个 ZONE、V4 `/SSC测试` 总数不变；V5 FIXING 255 + 2150 行、datum 3678 行逐行
不变；V6 全量测试 454 → 456 通过、0 失败。汇总贴草稿更新为「已修复 32 项，待解决 8 项」。

### M5（2026-10-22，APS 完成；186 只 FIXING 按 D6 (b) 收口）

W4.3 APS：aps250110（1 个 SITE、5 个 ZONE）整库 TTY 导出对拍（`aps250110_lv6_*.rvm`）。首轮 3348 对 3291 exact、off 56、
种类不同 40、只在 E3D 81：

| 问题 | 提交 | 结果 |
|---|---|---|
| 门框模板里 45° 斜接的 FBAR：单 SREC 截面自 `9d08daa` 起写 `Pyramid`，端头斜切就不是棱台（E3D 写面片） | e3d-model `4fbd7fe` | `SectionCutPlane::is_square`，两端不都是平头时 SREC 不交出身；种类 20 → 0 |
| 扶手立柱模板里 `RADI = 25` 的设计 DISH（半球）写成 `SphericalDish` | e3d-model `4fbd7fe` | `RADI ≠ 0` 记 `knuckled_dish`，按它挑 `EllipticalDish`；种类 10 → 0 |
| 扶手面板 `HRPANE` 没有 POS / ORI，模板里 55 件偏 3.3 m | e3d-model `07c3d2e` | 与立柱同一套站位 + `(0, −XOFF, ZOFF)`；off 55 → 0 |
| 墙板上 `BANG = 90` 的 PFIT 长短边对调 | e3d-model `07c3d2e` | 目录实例右乘 `Rz(BANG)`；off 1 → 0 |

修后 aps250110 3348 对 3346 exact、1 chord、1 off。其余设计库一次 TTY 会话普查（`aps_survey_lv6_*.rvm`，36 个 ZONE），
全部 10 本合计 7953 对 7816 exact、28 chord、109 off、只在 E3D 249、种类不同 75；`aps2505xx` 与 `aps2501xx` 逐库同内容，
`aps250209` / `aps250212` 不在 `/ALL`，aps250128 的导出读取器拒读（6 条记录认不出帧）。剩下的（详图式扶手、耳轴支架、
设备 `GENPRI`、`BPANEL` / `TREAD`、单 `DESP` 的 WALL、阀门 `Line`）归堆写进新草稿 ISSUE_13。

W4.4 的 186 只 FIXING：在 `ams251008` / `ams251010` / `ams251179`–`ams251183` 七个库文件里，这七个库不在 AMS 的 SYS 库（也不在
本机其它项目的 SYS 库）里，E3D 打不开。D6「在副本里挂进 `/ALL`」的前提不成立，要先在副本里用 ADMIN 登记；等决定。
2026-10-22 定：D6 改判 (b)，按「不属于任何项目的文件」记账，FIXING 验收 2405 / 2405。

回归门（标签 `m5e` 对 `m4d`）：V1–V5 全部不变、记录种类不同 0；MDS AISC / DIN / SPECIALS 与 ams8000 整库复核不变；
V6 全量测试 458 通过、0 失败。汇总贴草稿更新为「已修复 36 项，待解决 8 项」。

### M6（2026-10-22 完成）

| 工作包 | 提交 | 结果 |
|---|---|---|
| W5 圆管圆周 p-line | e3d-model `5a9582e` | `/GTYPE_CTUB_PTSS` 只有 0° / 90° / 180° / 270° 四条写了 PX / PY，其余 20 条两个程序都空（VV 315°、PP 225° 等），E3D 放在管外圆上。`section::pline_offset` 在两个程序都空时取截面里最大 SANN 沿 PLAX 的半径点，没有 SANN 的截面照旧按 0；`datum::solve` 的 POSL 与 `datum::section_pline` 都走它。真库门 `gensec_end_datum_real::circumference_pline_datums_sit_on_the_tube_wall` |

回归门（标签 `m6a` 对 `m5e`）：V1–V4 0 对变化；V5 FIXING mdb_all 255 行不变、rest 2150 行里 90 行变化（就是 FT44 名下那 90 只
FIXING，变好）、datum 3678 行里 90 行变化（3588 → 3678 / 3678 一致）；MDS 其余三套框、ams8000 整库、APS 10 本复核不变
（`extra-m6a.txt`、`ams8000_lv6-m6a.txt`）；V6 全量测试 459 通过、0 失败。汇总贴草稿更新为「已修复 37 项」。

偏离计划：W5 第 1 步（反查 core.dll）与第 3 步里「e3d-io 区分空表达式」都没做，走的是草稿 02 写明的兜底——只对圆截面
落地；其他截面上的空程序线照旧按 0（现有语料没碰到、也没有真值），记在汇总贴「已知、不改的」。

### 审核（2026-10-22）

按用户要求对本计划与执行记录做了一次只读核对（代码、提交、部署链路、issue 草稿），用户回复「全按推荐」。发现与处理：

| 发现 | 处理 |
|---|---|
| 交付链路：服务器按 `ci/gen-model-linux-20260918` 克隆 vendor，0.1.41 部署实际用 e3d-model `f2ecfe9`、e3d-io `5245654`（09-22），只推 main 不会上线 | W6 交付步骤补全；新增 D10 |
| gen-model refactor 本地领先 15 个（汇总贴写的是 2 个）；`Cargo.lock` 的 e3d-io 0.5.0 未提交 | W6 第 2 步；汇总贴第 6 项改正 |
| vendor 工作区有未提交改动，V6 的 459 通过带着 e3d-model 的 `element_diff.rs` | W6 第 1 步；新增 D11 |
| 汇总贴 #59 描述过期（本地 `aa6ccb44e` 已做留痕与启动续跑） | 汇总贴第 4 项改正 |
| D9 的交付物（#34 验收）没有草稿 | 补 `14-gen-model-34-comment.md`，并入 `issues.json` |
| aps7200 整库漏做（W4.3 写了，M5 和 ISSUE_13 都没提） | 进 W7h；ISSUE_13 补一笔 |
| 新对上的 MDS 三套框、ams8000、APS 不在回归门里 | §6 补 V7–V9，已并进 `run_gates.ps1`（见下节） |
| APS 剩余问题没有工作包 | 新增 W7、M8 |
| FN09 修正清单未出；汇总贴写的「两个倒圆都远大于邻边」不准（42387 那个放得下） | 清单已出；汇总贴第 5 项改正 |
| 186 只 FIXING | D6 改判 (b) |
| 计划抬头、§2、§9 与汇总贴不同步；草稿 02 已过时；#5 评论里「等 MDS 其余三套框再看」已有结论 | 本次同步 |

汇总贴草稿更新为「已修复 37 项，待解决 6 项」（186 只 FIXING 转入「已知、不改的」）。本次只改文档与草稿，没有动代码。

### 回归门扩到 V9（2026-10-22）

V7–V9 并进 `output/run_gates.ps1`。用标签 `v79a` 对 `m6a` 只跑 V7–V9（`-Skip V1,V2,V3,V4,V5,V6`，`gen_ams` 仍是 m6a 那轮的二进制）
验证：MDS AISC / DIN / SPECIALS、ams8000 整库（V2 跳过，由 V8 自己生成）、aps250110 与其余 10 本普查全部 `same as m6a`，全程约
70 秒；另用 m4 的 ams8000 结果核过 `CHANGED` 分支。

### M8 · W7g（2026-10-22 完成）

| 工作包 | 提交 | 结果 |
|---|---|---|
| W7g aps250128 整库拒读 | e3d-model `e59dfb7` | 6 条认不出帧的记录都是 `/100-FW-201` 等 6 只阀门的阀帽 `SphericalDish`（r 44.1 / h 80.6，深于半球）。真几何半宽 R = (r² + h²) / 2h = 52.365，E3D 的记录盒只到底圆半径 44.1，差 8.265 mm 顶穿判帧容差（8.056）。`declared_box_mm` 给 h > r 的 `SphericalDish` 补声明盒 `[−r, −r, 0] .. [r, r, h]`；只影响判帧，空间判定与写出端不变。单测 `deep_spherical_dish_frames_by_the_base_radius_box` |

取证：`output/rvm_dish_census.py` 普查盘上 546 份 E3D 3.1 导出的 107 条 `SphericalDish`，逐条等于这只盒（残差 0）；其中
24 条 h > r，除 aps250128 的 6 条外，aps250132 的 18 条（r 17.358 / h 26）此前靠容差混过。改后用 `rvm_read_audit`
过一遍 546 份导出：判帧 unknown 0。

回归门（标签 `w7g` 对 `m6a`，`gates-w7g.txt`）：V1–V5、V7、V8 全部 0 变化；V6 全量测试 460 通过、0 失败；V9 只有
aps250128 标出 `CHANGED`：0 对 → 190 对全 exact、记录种类一致，其余 APS 各库不变。APS 合计变为 8143 对 8006 exact。
汇总贴草稿更新为「已修复 38 项，待解决 6 项」。e3d-model 本地领先 33 个提交。

### M8 · W7b（2026-10-22 完成）

| 工作包 | 提交 | 结果 |
|---|---|---|
| W7b 设备 `GENPRI` 不出 | e3d-model `1cc594a`；gen-model `a34d58647`、`704f5d2f7` | 解码 `ARCHIV → ATTDAT.ELDATA` 里的 SMLib B-rep（外壳 + `IwBrepData` 数据版本 32，曲线 / 曲面全是 B 样条），新模块 `genpri` 求值并三角化：各面网格线、边采样两侧共用、UV 里约束 Delaunay（新依赖 `spade`）、修剪环自交加密 / 捏合。`gen_ams` 对 GENPRI 只写 `FacetGroup`；读取集合加 `ELDATA`（gen-model 侧同步 154）。单测 7 条 + 真库门 `genpri_real` |
| W7b 剖分照 `IwTess` | e3d-model `8f37bc5` | 面细分树（插节点截出的控制网厚度超弦高 / 尺寸向量超最大边长就分，中点吸附节点值）、等参边在叶子边界断开、UV 比叶子宽 1.1 倍的段中点断开、每段 `TessellateByBisection`；闭合边另在节点处断开。单测 +3，真库门局部包围盒加 GENPRI 10 |

取证：XBaseDll 3.1（读取链 `GeoIwBrepGen::ReadFrom → IwBrepData::ReadFromDB`；`GeoIwBrepImpl::Tessellate` 的取尺）与
E3D 2.10 的 `TbAg.dll`（SMLib `IwTess`：边用 `TessellateByBisection` 的 Hermite 控制多边形弦高，面用伪盒厚度与尺寸
向量二分），Core3D 3.1 `CSG_TreeBuilderOptions` 的默认容差 4 mm。`1cc594a` 的分段是近似（各面按节点画网格线）；
`8f37bc5` 改照 `IwTess` 建树、断边、二分，弦高仍取 6 mm（从 E3D 的分段反推）。

`1cc594a` 时 chord 15 只（GENPRI 10 / 14 / 13 各 5 台），差在 E3D 的分段密度，不在几何：GENPRI 10 / 14 是我们在相邻
柱面条的节点上画线，采到了 R5 半圆的最高点而 E3D 没采——`8f37bc5` 建树后对上。剩下 GENPRI 13 的 5 只：E3D 在
76 mm 长的条上把同样的 R5 圆弧切得更细（还有 1/3、1/6、1/24 的分点），这部分来自 `IwTessSrfCache::BuildTree` 里边界植入（`ImplantUVSegment` / `MarkFaceContainment`）
之后的细化或面三角化，尚未反编译。逐边证据见取证笔记「残差」。

回归门（标签 `w7b` 对 `w7g`，`gates-w7b.txt`；V6 修了读取集合条数钉后重跑 `gates-w7b-v6.txt`）：V1–V5、V7、V8
全部 0 变化；V6 全量测试 469 通过、0 失败；V9 只有 aps250162 标出 `CHANGED`：两侧都有 391 → 466，exact
391 → 451，chord 15（GENPRI 10 / 14 包围盒差 0.33 mm、GENPRI 13 差 0.49 mm），只在 E3D 75 → 0。APS 合计变为
8218 对 8066 exact。汇总贴草稿更新为「已修复 39 项，待解决 6 项」。gen-model 读取集合同步 `a34d58647` / `704f5d2f7`
（`model_impact` 单测 50 条通过）。

`8f37bc5`（标签 `w7b3`，`gates-w7b3-v6.txt`）只动 GENPRI 的剖分，而其余库没有 GENPRI（`w7b` 那轮 V1–V5、V7、V8
全不变已证明），只跑 V6 与 aps250162：V6 472 通过、0 失败（先红过一次：给只剩测试在用的 `is_planar` 挂
`#[cfg(test)]`，读取集合护栏截到第一个 `#[cfg(test)]` 就看不到 `ELDATA` 了，改成挪进测试模块）；aps250162
两侧都有 466，exact 451 → 461、chord 15 → 5，off 0。APS 合计变为 8218 对 8076 exact。e3d-model 本地领先 35 个提交，
gen-model refactor 领先 17 个。

收口（2026-10-22 用户「W7b 收口，开始 W7c」）：GENPRI 13 的 5 只记为 chord 档残差，`IwTess` 的边界植入与面剖分不再追。

### M8 · W7c（2026-10-22 完成）

| 工作包 | 提交 | 结果 |
|---|---|---|
| W7c 单 `DESP` 的 WALL | e3d-model `0184cb6` | `/100_thk_wall` 的目录截面是 `[0, CPAR[1]] × [0, DESP[1]]`，厚 100 在目录 `PARA`，DESP 只有高一项（4000 / 3800）。截面本来就走 SPRE 目录链，DESP 的「厚、高」只是链断时的退路矩形：改成 DESP 满两项才有退路，目录求不出又没有退路才报错。两堵外墙 exact，真库门 `aps250110_structures_real` |
| W7c 楼梯踏步 `TREAD` | e3d-model `f21e7a1`、`faf975e`；gen-model `a67fc44f9` | 目录件 GMRE 一只 SBOX，DTSE 把 `TWID` / `TDEP` / `THK` 接到 `ATTRIB TWIDTH` / `ATTRIB TREADD` / `PARA[3]`。`TREAD` 归 `Catalog`、走目录构件求值器；新模块 `stair` 按属主梯段现算两个伪属性：`TWIDTH` 取梯段的，`TREADD` = going + `TREADO`（going 取 `OGOING`，否则 RPATH 首末水平距离 ÷ round(竖向距离 ÷ `RISE`)），与 E3D 差 ±0.005 mm。读取集合 152 → 157（gen-model 同步 159）。`faf975e` 补登记祖先读护栏，并给 `increment::implicit_frame_host_of` 加 STRFLT 支：梯段或它的 RPATH 改了，整棵梯段子树进增量计划。25 只 exact |
| W7c 折弯板 `BPANEL` | e3d-model `230d20b` | 新模块 `bpanel`：RPATH 的 POINTR 中线（`FRAD` 倒圆，与扶手同一套段表）± `WTHICK`/2，沿 `YDIR` 拉到 `WIDT`；BPANEL 列为模型单元（只读自己与名下 RPATH / 负体，增量沿 owner 链上卷到它就够）。E3D 名下没有负体时逐段写一条 FacetGroup（N 个点 2N − 3 条，弯按曲面图元口径、至少 2 段）；有开孔时扣完整只一条，弯按布尔操作数的粗口径（屋面板 R25 弯只剩一条弦，按细口径会差 0.76 mm）。`gen_ams` 对 BPANEL 只写面片。52 只 exact，真库门加六只板的包围盒与逐段记录条数 |

取证：E3D TTY 只读导出 `aps250110_lv6_2013286654_*.rvm` 逐件取世界包围盒与记录条数（`output/rvm_group_aabb.py`），属性与目录链用
探针 `output/e3d-model-patch/examples/zz_tmp_noun_probe.rs`（不入库）。TREAD 的 going 按 RPATH 推算，与 E3D 差 ±0.005 mm，确切算式没有取证。

回归门（标签 `w7c` 对 `w7b`，`gates-w7c.txt`）：V1–V5、V7、V8 全部 0 变化；V9 aps250110 标出 `CHANGED`：两侧都有 3348 → 3427，
exact 3346 → 3425，只在 E3D 81 → 2（剩 `/BUILDING_01_LEVEL_03/HR_001` 的 RAIL 2，归 W7a），chord 1 / off 1 / 种类不同 10 不变；
aps250162 的 `CHANGED`（exact 451 → 461）是 `w7b` 那轮之后合入的 `8f37bc5`，其余 APS 不变。V6 474 通过、1 失败：`f21e7a1` 的
梯段上溯没登记进 `ancestor_reads_guard`（提交 TREAD 时只跑了相关几组测试），`faf975e` 补上后重跑（`gates-w7c-v6.txt`）476 通过、
0 失败。clippy 对 `bpanel` / `stair` 与 `increment` 的非测试代码无告警（`tests/gensec_end_datum_real.rs` 里 E3D 打印的 0.707106781
被 `approx_constant` 判错，`5a9582e` 起就有，不影响 cargo test）。APS 合计变为 8297 对 8155 exact、只在 E3D 95。汇总贴草稿
更新为「已修复 42 项，待解决 6 项」。e3d-model 本地领先 39 个提交，gen-model refactor 领先 18 个。

### M8 · W7a（2026-10-22，完成）

用户「继续修复…fix-plan.md」。取证把 aps250112 的 off 108 / 只在 E3D 93 与 aps250110 的只在 E3D 2 归成五类，全部已修：

| 工作包 | 提交 | 结果 |
|---|---|---|
| 站位 / 朝向一条规则 | e3d-model `5ee3cb8` | 立柱 / 端头 / 面板 / 门没有 POS / ORI。站位：从 `POSREF` 点沿离开它的边走 `ZDIS`，起量点是该边直段的起点（拐点 `FRAD` 圆角的切长先让掉）；闭合路径末点的边绕回首点；开口末点沿进入段往回量；`CURTYP THRU` 的边按弧长量；越过终点夹在终点。朝向：局部 +X 按 `HRJUSL`（`LEFT` 顺路径、`RIGH` 逆路径、`FORW` 取 路径 × Z），端头也一样——此前末点取「倒数第二条边反向」，再靠 36004 / `LEFT` 两条特例补。`XOFF` 偏在路径右侧、`ZOFF` 向上。aps250112 `/EL(+)100000` 的 5 根立柱（差 100 = `FRAD`）与 2 只端头（转反 180°），模板件共 102 件 off 归零 |
| 目录构件式立柱 / 端头 | e3d-model `09949be` | `HRPOST` / `HRTERM` 归 `Catalog`，GPART 有 CATR（→ SFIT）的自己成件、走目录构件求值器；没有 CATR 的是模板件（几何在 TMPL 成员上），不记 `catalog_pending`。aps250112 68 根立柱、15 只端头（`Return` 10、`Pads` 5）全出、exact；`Return` 回环起点 `LEFT`、末点 `RIGH`，两只都朝扶手里，正好确认朝向看 `HRJUSL` 而不是「朝路径外」 |
| 栏杆 | e3d-model `704c632`；gen-model `9216e2f72` | `STAREF` + `SZDI` → `ENDREF` + `EZDI` 两端按立柱同一口径取站位（aps250112 10 根 `STAREF == ENDREF` 的截段）；`CURTYP THRU` 的边换成过三点的圆弧、两邻直段与弧两端按切向斜接（aps250110 `/BUILDING_01_LEVEL_03/HR_001` 闭合路径的 RAIL 1 / 2）。读取集合 157 → 159（gen-model 并集 161） |
| 梯段上的布局式扶手 | e3d-model `bae472b` | `RAILHE` 是栏杆**顶**高：Ø40 的截面圆挂在抬起的路径下面，圆心沿截面「上」向 `U = normalize(Z − (Z·d)d)` 往下 20（平台段即 `RAILHE − 20`），两端平头、拐角斜接（此前轴线放在 `RAILHE`，斜段两端靠水平切面补齐包围盒）。踢脚板板厚 4、沿 `U` 离路径 10 起高 100，拐角斜接、开口端竖切（此前 8 × 110、从路径起）。相邻两节共用的斜接面在布尔并之后留下零体积的皮、每片单写一条记录，HANDRA 也走 `drop_subresolution_films`，与 E3D 一样整只一条面片。aps250112 `/EQUIP_SUPPORT/SF_001..003` 名下 6 只 off 20 → exact，aps250110 `/B01_LEVEL_03_ACCESS/SF_001`、`SF_002` 名下 4 只（此前 exact / chord 0.06）全 exact |

取证：E3D TTY 只读导出里，模板件每个图元与我们同名图元的刚体差 `D = W_e3d · W_ours⁻¹`（`output/rvm_frame_delta.py`）在同一只立柱里处处
相同，就是两边立柱标架之差；目录件与栏杆取图元世界包围盒（`output/rvm_prim_dump.py`）。属性、路径点与目录链用探针
`output/e3d-model-patch/examples/zz_tmp_noun_probe.rs`、`zz_tmp_handrail_census.rs`（不入库，普查在 `verification/regress-20261021/w7a-census-*.txt`）。
真库门 `aps_handrail_real`（新）：模板件与目录件的标架、目录立柱与两根栏杆的世界包围盒。布局式扶手取 E3D 面片顶点在路径标架里的
分布（`output/rvm_facet_band.py`：沿路径的距离、离路径线的高、横向偏）：平台段截面环中心在 `RAILHE − 20`（上平台 105980、顶 106000），
斜段端环垂直于轴、顶点正在端点上方；真库门加两只梯段扶手（aps250112、aps250110 各一只）的包围盒与记录条数。

详图式扶手（前三个提交，标签 `w7a` 对 `w7c`，`gates-w7a.txt`）：V1–V5、V7、V8 全部 0 变化；V9 只有 aps250110 与 aps250112 标出
`CHANGED`——aps250110 两侧都有 3427 → 3429、exact 3425 → 3427、只在 E3D 2 → 0；aps250112 两侧都有 678 → 771、exact
564 → 761、chord 6 → 4、off 108 → 6、只在 E3D 93 → 0，其余 APS 不变。V6 先 478 通过、1 失败（读取集合条数钉
没跟上 `SZDI` / `EZDI`），改钉后重跑（`gates-w7a-v6.txt`）479 通过、0 失败。APS 合计变为 8392 对 8354 exact、chord 31、
off 7、只在 E3D 0、种类不同 75。gen-model `model_impact` 单测 51 条过（读取集合 159 → 161）。汇总贴草稿更新为「已修复
45 项，待解决 6 项」。e3d-model 本地领先 42 个提交，gen-model refactor 领先 22 个（交付按提交号推，见 W6 末尾）。

梯段上的布局式扶手（`bae472b`，标签 `w7a2` 对 `w7a`，`gates-w7a2.txt`）：V1–V5、V7、V8 全部 0 变化；V6 480 通过、
0 失败；V9 只有 aps250110 与 aps250112 标出 `CHANGED`——aps250110 exact 3427 → 3428、chord 1 → 0；aps250112 exact
761 → 767、off 6 → 0、种类不同 16 → 10，其余 APS 不变。APS 合计变为 8392 对 8361 exact、chord 30、off 1（W7f 的
GENSEC）、只在 E3D 0、种类不同 69。aps250209 的 81 只布局式扶手跟着变，那本库不在 `/ALL`、没有 E3D 真值（归 W7h
记账）。汇总贴草稿更新为「已修复 46 项，待解决 6 项」。e3d-model 本地领先 43 个提交，gen-model refactor 领先 22 个。

余下一处不改：

- **记录条数**：E3D 把带 `FRAD` 圆角的栏杆逐段写（直段一条、每个圆角弧两条），我们写一条，aps250112 10 根记为种类
  不同；包围盒 6 根 exact、4 根 chord 0.13–0.15（圆角弧的分段不同），不影响几何。验收只看 off 与只在一侧，这条只记账。

### M8 · W7d（2026-10-22，完成；带出 W7i）

用户「继续修复…fix-plan.md」。W7d 记的「隐式管多出」大半不是多出：APS 普查宏（`aps_survey_lv6.mac`）没开「隐式管导成
单独容器」，E3D 把路由容器的隐式管直接写在容器自己那一组里（组名就是容器名），对拍工具配不上——E3D 那组发不出
主键，我们的 `TUBI n of <容器>` 落进只在生成侧。

| 工作包 | 提交 | 结果 |
|---|---|---|
| SUPC 不补管身 | e3d-model `1f02613`；gen-model `749681d8f` | `cachelib/GTTUBE` 硬编码的 noun 码只有 BRAN、TRUNNI、LUG；aps250104 TTY 导出里 16 只 TRUNNI 都有管身、7 只 SUPC 一根都没有。`DERIVED_ROUTE_CONTAINER_NOUNS` 去掉 SUPC，与 HANG 同路只下钻成员；gen-model 成员挪位的测试同步。已部署的库里旧的 SUPC 管身要靠整库重生成清掉 |
| 布尔结果不另写无体图元 | e3d-model `8102052` | aps250104 23 只锚固件 ANCI（目录 `/BT23-100`）扣过 3 把负体，E3D 只写一条布尔结果面片；目录里 `PDIA = DESP[11] = 0`、长 10 的 LCYL 没有体，不在布尔结果里也不另写。回归门里全部带零尺寸圆柱、又配得上记录种类的 74 组：走图元的 51 组 E3D 都写了（zone 66457 阀门的 `Cylinder[0, 120]`、aps250142 风管的零高圆柱），布尔结果的 23 组都没写。真库门 `rvm_record_kinds_real` 加 aps250104 一例 |
| 容器组里的隐式管 | e3d-model `fa72312` | `rvm_compare` RVM ↔ RVM 时，基准里有同名带几何成员、生成侧没有同名元素的容器，把它名下的管身并成一件（世界 AABB 取并、记录种类拼接、世界帧判档）按容器名配对；开了单独容器的导出里容器组没几何，不受影响。报告加 `container_tube_groups` |

取证：E3D 组 `/PS-01-00111/B1`（TRUNNI）一条 `Cylinder[44.45, 524.56]`，与我们的 `TUBI 1 of TRUNNI /PS-01-00111/B1` 原点、轴、尺寸一致；
`/KIOSK_PUMP_01_DISCHARGE-B1` 等 4 条泵房支管同（只差绕轴滚角，happyrust/e3d-model#1 那一类）。目录链用 vendor `catalogue_probe`
（ANCI `2013286648/560`、阀门 `24383/67118`），零尺寸圆柱的分布用 `output/w7d/zero_cylinders.py`、`zero_cyl_vs_e3d.py`，未配对账用
`output/w7d/unpaired_by_db.py`（都不入库）。

回归门（标签 `w7d` 对 `w7a2`，`gates-w7d.txt`）：V1–V3、V5、V7 全部 0 变化；V6 482 通过、0 失败；V4（`/SSC测试`）4523 →
4528，新配上 5 只容器全 exact；V8（ams8000 整库）2562 → 2614，51 exact、1 off、种类不同 28 → 29；V9 APS 合计 8392 →
8591 对、8361 → 8556 exact、chord 30、off 1 → 5、只在 E3D 0、只在我们 33 → 0、种类不同 69 → 50（aps250104 23 → 0；新配上的
容器组带进 4）。gen-model `model_update_plan` 单测：改的那条过；`partition_splits_pose_from_regen_and_respects_cancellation`
整组跑失败、单跑通过，把 SUPC 改回去整组跑也一样失败，与本轮无关（测试之间共用的状态；根因与修法见 §9「第二次审核」）。汇总贴草稿更新为
「已修复 49 项，待解决 6 项」。e3d-model 本地领先 46 个提交，gen-model refactor 领先 23 个。

带出的 W7i（容器组配上之后才看得见）：5 条支管的管身条数与 E3D 不同——aps250142 `/SUPPLY_WORKSHOP-001` 支管头我们多一根
25 mm 矩形管（另一根与 E3D 一致）、`/RETURN_LEVEL02-002/01` 少 1 根、`/SUPPLY_LEVEL02-001/02` 少 4 根（E3D 一条 Box、
四条 Cylinder，我们一条 Cylinder）；aps250124 `/100-LAYOUT300-307/B1` E3D 10 根、我们 8 根；ams8000 `/M-OR-1RX-3081D` E3D 3 根、
我们 5 根（多 240 mm 与 140 mm 两根）。逐条取路由与 P 点再改。

W7e 的取证已做一半：E3D 的 `Line` 记录是沿记录 Z、以原点为中点的一段，参数 `[5, 长]`、声明盒
`[-5, -5, -长/2, 5, 5, 长/2]`；来源是目录 `LINE`（`PTS` 是 P 点号表、以 1000 收尾），`TUFL` 为真的才写——acp7000 里
`TUFL` 为真的 26 条全是两点线，aps250132 `/V118` 那条从 P0 到 P3、长 292。ams 阀门的 `LINE` 都是 `TUFL` 假，E3D 不写，
我们也不写。

### M8 · W7i（2026-10-22，修 2 条；3 条转 W7j）

W7d 带出的 5 条支管逐条取了路由与 P 点（vendor `bran_audit`，`output/w7d/bran-audit-*`）：

| 工作包 | 提交 | 结果 |
|---|---|---|
| 放坡 TEE 按 `SLODIR` 判共轴 | e3d-model `69dea3f`；gen-model `c6cd27a85` | aps250124 `/100-LAYOUT300-307/B1` 按 1:75 放坡（`SLOREF /1in75`，`DEFSLO 0.7639°`）：两只弯头转了坡度角，中间 TEE `2013286668/683` 没转，`SLODIR` 把 P1 / P2 各压低 0.7639°。按目录方向判，两侧槽位各偏 0.764°、不建；E3D 两根都画（1042.81、711.16 mm）。`SLODIR` 是 E3D 坡度工具 `PSLDIR n` 的存储（E3D 2.1 `PMLLIB/design/objects/pipeSlopeComponent.pmlobj`）：第 n 个三元组是 Pn 的实际方向，全零没设，P 点位置不动。按属主系解（与 `POS` / `ORI` / `HDIR` 同口径）；全语料只有这一只非零、`ORI` 为零，属主系与构件系在数据上分不出。读取集合 159 → 160（gen-model 并集 162） |
| 风管分流片 SPLR 穿过 | e3d-model `d8d4af7` | aps250142 19 只 SPLR：18 只坐在两件接缝上，进出槽位与穿过后的合并槽位都是零长；`/SUPPLY_WORKSHOP-001` 的 `2013286686/18` 伸进下一件 25 mm，当站点会多画一根 25 mm 矩形管，E3D 没有。零长构件不一概穿过：同一条支管的 BRCO `2013286686/56` 也零长、也错开 144.7 mm，E3D 照画它到下一件那根。与风管链上的 ATTA 同在 `station_verdict` 里 |
| 测试注释挪位 | e3d-model `d5e343d` | W7d `8102052` 把耳轴用例插在 `assert_world_aabb` 的文档注释与函数之间，挪回去 |

转 W7j 的 3 条（各要一块新能力，不硬改）：

- aps250142 `/RETURN_LEVEL02-002/01` 少一根头管（2374.5 mm、r 225）：`HSTU /AVEVAHVACSPEC/HCTUBE` → `/HCTUBE` 的 TUBE
  `PDIA = ( :0x17EF4B61[1] )`，管径是设计侧 UDA。要接 UDA 定义、按哈希精确取值；按同名回退会把 ams7997 的口径带歪。
- aps250142 `/SUPPLY_LEVEL02-001/02` 少 4 根：扁圆风管 `LSTU /AVEVAHVACSPEC/FOTUBEB` → `/FOTUBE2`，管身目录件是 BOXI + 两只
  TUBE（P3→P4、P3→P5）加保温；E3D 写一条 Box、四条 Cylinder，我们按圆管写一条 Cylinder。要让隐式风管按管身目录件的
  GMSE 逐图元求值。
- ams8000 `/M-OR-1RX-3081D` 多 2 根（240、140 mm）：托架支撑 ANCI 成对挂 DATUM / SREF，槽位一正一反；
  `/C-GR-1RX-3020B`（`24384/2944`）说明 DATUM 是站点、`LSTU` 不是判据，规则还没定。

回归门（标签 `w7i` 对 `w7d`，`gates-w7i.txt`）：V1–V5、V7、V8 全部 0 变化；V6 485 通过、0 失败；V9 aps250124 exact
231 → 232、off 1 → 0；aps250142 exact 518 → 519、off 3 → 2。APS 合计 8591 对、8556 → 8558 exact、chord 30、off 5 → 3、
种类不同 50 → 48。

### M8 · W7e（2026-10-22，完成）

| 工作包 | 提交 | 结果 |
|---|---|---|
| 目录 LINE 写 `Line` 记录 | e3d-model `845f4e5`；gen-model `635bf7a90` | 过得了表现级（`TUFL` 为真）、两点的目录 `LINE` 照 E3D 写 `Line[5, 长]`：线段从 `PTS` 第一点到第二点（P0 是构件原点），记录原点在中点，声明盒 `[-5, -5, -长/2, 5, 5, 长/2]`。`TUFL` 假的照旧不写，多点线没有样本、照旧只记账，布尔结果与其他无体图元一样不另写。读取集合 160 → 161（`PTS`，gen-model 并集 163） |

记录标架只看线段的世界方向：Y 取世界 −Z 在垂直于线段那张面上的投影，竖直时取世界 +Y，X = Y × Z。APS 普查 TTY 导出里
2504 条 `Line`（26 条阀杆线、2478 条 GENSEC 基准线，竖直 821、斜 1683）无一例外；与构件自己的 X / Y 无关——26 只
阀门里 16 只的记录 X / Y 与阀门的不同，ORIMAT 也对不上。取证脚本 `output/w7d/line_records.py`、`line_frames.py`、
`line_vs_world.py`、`line_frame_rule.py`（不入库）。

回归门（标签 `w7e` 对 `w7i`，`gates-w7e.txt`）：V1–V5、V7、V8 全部 0 变化（AMS 那几套没有多出 `Line`）；V6 487 通过、
0 失败；V9 aps250132 种类不同 14 → 0、aps250160 12 → 0。APS 合计 8591 对、8558 exact、chord 30、off 3、只在 E3D 0、只在我们 0、
种类不同 48 → 22。余下的 22：aps250110 门框模板里 10 只整圈的 REVO（E3D 拆两条写、几何 exact，ISSUE_09 第 4 类跟）、aps250112
10 根带 `FRAD` 圆角的栏杆（W7a 已归因）、W7j 的 2 条。off 3：aps250110 那只 GENSEC（W7f）与 W7j 的 2 条。

### M8 · W7f（2026-10-22，归因、不改）

aps250110 `GENSEC 1 of FRMW /AREA02_PR_AXIS_P2.2`（`2013286654/1821`，UB203x133x25，`JUSL CTOP`）不是弯梁：SPINE 只有两个
`POINSP`，从 `(0, 0, 0)` 到 `(0, −4676.8, −7.56)`，是一根 1:620 放坡的直梁；SPINE `DRNS = (0, −1, 0)`、`DRNE = (0, 1, 0)`，两端竖直斩切。
两端另有 FIXING 模板的退刀（起点 `JOIS 2013286654/1827`，`/J2S4-BWC-PFI-PFI-1R`），起点退 10 mm、终点退 8 mm。

逐点对了两侧面片（`output/w7d/facet_ends.py`、`facet_slivers.py`）：端面、翼缘、腹板全一致（起点端面 y = −9.995 / −9.996）。
差在一片楔形薄片：退刀的底面是水平的，梁底随坡往下走，退刀区里下翼缘底下留下一片最厚 0.016 mm 的楔形；终点是
上翼缘顶上同样一片。E3D 的楔形尖端落在斩切面上（y = 0.005 / −4676.795），我们的落在 −0.531 / −4676.258；局部包围盒
的 0.537 mm 全是这两个尖端。尖端在哪里取决于退刀底面与梁底的高差：两侧只差约 0.0008 mm，沿 1:620 的坡放大成
0.5 mm。这是个病态的近共面布尔，追到微米级对齐 E3D 的浮点没有意义，不改。对拍工具现有的
`reference_artifact` 只滤断开的薄片，这片连着主体，所以仍记 off。

### M8 · W7h（2026-10-22，aps7200 已对拍；问题归堆进 W7k）

aps7200 不在原项目的 `/ALL`，在项目副本上导：宏 `verification/civil-tty-20261021/aps7200-w7h/aps7200_lv6.mac`（由
`aps_survey_lv6.mac` 改前缀、只读），用 `verification/e3d-tty-native-20261015-075723/run_copy_tty_long.ps1`（与
`run_copy_tty.ps1` 只差 TTY 超时 1800 s）跑，`result.json`：隔离运行时与副本都钉住、无环境泄漏、无写库命令。
25 个 ZONE（4 个 SITE）各导一份，生成侧是 `output/aps_zones.py aps7200`（当前代码，2406 件、失败 0），对拍
`verification/regress-20261021/aps_survey_aps7200-w7h.txt`。

合计 2294 对、2261 exact、chord 3、off 30、只在 E3D 58、种类不同 27。按问题归堆（W7k，都还没取证根因）：

| 堆 | 件数 | 现象 |
|---|---:|---|
| 结构节点 `SJOINT`（`SNODE` 名下） | 34 | 只在 E3D：我们不出几何 |
| 电缆桥架路径点 `POINTR`（`CWBRAN` 的 `RPATH`） | 20 | 只在 E3D |
| 矩形风管 `/HVAC/RECT/...` | 14 | 11 只弯头 E3D 多一条 `RectangularTorus`、我们 off 125 mm；一条支管的管身 E3D 写 Box、我们写 Cylinder（off 49.5）；REDU off 16.4；`/HVAC2-B1/ITEM8` off 375 |
| 吊架 `PCLA` / `PCLAMP` | 12 | off 6（104 mm 两只、3–4 mm 四只，记录条数也不同）、chord 3、`PCLAMP of HANGER` 只在 E3D 3 |
| 踢脚板 FIXING 与 WALL | 5 | `/OUTER-` / `/INNER-` / `/EAST-KICKPLATE` 的 FIXING 与 `/INNER-KICKPLATE` off 510–1885 mm；`WALL 1 /C1101-BASE` off 1742 mm |
| 其余 off | 5 | `SBFI 1 CMPF 1 of PANE /FLR-PLT1` 30 mm；`/B-BC-1A`、`/C-BC-1A` 11.95 mm；两只 `RTOR` 0.87 mm |
| 只差记录条数（几何 exact） | 9 | 整圈设计 REVO 2（与 aps250110 同类）、`/TAPER-SCTN1..4`（E3D 1 条、我们 2 条）、`POHE`（E3D 8 条、我们 1 条）、`FITT 1 STWA 4 of /WALLS` |
| 其它只在 E3D | 1 | `CYLINDER 3 of SUBSTRUCTURE 1 of STRUCTURE /EQRACK.5M.HR5` |

aps250209 / aps250212 同样不在 `/ALL`，2026-10-24 在副本上登记后导出对拍，见下一节 W7t。

### M8 · W7t（2026-10-24，aps250209 / aps250212 在副本上对拍；副本 SYS 库用 e3d-io 写路径登记）

副本 `verification/e3d-tty-native-20261015-075723/projects/AvevaPlantSample/aps000/apssys`（2016 年的 SYS 库）里根本没有
`DBNO` 250209 / 250212 的 DB 元素（243 只 DB 元素里有 250204–250208、250215–250217、250220、250221，就是没有这两只），所以不是
「挂进 `/ALL`」而是先登记。E3D ADMIN 在 TTY 下会崩（S4 那次 0xC0000005），照 2026-10-18 挂 aps7200 的路（会话 139 只给 `/ALL` 的
`CURD` 追加了 `24575/848`，`e3d-descriptor diff --from 138 --to 139` 核过只改这一项，成员表没动）用 e3d-io 的 `Transaction` 做，
临时例子 `tmp_sys_mount`（用完已删）：

- `Transaction::open` 基线会话 139，`SpatialPolicy::Off`；模板 `sysvir.dat`。
- `insert` 两只 `DB` 到 `*SAMPLE` 队的 DBLI `24575/444`（aps7200 的 DB 元素 `24575/848` 也在它名下），名字 `*SAMPLE/DESI250209`、
  `*SAMPLE/DESI250212`，属性照 `24575/848` 抄：`DBNO` 250209 / 250212、`STYP 1`（DESI）、`FINO 0`（文件名按 dbno）、`AREA 0`、
  `CRCY 3`、`DACC 1`、`CLAI 2`、`EXTNO 1`、`DESC` 说明是副本登记；`TYPEX [1, 531499]` 由写路径补。
- `update` MDB `/ALL`（`24575/905`）的 `CURD`：137 → 139，追加 `24575/1041`、`24575/1042`；成员表（`DBLS` 那份）不动，与 139 的做法一致。
- 先 `stage` + `rollback` 走一遍：会话 140、追加 15 页，回滚后文件 SHA256 不变；再 `commit_reporting`：会话 140，`merged_onto None`、
  `reminted []`。写前的副本留在 `civil-tty-20261021/aps250209-w7t/apssys.bak-ses139`。原项目 `aps000/apssys` 没动。
- 读回核：两只 DB 元素的属性与 `24575/848` 同一套（只差 DBNO / NAME / DESC），DBLI 成员 5 → 7，`CURD` 139。

导出：`output/w7d/make_aps2502xx_macro.py` 生成的只读宏（`aps250209-w7t/aps250209_lv6.mac` 13 个 ZONE、`aps250212-w7t/aps250212_lv6.mac`
1 个），`run_copy_tty_long.ps1` 跑，`result.json` 两份都 `tty_exit 0`、无写库命令、无环境泄漏、运行时与副本钉住；登记前同一份宏
13 个 ZONE 全部 `dbref / export add failed`，登记后 13 + 1 全部 `exported`。生成侧 `output/aps2502xx_compare.ps1 -Tag w7t`（新脚本，照
`aps7200_compare.ps1`），对拍 `verification/regress-20261021/aps_survey_aps250209-w7t.txt`、`aps_survey_aps250212-w7t.txt`。

**aps250209**（13 ZONE；生成 4583 件、失败 22、unknown 892）：4583 对、4564 exact、chord 0、off 19、只在 E3D 40、种类不同 2。按堆：

| 堆 | 件数 | 现象 |
|---|---:|---|
| 梯段上的布局式 HANDRA（`HANDRA 1 / 2 of STRFLT n of SBFRAMEWORK /...-STAIRS`） | off 16 | 只差路径终点：我们的盒沿路径到 2954.02，E3D 到 2944.95（梯段 STRFLT 的盒到 2954.95）——顶端比 E3D 长 9.07 mm，E3D 在梯段顶边前 10 mm 收口；原点、宽、高全同（aps250112 那 6 只修到 exact 的是另一种端部） |
| 带圆弧踢脚板的布局式 HANDRA（`FRMWORK` / `SBFRAMEWORK /...-HANDRAILS` 名下） | 只在 E3D 22 | 生成侧 22 只 `failed`：`kickplate arc 尺寸退化: layout kickplate does not support arc segments`；E3D 每只写一条面片 |
| 梯段 `STRFLT n of SBFRAMEWORK /...-STAIRS` 自身 | 只在 E3D 18 | E3D 给 STRFLT 自己写一条面片（盒 900 宽 × 2955 长、高 −263..1980，是梯段的踏步与斜梁），我们只画它名下的 HANDRA；此前记在 `evidence_pending` |
| 设备模板 `CTORUS 1 / 2 of TMPLATE 1 of EQUIPMENT /A02-AREA05-PRIMARY-CRUSHER` | off 2（409.56） | 半圈管 `RINS 1558.44 / ROUT 2377.56 / ANGL 180`（中心线半径 1968、管半径 409.56，两边顶点都对得上），名下 3 个负体：`NCTO`（1588.81 / 2337.31，掉空）、`NRTO`（1968 / 2377.56、HEIG 1151.56，切外半）、`NRTO`（1555.62 / 1977.18、HEIG 775.34、POS z −290）。E3D 的布尔结果是内侧半管（rad 1552–1968、z ±409.56，盒 ±1968）；我们三个负体都扣了，主体只剩 z 97.7–409.6 的上段、另带 22 片碎块（r 1532–1817、2235–2340），盒仍 ±2377.56——第二只 NRTO 的落点 / 高度口径或布尔本身有差，待逐个取证；本库其他 8 只 CTOR exact |
| 整圈 REVO（`/AREA05-CV_0001/Belt` REVO 4、`/AREA07-CV_0001/Belt` REVO 2） | 种类不同 2、off 1.18 | E3D 拆两条写（ISSUE_09 既有类） |

**aps250212**（1 ZONE `/TERRAIN-A-ZONE`；生成 3 件、unknown 547）：0 对、只在 E3D 6。E3D 给地形模型 `TRNMOD /TerrainA/DEF01/MOD01 → TRNCC 1 →
TRNCEL 1..6` 各写一条 `GENPRI 1` 面片（世界系、盒 125 m 级，归档在 `ARCHIV` 下——与设备 GENPRI（W7b）同一套 B-rep 载体），TRNCUC / TRNCUS
不写；我们对 TRNMOD 子树一个都没进（`unknown_nouns` TRNMOD / TRNCC / TRNCEL / TRNCUR …，`evidence_pending GENPRI 8`），只画了
`TRNDEF /TerrainA/DEF01` 下 `TRNPRI 1` 的 CYLI / BOX / BOX 三件——TRNDEF 在 TRNWLD 下、不在这个 ZONE 的导出里，不可比。

**截图与 issue（2026-10-24，用户「更新到 github issue 以及配套的截图」）**：五堆各一张图，由 `output/w7d/render_w7t_pngs.py` 把两边 RVM 的
面片按世界 mm 渲染（matplotlib），入库在 gen-model `docs/assets/2026-10-24-aps2502xx-tty-compare/`（`docs/evidence/` 被 `.gitignore` 挡着，改走
`docs/assets/`——#7 那张日志截图也在这里），提交 `ed872f5ee` 已推到 `origin/refactor`（推前本地与远端同步，只多这一个提交）；私有仓用
`blob/refactor/...?raw=true` 链接嵌进 issue。按用户选的范围跑 `submit.ps1 -Only ISSUE_09,ISSUE_13,ISSUE_06,ISSUE_03`，建了 4 个 issue：
`happyrust/e3d-model#7`（ISSUE_09 记录写法）、`happyrust/e3d-model#8`（ISSUE_13 APS 覆盖，含第 14 类与五张图）、`happyrust/gen-model#62`
（ISSUE_06 覆盖计划）、`happyrust/gen-model#63`（ISSUE_03 汇总贴），`submitted.json` 已记。草稿里的 e3d-model / e3d-io 提交链接要等推送后才能打开；
对 #58 / #34 / e3d-model #1 #5 #6 的 5 条评论按用户决定等推送后再跑一次 `submit.ps1`（会自动跳过已建的 4 个）。

**W7t 修复（2026-10-24，用户「按顺序继续。直到所有的模型问题都修复完成了」）**，按建议顺序五堆全修，每步先取证 E3D 的写法再改，每步过全部回归门
（`gates-w7t5` … `gates-w7t8.txt`）；修后截图在 `docs/assets/2026-10-24-aps2502xx-tty-compare/*-fixed.png`（gen-model `0482c73f9`）：

1. **梯段扶手顶端 9.07 mm**（e3d-model `df70971`）：E3D 的开口端在 RPATH `DRNS` / `DRNE` 非零、且与该段既不平行也不垂直时是过端点、法向 = 端向的
   平面（Core3D `setMitrePlanes`，复用 GENSEC SPINE 的 `spine_mitre_plane` 门）：这些梯段的 `DRNE = +Y` → 斜段顶端竖切，我们按平头多伸 9.07 mm。
   `handrail::apply_end_directions`，祖先读守卫登记 `DRNS` / `DRNE`。off 16 → 0。
2. **圆弧踢脚板**（同一提交）：E3D 的踢脚板不跟弧——直板画到原拐点、按角平分面斜接（`/A01-AREA05-4TH-FLOOR-HANDRAILS` FRAD 100 的水平拐角、
   `/A02-AREA07-HANDRAILS` 竖直面里的内弯拐角都是）；现在踢脚板按没有倒角的路径重排直段（`handrail::band_segment_solid`），栏杆照旧走弧。
   布局式 HANDRA 整只一条 FacetGroup（`gen_ams` `e3d_exports_facets_only`）。只在 E3D 22 → 0。
3. **STRFLT 自己的面片**（`eb7e633`）：`INSTOP` 以 LAYOUT 开头的梯段没有 TREADS / STRSTR 成员，E3D 给 STRFLT 自己写一条面片：两条宽 50、
   深 `FTHICK` 的斜梁贴在 `TWIDTH` 之外（中心 ±(TWIDTH/2 + 25)，正是 HANDRA 的 POS）、拐角斜接、两端按 RPATH 端向；斜段 `n = round(升高 / RISE)`
   级踏步盒（进深 g = 水平投影 / n、探 `TREADO`、厚 g/10，第 0 级只在前面有平台段时画、第 n 级不画）；水平段是宽 `TWIDTH`、厚 g/10 的平台板。
   新模块 `stair_flight`，`category::is_model_unit` 认 STRFLT 为单元（gb==2 容器护栏的唯一例外），读取集合 163 → 168（gen-model `86c0cf9c5`；最初的 `525caa665` 被同一工作区里另一条线的 `git reset` 摔掉、已重提）。
   只在 E3D 18 → 0；aps250112 的 3 段梯段顺带多 3 对 exact。踏步厚是 g/10 还是 FTHICK/10 还要一条两者不等的样本（语料里 FTHICK 恰等于 g）。
4. **CTORUS 负体布尔**（`87c0acb`）：不是负体口径，是正体的格——E3D 把 LNEGIT 的最终结果写成粗面片时正体也按 10 mm 口径离散（扫 18 段、
   管 16 段，与 NCTO / NRTO 同一张格；CYLI / CONE / DISH 早已这么做，CTOR / RTOR 漏了）。切外半的 NRTO `ROUT` 与正体相同，同格下外弦重合、切得干净；
   我们 78 × 64 的细格正体在 18 条粗弦之间探出去，差完留一圈 22 片碎块。修后体积 8.9620e7 对 E3D 面片的 8.9621e7 mm³，连 E3D 自己留的碎片都一样。
   同类证据 aps250164 `/C1101-SKIRT`（扫 3 管 8）、aps7200 `/C1101-PL1-BASE`（RTOR 扫 15 段 11°）。off 2 → 0；aps7200 两只 RTOR off 0.87 → exact
   （2352 对 2348 exact、off 1）。语料里带负体的环 347 只（ams7997 340 只 RTOR 不在 TTY 导出里），包围盒不随格数变。
5. **地形 GENPRI**（`0b7b1b5`）：不是没进子树，是 `ELDATA` 的归档版本是 4（解码器只认 2，6 只全 `failed`）。版本 4 没有 SMLib B-rep，是
   `GeoGenericPrimitiveImpl::ReadFrom` 在版本 ≥ 3 多读的缓存面片（字节流：顶点号流 / 顶点表 / 法向表 / 法向号流，布局见 `genpri` 模块文档），
   E3D 的导出写的就是它：6 格的多边形数 / 环数 / 点数 / 包围盒逐一相同（一张面的几个环按环向分：反向的是孔、同向的各写一个多边形）。
   它们是带边界环的高程面、不成体，照 E3D 只写 RVM（`bodyless` 通道的 `OpenFacets`），模型集不进。TRNSUR 的那只变换类型是 2，不在导出里，照例拒读。
   aps250212 只在 E3D 6 → 6 对全 exact。

回归门：V1–V5、V7、V9 其余库与 aps7200 逐轮 0 变化，V6 507 → 514 通过（新增真库门 5 条 + 单元 2 条）；w7t8 那轮 V8 ams8000 多出的 1 件 off
（`BOX 1 /1-LNR-Q005-PJ` 500 mm）是另一条线的测试当时正在改活库 ams8000（EQUI 被挺高 500 再改回，库文件 20:24 还在变），重跑 `w7t8b`
回到 2613 exact / off 1。取证脚本：`output/w7d/ctorus_facet_probe.py`（两边面片的 r / z 分布与体积）、`genpri_v4_probe.py`（版本 4 字节流）、
`terrain_facets_match.py`（缓存面片与 E3D 逐环对照）。

### M8 · W7k（2026-10-22 起，按建议顺序做；矩形风管弯头已修）

**矩形风管弯头**（e3d-io `ca949ef`；e3d-model `3a5dadb` 真库门）。aps7200 的 11 只矩形风管弯头（目录 acp7021 `/DBS`）`P2` / `P6`
的轴是 `[26, 64]`，E3D 打印为 `Y DESP 14 -Z`：命名角轴里正的角度字是一个 `DECARG` 操作数，与目录元组的操作数同一套分段
（1..=50 `PARAM n`、51..=100 `DESP n`……），64 是 `DESP 14`，弯头的 `DESP[14] = 90`。e3d-io `axis_spec` 此前按 `PARAM 64` 读，
构件的 `PARA` 只有四项、出界得 0：两只 P 点不转、弯头是直的，E3D 写的本体 `RectangularTorus[100, 300, 400, 90°]` 我们整条没写，
对拍 off 125 mm。现在角度字走 `catalogue_expr` 的操作数解码（`operand_number` / `operand_text`），`PARAM` 段读法不变。

结果（`aps_survey_aps7200-w7k.txt`）：11 只弯头和跟在后面的 `REDU 1 of /HVAC/RECT/M1/B1`（off 16.4 mm，位置随弯头的 P 点）
对上，exact 2261 → 2273、off 30 → 18、种类不同 27 → 16，没有新问题。回归门（标签 `w7k` 对 `w7e`，`gates-w7k.txt`）：
V1–V5、V7–V9 全部 0 变化，V6 488 通过、0 失败；V9 的普查表从这一轮起带上 aps7200。矩形风管余 2 件：
`BRAN /HVAC/RECT/M1/B2` 的管身 E3D 写 Box、我们写 Cylinder（off 49.5）与 `/HVAC2-B1/ITEM8`（off 375），排在后面。

e3d-io 本地现在领先 `github/main` 2 个（交付的 `3bed2ba` 加这个），交付仍按 `3bed2ba` 推。

**余下各堆的根因与修复方案**（2026-10-22，用户要求写进 issue；逐堆的现象与证据见
`output/github-issues-20261021/13-e3d-model-aps-coverage.md` 第 9 类，取证输出在 `output/w7d/`）：

| 堆 | 根因 | 方案 |
|---|---|---|
| SJOINT 34 | `SJOI` / `PJOI` 整类判「无三维表示」；带 `SPRE` 的走 JOIN 目录几何，E3D 按结构级别 0 过滤（托座 62、底板 18 的图元 `LEVE` 6–10，全滤掉；螺栓件、法兰件各 26 个可见图元，与 E3D 条数一致） | 带 `SPRE` 的改目录构件；标架 `GJTORI` / `GJTORG` 先取 TTY 真值再定 |
| `/B-BC-1A`、`/C-BC-1A` off 11.95 | 法兰 SJOI 的 `NSBO`（`NAPP = -1`）在 SCTN 端关节的 mode 2 门被挡 | 复核 mode 改写，mode 2 放行 `-1`，与 SJOINT 一起验 |
| POINTR 20 | 空桥架分支的路径点记号（10 mm 世界轴立方体），我们判成路径数据 | 确认画记号的条件后照画（或定口径不画） |
| 吊架 12 | e3d-io `catalogue_pml::decode_number` 把负指数定点数（0.25 = `16384 0 -1`）读成 NaN，图元与一只负体被跳过（aps7200 20 处） | 先认 ±64 内的定点数、再看双精度位 |
| 踢脚板 4 | `datum.rs` 读了 JLDATU 的 `PKDI` 没用；`CENT` 端点在 `RADI` 圆内时退直段 | 站位加 `PKDI × 全长`；`CENT` 按端点半径画弧 |
| 弧墙 1 | `DRNS` / `DRNE` 当全局半空间斜切，165° 的弧被切剩一小块 | 核坐标系，斜切只作用于端部 |
| SBFI 1 | PANE 上 CMPF 名下的 SBFI 差一个板厚（待取证） | TTY 取位姿后定规则 |
| CYLI 1 | 尺寸 0 跳过，E3D 写 `Cylinder[0, 0]` | 写零尺寸记录 |
| TRNS 1（`TRNS /HVAC2-B1/ITEM8`，off 375，即 W7h 堆表矩形风管里的那件） | 偏心 LSNO 的 `PBAX = P7 / P8` 方向差 90°（待取证） | 核 P7 / P8 的轴描述 |
| 头管 1（`BRAN /HVAC/RECT/M1/B2` 的管身，off 49.5，E3D 写 50 × 100 × 2150 的 Box、我们写 1 mm 兑底圆管） | 截面求不出才落到 1 mm 兑底，疑同 W7j 的头管 UDA | 与 W7j 一起 |
| 写法 9、RTOR 2 | 记录条数 / 面片切分 | 并进 ISSUE_09 |

修复顺序改为：吊架解码、`PKDI`、`CENT`、零尺寸 CYLI → SJOINT（含梁端负体门）→ 弧墙、SBFI、TRNS → 头管 UDA 与扁圆风管 →
POINTR（先定口径）。用户要求用 gh 建 issue：按约定 gh 写操作由用户跑，`submit.ps1` 加了 `-Only`（可只建 ISSUE_09、ISSUE_13）。

**第二批**（2026-10-22 23:57）：先在项目副本上用只读 TTY 取了 74 件的世界位姿（`verification/civil-tty-20261021/
aps7200-w7k-truth/`：34 只 SJOI 与 SNOD、踢脚板的 datum / FIXING、`/FLR-PLT1` 的 CMPF / SBFI），再修三堆：

| 堆 | 提交 | 结果 |
|---|---|---|
| 吊架 | e3d-io `9440bae`：`decode_number` 先认 ±64 内的定点数 | off 6、只在 E3D 3 → exact；chord 3 不变（面片切分）；`FITT 1 STWA 4 of /WALLS` 少的 2 条面片补上 |
| 踢脚板 | e3d-model `efa8573`：JLDATU 加 `PKDI × 全长`；`CENT` 端点在圆内时半径取终点距、走劣弧 | 3 只 FIXING exact；`/INNER-KICKPLATE` 梁身 off 641 → 5.6（圆弧切分） |
| 板上套管 | e3d-model `8974275`：PANE 上 CMPF 名下的 SBFI 贴 CMPF `SJUS` 指的板面 | off 30 → exact |

板上套管的第一版把 FLOOR / GWALL 也算进去，回归门 `w7l` 拦下了 ams1112（V2，47 只）与 `/SSC测试`（V4，12 只）的套管：
两本库的 149 只 CMPF 都在 GWALL 上（`SJUS DBOT`），`POS.z` 是沿墙高的位置。收窄到 PANE 后的回归门 `w7l2`（对 `w7k`）：
V1–V4、V7–V9 全部 0 变化；V5 rest 有 45 只 MDS FIXING 的体积变了 1e-15 量级（位姿、包围盒、三角形数都不变）；
V6 491 通过、0 失败（新增真库门 `aps7200_frames_real` 3 条）。aps7200：2297 对、2286 exact、chord 3、off 8、只在 E3D 55、
种类不同 9。e3d-io 现领先 `github/main` 3 个、e3d-model 53 个，交付仍按 `3bed2ba` / `230d20b`。

SJOINT 的标架规则也从同一批真值里定出来了（34 / 34）：Z = `CUTP`；Y₀ = 宿主 SCTN 轴去掉沿 Z 的分量、
X₀ = Y₀ × Z、再绕 Z 转 `BANG`；原点是 `CREF` 梁的 `JLIN` 线与宿主截面 `POSL` p-line 的交点。脚本
`output/w7d/sjoi_frames.py`、`sjoi_origin_check.py`。下一步做 SJOINT。

**第三批**（2026-10-23 01:40）：SJOINT 已修，e3d-model `28d0402`。

- 带 `GMRE` 目录件、标架有样本的 SJOI 改判目录件：aps7200 的 34 只（螺栓连接角钢 32、法兰端板 2）各写 26 条记录，
  与 TTY 一致；62 只焊接托座按表现级整只不画，与 E3D 一致。标架按上面那条规则接进 `transform`（`SectionJoint`）。
- 法兰端板 NGMS 里的 `NSBO`（`NAPP -1`）在 SCTN 端关节的 mode 2 放行：`/B-BC-1A`、`/C-BC-1A` 梁端短 11.95 mm，exact。
- Z 的符号跟着连接梁的滚转：第一版只按 `CUTP` 放，回归门 `w7m` 拦下了 ams7324 `/MDS/FT09` 两只 `IJOINT1`
  （角板翻到柱的另一侧，差 62 mm）与 ams7330 一只自连的 PowerClick 关节（差 235 mm），另有 `noun_coverage`
  护栏红。aps7200 的 34 只连接梁全是 `BANG` 0（Z = `CUTP`），MDS 有 E3D 导出的 4 只连接梁全是 `BANG` 180
  （Z = −`CUTP`，角板落在梁那一侧），合起来是「关节的 X₀ 与连接梁截面的 X 反向」。连接梁转 ±90° 的、连的就是
  宿主自己的还没有样本，照旧不画、不切梁端（`datum::section_joint_cut_sign`）。
- 增量：关节级联推广到 SJOI ↔ SCTN；SNOD 里挂着目录关节的 SCTN 进现算标架级联。读取集合多 `CUTP`、`JLIN`
  （e3d-model 163、gen-model 165）。
- 回归门 `w7m2`（对 `w7l2`）：V1、V3–V5、V7–V9 0 变化；V2 只有 ams7324 两只 `IJOINT1` missing → exact；
  V6 496 通过、0 失败。aps7200：2331 对、2322 exact、chord 3、off 6、只在 E3D 21、种类不同 9。e3d-model 领先
  `github/main` 54 个，交付仍按 `230d20b`。

**SNODE 点标记**（2026-10-23 01:55，用户「继续修复 SNODE 的绘制显示」）：e3d-model `3e1b1e2`。

- E3D 给每只 SNOD 写三条 `Line[5, 20]`（Core3D `gm_CreateMark`）：以 SNOD 站位为中点，按 X / Y / Z 沿宿主 SCTN
  所在属主系的三轴，不随杆件轴与 `BANG`（aps7200 的 191 只全是：SBFR 下一律世界三轴，转过 −15° 的 STRU、墙上
  FITT 模板、转过 −90° 的楼梯结构里的跟着属主系转）。对拍工具把线记录两边都丢，前面的数里没有它。
- 点标记走新的只写记录通道 `GenerationOutcome::marks`，只进 RVM；模型集、`elements.json`、增量都不动。
  gen-model 的烘焙要求每件都有实体（只有线的件会让整根失败），GLB 导出也只有网格，界面要显示节点线得另加
  线几何的存储与显示（方案待定；`e3d_mesh_store` / `mesh_glb` / `handlers` 现在有别的工作线的未提交改动）。
- aps7200 191 只逐只对上（原点 0.01 mm、线的方向与记录标架 1e-4，`output/w7d/snode_parity.py`）。回归门 `w7n`
  （对 `w7m2`）：V1–V5、V7–V9 0 变化，V6 497 通过、0 失败。e3d-model 领先 `github/main` 55 个。
- E3D 另有 37 组只有线的记录不是节点，仍不写：名字是 `/GRID-STABILIZER/1/X1…Y7`、`/CABLE001…006`、`/RN001`、
  `/ControlNode1` 的 21 组，没有 `SPRE` 的 SCTN 中心线 6 根（墙上 FITT 模板 4、`/STAIRC.BOT` 2），踢脚板 GENSEC 的
  JLDATU / PLDATU 基准线 10 组。

**弧墙**（2026-10-23 11:20，用户「按推荐继续修复」）：e3d-model `445d634`。

- `WALL 1 of FRMW /C1101-BASE`（`23584/6980`：165° 弧、`RADI` 1850、`JUSL OBOW`、`DESP [100, 1705]`）两端各挂一只 PJOI
  （`JOIS` `23584/6998`、`JOIE` `23584/7002`，`CREF` 都回指墙自己，`CUTP` 就是 SPINE `DRNS` / `DRNE` 翻了 Y 的向量）；
  SPINE 的 `DRNS` (0.697, 0.717, 0)、`DRNE` (−0.488, 0.873, 0) 与端点切向成 44°。此前按它们过端点切半空间，165° 的弧
  绕回来被两刀削得只剩 565 mm 一小块；E3D TTY 里两端都是径向平头，世界包围盒 (3573.04, 7001.13, 1000)–(7210, 8850, 2705)。
- 规则：挂着关节的那一端不按 SPINE 端向斜切（端部归关节管），另一端照旧。没有关节的弧墙 E3D 照 `DRNS` 斜切
  （ams1112 `/1RS-WF05-W-C-RR001` WALL 4，`DRNS = E`，V1 不变）。语料里（ams1112、ams7997、aps250110、aps7200）只有
  这一堵 WALL 挂着关节，直墙挂关节的没有样本：直墙的端切不动，PJOI 的 `CUTP` 也没有拿来切——以后碰到再取证。
- 真库门 `rvm_record_kinds_real::a_jointed_arc_wall_keeps_its_radial_ends`：世界包围盒对 E3D 面片 0.05 mm。
- 回归门 `w7o`（对 `w7n`，`gates-w7o.txt`）：V1–V5、V7、V8 0 变化，V6 498 通过、0 失败，V9 只有 aps7200 变：2331 对、
  2322 → 2323 exact、off 6 → 5、chord 3、只在 E3D 21、种类不同 9（`aps_survey_aps7200-w7o.txt`）。e3d-model 领先
  `github/main` 58 个，交付仍按 `release/0.1.43` = `aa567e4`。余下 aps7200 off 5：`/HVAC2-B1/ITEM8` 375、`BRAN /HVAC/RECT/M1/B2`
  49.5（头管，W7j）、`/INNER-KICKPLATE` 5.6 与两只 RTOR 0.87（圆弧切分，ISSUE_09）。

**圆转方风管变径**（2026-10-23 14:15，用户「按推荐继续」）：e3d-io `8a342cb`，e3d-model `80123a0` 真库门。

- `TRNS /HVAC2-B1/ITEM8`（`23584/7410`，目录 acp7002 `/FORD`：Ø400 转 800 × 300，`DESP[10]` = 200 偏心）本体是两只偏心 LSNO
  加两只 LPYR：LSNO 的偏心方向 `PBAX` 取 P7 / P8，它们的 `PTCD` 是 `AXIS -Y ( ATANT ( ±DESP[10] - (DESP[4] - DESP[5]) / 2 ,
  0 - DESP[11] ) ) ∓X`。e3d-io `catalogue_pml` 对 `ATANT` 一律拒答（注释写明「两个参数谁是纵坐标没对过 E3D」），方向回到
  模板缺省 +Y（世界 +Z），两只 Snout 往上偏 450 / 50 mm；E3D 记录里它们沿世界 −Y / +Y。按 E3D 定：`ATANT ( a , b )` =
  atan2(a, b)（度，纵坐标在前、保象限），这里是 atan2(−450, 0) / atan2(−50, 0) = −90°，把 −Y 转到 +X / −X；另一种次序给 180°、
  两只都留在 −Y。这样读时两条链对任意偏移都指向跑道形的两个半圆心 `(DESP[10] ± 250, DESP[11])`——目录作者的用意。
  单测用 P7 的程序原词（含两个参数都非零的象限样例）；真库门 `rvm_record_kinds_real::a_round_to_rectangular_transition_leans_its_snouts_the_way_e3d_does`
  钉两只 Snout 的参数、记录 X 轴与原点。
- 语料里 `ATANT` 只有这一处被求值：生成 ams8000 全库时加临时追踪，一次都没触发；ams7000 目录里带 `ATANT` 的 PTCD
  （`AXIS -X ( ATANT ( PARA[4] , PARA[2] + PARA[7] ) ) -Z` 一类）没有设计件用到。
- 回归门 `w7p`（对 `w7o`）：V1、V3–V5、V7 0 变化，V6 499 通过、0 失败，V9 只有 aps7200 变（2331 对、2323 → 2324 exact、
  off 5 → 4；`aps_survey_aps7200-w7p.txt`）；**V2 / V8 各有 1 件变差**：ams8000 `FTUB 4 of BRAN /C-OR-1R345-C`（`24384/23262`）exact → off 500，
  整只的世界标架 z 2900 → 3400。追查：它的目录（`/ACP1000-TFVL-50`，含设计表 DTSE）与整支桥架的 PTCD 都没有 `ATANT`，全库生成时
  `ATANT` 一次都没被求值；同一二进制单独、整支、全库、照门参数重跑 15 次全在 2900，此前 15 轮门（w7g–w7o）也全在 2900；重跑整套门
  `w7p2`（对 `w7o`）：V1–V5、V7–V9 0 变化（aps7200 同 `w7p`），V6 499 通过。当时判为生成器偏发；**后来查明不是**（同日 21:40，详见 §7）：
  `ams8000_0001` 当天被别的线写了会话 652–659，`w7p` 读到的会话 653 里这根 FTUB 就是在 z 3400（`--sesno` 逐会话复现：651 → 2900、652–653 → 3400、
  654 起 → 2900），生成器在同一会话上重跑 91 次全一致。
- e3d-io 现领先 `github/main` 4 个（交付仍按 `3bed2ba`），e3d-model 59 个（交付仍按 `release/0.1.43` = `aa567e4`）。余下 aps7200 off 4：
  `BRAN /HVAC/RECT/M1/B2` 头管 49.5（W7j）、`/INNER-KICKPLATE` 5.6 与两只 RTOR 0.87（圆弧切分，ISSUE_09）；只在 E3D 21 = POINTR 20 + 零尺寸 CYLI 1。

**零尺寸 CYLI**（2026-10-23 16:55，用户「继续修复」）：e3d-model `b758b28`。

- `CYLINDER 3 of SUBSTRUCTURE 1 of STRUCTURE /EQRACK.5M.HR5`（`23584/9172`，`DIAM = HEIG = 0`、`POS` / `ORI` 全 0、`LEVE [0,10]`）：
  Core 不出体、我们按尺寸退化跳过（`skipped`），E3D 的 RVM 驱动却照走 DRCYL，TTY 里是一条 `Cylinder[0, 0]`，矩阵是元素的
  世界标架（属主绕 Z 转了 180°：原点 (7698.5, 10140, 5880)，X = −X、Y = −Y）。
- 做法：不让它进模型集（gen-model 的烘焙要实体，空体会把整根标成失败），而是与 SNODE 点标记同款的只写 RVM 通道
  `GenerationOutcome::bodyless`（`elmodl::bodyless_design_record`）：按 `NoShape` 跳过的设计 CYLI，过了表现级门、两个尺寸至少
  一个 ≤ 0 的，`gen_ams` 照 E3D 写一条 `Cylinder[DIAM/2, HEIG]`；账仍记在 `skipped`，`elements.json` 与增量都不动。只认 CYLI：
  别的设计基本体零尺寸时 E3D 写不写，语料里没有样本。真库门 `rvm_record_kinds_real::a_zero_size_design_cylinder_still_writes_its_empty_record`。
- 回归门 `w7q`（对 `w7p2`，`gates-w7q.txt`）：V1–V5、V7、V8 0 变化，V6 500 通过、0 失败，V9 只有 aps7200 变：2331 → 2332 对、
  2324 → 2325 exact、只在 E3D 21 → 20（`aps_survey_aps7200-w7q.txt`）。e3d-model 领先 `github/main` 60 个。余下 aps7200：off 4
  （头管 49.5 → W7j；`/INNER-KICKPLATE` 5.6 与两只 RTOR 0.87 → ISSUE_09）、只在 E3D 20 = POINTR（先定口径）。

**PIPE 下的矩形风管支管**（2026-10-23 17:40，用户「继续下一步」）：e3d-model `22c5e71`。

- `BRAN /HVAC/RECT/M1/B2`（`23584/7312`）挂在 **PIPE** `/HVAC/RECT/M1` 下，不是 HVAC；`OLET 23584/7318 → ELBO 23584/7319` 那段
  隐式管身的 `LSTU = /RDUCT/XYDUCT`，SCOM `/XYDUCT`（GTYP TUBE）`PARA = [1, 1, …]`，断面写在 GMSE 的 BOXING 里：`PXLE = [40,1,-1000,0]`、
  `PZLE = [40,1,-500,0]`，负操作数是十分之一常数 → 100 × 50。E3D 照矩形画（TTY 容器组里 `Box[50, 100, 2150]`）；我们按属主 noun
  判成管道学科，读 `PARA[2]` = 1 当外径，画成 1 mm 的圆管（off 49.5，种类不同）。不是 W7j 那种头管 UDA。
- 做法（`pipeline.rs`）：管道学科的槽位先按风管那条路（`hvac_tube::duct_departure_section`）问一遍，解得出**矩形**断面就用它；
  圆的、解不出的、读挂的照旧走 `PARA[2]` / 到达侧通径，一个字不变。记录标架按离开侧构件的滚转写（`Box[100, 50, 2150]`），与 E3D
  按世界轴写的是同一只盒子（世界包围盒逐位相同）。真库门 `rvm_record_kinds_real::a_boxed_tube_catalogue_under_a_pipe_still_draws_the_rectangle`。
- 回归门 `w7r`（对 `w7q`，`gates-w7r.txt`）：V1–V5、V7、V8 0 变化，V6 501 通过、0 失败，V9 只有 aps7200 变：2332 对、2325 → 2326 exact、
  off 4 → 3、种类不同 9 → 8（`aps_survey_aps7200-w7r.txt`）。e3d-model 领先 `github/main` 61 个。余下 aps7200：off 3（`/INNER-KICKPLATE` 5.6
  与两只 RTOR 0.87，圆弧切分 → ISSUE_09）、只在 E3D 20 = POINTR。
- W7j 的头管 UDA（aps250142 `/RETURN_LEVEL02-002/01`，`/HCTUBE` 的 `PDIA = ( :0x17EF4B61[1] )`）核过：E3D 画的头管 Ø450 = `HBOR`，我们头槽位
  `unbuildable`（`gen_ams` 没挂 UDA 字典，`PreviousElementEnv` 也不在宿主 BRAN 上读键）。**另一条线已在做**：工作树 `vendor/e3d-model-uda`、
  分支 `w7j-uda`（基于 `b758b28`，`5e1649e` gen_ams 挂 UDA 字典、`e11d359` 头槽位在宿主 BRAN 上取 `:HXYsize`），这边不重做；并回 main 时
  `tests/rvm_record_kinds_real.rs` 和 `changelog.md` 与 `22c5e71` 会有两处小冲突（都是尾部 / 顶部追加）。

**桥架路径点记号**（2026-10-23 21:30，用户「继续下一步」；口径问答卡 30 分钟没人答，按推荐项做）：e3d-model `fb72d65`。

- E3D 给 `CWBRAN` 的 RPATH 每个 `POINTR` 写一只 10 × 10 × 10、沿世界轴、以点为中心的 `Box`（`CAB_CSGRoutePoint::getCSGTree`）：
  aps7200 zone `=23584/10278` 的 8 条空分支共 20 点，TTY 导出全是这样（那个 zone 里只有这 20 只记号和一组只有线的 `/RN001`）。
  我们把 POINTR 当路径数据（`ProfileData`，被 CWBRAN 消费），一条不写。
- 口径（推荐项，卡超时后按最稳妥的做）：照 E3D 写记号、**只进 RVM**——走 `GenerationOutcome::bodyless`（`rpath::route_point_marks`，新增
  `DegenerateCatalogueShape::Box` 与 `rvm_write::bodyless_box_geometry`），模型集、`elements.json`、增量和界面都不动；不认同的话
  `git revert fb72d65` 即回。条件按「有 RPATH 就画」：全部 TTY 导出（aps7200 25 个 zone、APS 10 本 36 件、ams8000 10 件）里 CWBRAN 只出现在
  这个 zone（ams8000 的桥架是 BRAN + FTUB），挂着托盘件的 CWBRAN E3D 画不画记号没有样本，碰到反例再收窄；CURVE / EXTGEO 成员跳过。
  生产侧（gen-model）不写 RVM，不受影响。真库门 `rvm_record_kinds_real::empty_cableway_branches_mark_their_route_points_like_e3d`。
- 回归门 `w7s`（对 `w7r`，`gates-w7s.txt`）：V1–V5、V7、V8 0 变化，V6 502 通过、0 失败，V9 只有 aps7200 变：2332 → 2352 对、
  2326 → 2346 exact、只在 E3D 20 → 0（`aps_survey_aps7200-w7s.txt`）。e3d-model 领先 `github/main` 62 个。
- **aps7200 W7k 收口**：2352 对、2346 exact、chord 3、off 3、只在 E3D 0、只在我们 0、种类不同 8。剩下的 off 3（`/INNER-KICKPLATE` 5.6、
  两只 RTOR 0.87）与种类不同 8 全是圆弧切分 / 记录写法，归 ISSUE_09 不改；W7j 的头管 UDA 在另一条线（`w7j-uda`）。

### M8 · W7j 头管 UDA（2026-10-23，e3d-model 分支 `w7j-uda`；2026-10-24 已并回 main）

用户「继续修复」时另一个会话正在主工作区改 `pipeline.rs`（aps7200 `BRAN /HVAC/RECT/M1/B2`）。为不撞车，这一项在 e3d-model 的独立 worktree
`vendor\e3d-model-uda`（分支 `w7j-uda`，从 `b758b28` 拉）做，独立 target `target-e3d-model-uda`；回归门用脚本的临时副本指向它（用完已删）。

- `/HVAC/RECT/M1/B2` 不是 UDA 头管：另一会话已查明是 PIPE 属主下 `OLET → ELBO` 槽位，`LSTU = /RDUCT/XYDUCT`，断面写在 BOXING
  里（100 × 50），我们按 `PARA[2] = 1` 读成 1 mm 圆管。W7k 根因表「头管 1」那行的「疑同 W7j」不成立，UDA 头管只有 aps250142
  `/RETURN_LEVEL02-002/01` 一条。
- 根因两处：① `gen_ams` 从不挂 UDA 字典，目录表达式里按键读 UDA 的一类在对拍里永远求不出值，而 gen-model 生产一直挂着
  （`UdaCatalogProvider`）；② 风管容器头 / 尾槽位的求值环境只挂目录件 SCOM，`( :HXYsize[n] )` 读的是 BRAN 上的 UDA，挂了字典也问不到
  ——生产同样画不出这根头管。
- e3d-model `5e1649e`：`DirectoryDbResolver::uda_catalog`（给定 DICT 库号逐本读 `UdaCatalog`、按给出的顺序合并），`gen_ams` 用它 UDET
  那份 DICT 库号挂字典（APS 一套 5184 条定义）。e3d-model `e11d359`：`hvac_tube::PreviousElementEnv` 加 `uda_host`（容器头 / 尾 = 宿主
  BRAN），按目录写下的键 `0x17EF4B61`（acp7006 `/UDA/HVAC/HXYsize` 的 `OLDKEY`）在 BRAN 上精确取（`DbElement::uda_by_key`），不走
  同名回退；BRAN 的字典不认这把键才交给目录件。ams7997 的 BRAN 把值存在 ams5100 那把键 `0x2652AB52` 下：同名回退拿得到
  675 × 600，精确取拿不到，头槽位仍 NoRadius——与 E3D 在那里求值失败一致。矩形 H 族头槽位的断面现在读得出，但 BRAN 没有
  `LEAV`、滚转没有样本：第一版在 `leave_frame_z_axis` 报错，打断了 aps250142、aps7200 的整库生成，改为按解不出记账（与改动前同
  结果）。语料里 E3D 画出来的 H 族头管只有圆的这一根。
- 真库门 `hvac_tube::a_head_slot_reads_the_branch_uda_by_the_catalogue_key_only`：aps250142 头槽位解出 `Round { 450 }`；ams7997
  BRAN 上同名回退给 675、精确取不给正数。
- 回归门 `uda3`（对 `w7q`，`gates-uda3.txt`）：V1–V5、V7、V8 0 变化；V9 只有 aps250142 变：exact 519 → 520、off 2 → 1、种类不同
  2 → 1（`/RETURN_LEVEL02-002/01` 那只容器 exact；余 off 1 是扁圆风管）；aps7200 与 `w7q` 同（2332 / 2325 / off 4 / 只在 E3D 20）。
  45 份生成里除 aps250142 与 ams7997（布尔剖分逐次不同）外逐字节同 `w7q`——挂字典没让别的库改值。V6 首轮 498 通过、1 失败：
  `attribute_reads_guard` 认出库代码里新写的字面量 DICT；改成由调用方传库号（与 `UdetCatalog::scan` 同一种签名）后重跑 `uda4`：
  499 通过、0 失败（比 `w7q` 少的两条是主工作区未入库的 `relocation_noise_real`，多的一条是上面的真库门），aps250142 / aps7200 重新生成
  与 `uda3` 逐字节相同。
- 读取集合不用动：生成器按键读的是 UDA，不进属性名表；gen-model 的 `model_impact` 对 UDA 改动一律保守重生成
  （`children_and_uda_changes_conservatively_regenerate`）。
- **已并回 main**（2026-10-24，merge `e9d6cc5`，自动合并无冲突；两处改动不碰 `pipeline.rs`）。主工作区回归门 `w7t9`（对 `w7t8`，
  `gates-w7t9.txt`）：只有 aps250142 变：exact 519 → 520、off 2 → 1、种类不同 2 → 1，与独立 worktree 上的 `uda3` 结论一致；其余
  V1–V9 0 变化。不在已交付的 0.1.43 里。

### M8 · W7j 扁圆风管（2026-10-24，完成）

用户「按推荐继续」。e3d-model `b899445`，先取证 E3D 的写法再改：

- 取证：`LSTU /AVEVAHVACSPEC/FOTUBEB` → `/FOTUBE2` 的 GMSE 可见五只：`-BOXI`（`PXLE = LEAWID − LEAHEI`、`PZLE = LEAHEI`）、
  `-TUB1` / `-TUB2`（`PDIA = LEAHEI`，轴是 PTSE 的 P4 / P5，`PX = ±(LEAWID − LEAHEI)/2`）、`-INS1` / `-INS2`（`OBST 1` 但 `TUFL` 真、
  `LEVE [0,10]`，`REPRE TUBE ON` 下 E3D 照画；`PDIA = LEAHEI + IPAR[1]`，保温 0 时与 TUB1 / TUB2 重合）。`/SUPPLY_LEVEL02-001/02`
  STIF → DAMP 那 4.41 mm 的槽位（LEAWID 500 / LEAHEI 100）E3D 写 `Box[100, 400, 4.41]` + 四条 `Cylinder[50, 4.41]`（±200，两对
  字节一样）；我们此前只认 GMSE 里**第一只**可见图元，按 TUB1 写一条居中的 `Cylinder[50, 4.41]`（off 200）。
- 改法：`hvac_tube::duct_departure_section` 收集全部可见的 BOXING / TUBE（仍是 `display_filtered` 那一道）：一只照旧；多只交新的
  `TubeSection::Composite`——BOXING 居中，TUBE 按 `PAXI = Pn` 取该 P 点的 `PX` / `PZ`（同一套 `OF PREV` 环境求，求不出的分量为 0）
  作断面内偏移，同形同位的成员并成一块记份数；`route::build_tube` 逐块按真尺寸建在滑转后的管身局部系里（滑转与矩形断面同一格
  `z_axis`），`parts` / `part_nouns`（`BOXI` / `TUBE`）/ `part_records`（份数 + 偏移标架）交给隐式管身的 `GeneratedElement`，`gen_ams`
  照目录 noun 写 `Box` / `Cylinder`。`TubeSection` 不再 `Copy`。目录局部 X（`PXLE` 方向）与管身局部 X 的正向是否一致没有样本：
  语料里的偏移成对对称，符号不影响结果。读取集合不用动（没有新属性名）。
- 门：单元 `route::a_flat_oval_duct_body_builds_its_box_and_tubes_as_parts`；真库门
  `rvm_record_kinds_real::a_flat_oval_duct_body_writes_its_box_and_tubes_like_e3d`（五条记录、世界盒对 E3D 0.05 mm、模型集一件三块）。
- 回归门 `w7u1`（对 `w7t9`，`gates-w7u1.txt`）：V1–V5、V7、V8 0 变化；V6 515 → 517 通过、0 失败；V9 只有 aps250142 变：521 对
  520 → 521 exact、off 1 → 0、种类不同 1 → 0；aps7200（2352 / 2348 / off 1）、aps250209（4623 / 4622 / off 1）、aps250212（6 / 6）
  与 `w7t9` 同。至此 W7j 只剩 ams8000 `/M-OR-1RX-3081D` 托架那一条。

### M8 · W7j ams8000 托架（2026-10-24，完成；W7j 收口）

e3d-model `d0fe6d1`，gen-model `988cb525b`（读取集合 + `mbd/branch_query.rs` 提示文案）。W7i 留下的「托架支撑 ANCI 的 DATUM / SREF 成对、
正反向槽位的规则」不是几何规则，是 ATTA 自己的一个布尔属性：

- 取证：`/M-OR-1RX-3081D`（24384/2860）E3D 写三根隐式管（`/1RX03-MSUP-E0015/SREF` → BEND 820、两只弯头间 4.17 / 4.18），我们多写
  DATUM → SREF 240（E0017）、FTUB → DATUM 140（E0018）。先排除形态：ams8000 全库 123 条建成槽位里 95 条两端挖着 ATTA、除这两条外全与
  E3D 一致（`output/w7d/bran-audit-8000-all`），同形的槽位（3081B 的 FTUB → DATUM 110、3081C 的 DATUM → BEND 100、3041D 的 DATUM → SREF 670）
  E3D 都画；LSTU（E0017 / E0018 的是 `/ACP1000-Ladder/ACP1000-TUBE:500`、与支管的 Trough 不同，但 3081B / 3051C 等同样混用照画；
  `output/w7d/lstu_census_ams8000.txt`）、PCON（CRS / ATT / BOXI）、目录（`/RACK-REST-500`、`/SPECIAL.500`，与 2944 同族）逐一排除。
  逐属性对出来的唯一差别：E0017、E0018 两对 ATTA **没存 `SPKBRK`**，ams8000 其余 233 只梯架支撑 ATTA 全真（另两只没存的在
  `/M-OR-1RX-3AF0A`，槽位全零长；`output/w7d/spkbrk_census_ams8000.txt`）；E3D 自己记在 `SPAMAP` 第二个字里的隐式管空间键也恰好只在
  画了管的成员上有值。旧 fast_model（`cata_model.rs`）就是按 `SPKBRK` 跳过不断管的 ATTA / STIF / BRCO。
- 旁证（只读 TTY，`verification/civil-tty-20261021/spkbrk_lv6.mac`，`L3_E3D_INSTALL_DIR = E:\reverse\e3d\shadow_e3d31_gen_model_test`；缺省的
  `e3d31-runtime` 起不来，0xC0020001）：ams7997 `/Copy-of-1RCS0620-1R80025` 的 `/Copy-of-NP-R322003-DAB`（`/AVEVA-ATTA-SPEC`，`GTYP=ATTA`，
  此前一律穿过）是 ams7997 1849 只 `GTYP=ATTA` 里唯一 `SPKBRK` 真的，E3D 把 WELD 5 → WELD 6 那根在它那里切成 124.0 + 300.0
  （`spkbrk_lv6_24381_145993.rvm`）；ams8000 HVAC `/-CSV-S-1-H-7801` 的 `/NX-XVNA-S8002` 那对（`GTYP=ANCI`，没存）E3D 不断
  （`spkbrk_lv6_24384_24935.rvm`）。管道支撑附着点（ams8000 43 + 2、ams7997 1848、aps7200 60）与风管链上的 MDS 支撑（ams7997 572、
  ams8000 2）全没存——「GTYP=ATTA 穿过」「风管 ATTA 一律穿过」两条经验判据只是和它相关。STIF / BRCO / SPLR 的模板里没有 `SPKBRK`。
- 改法：`route::station_verdict` 第三参改为 ATTA 的 `SPKBRK`（没存按布尔缺省假；模板里没有 → `PassThroughUnresolved`，照旧单列），
  `member_station_verdict` 不再走 `SPRE → SCOM.GTYP`、风管不再特判 ATTA；SPLR 照旧穿过。读取集合 168 → 169。普查探针入库
  `examples/atta_spkbrk_census.rs`。
- 门：单元 `station_verdict_follows_spkbrk_for_atta`、`an_atta_on_a_duct_route_follows_the_same_spkbrk_rule`；真库门
  `rvm_record_kinds_real::an_atta_without_spkbrk_does_not_end_a_tube`。回归门 `w7u2`（对 `w7u1`，`gates-w7u2.txt`）：V1–V5、V7、V9
  0 变化；V6 517 → 518 通过、0 失败；V8 ams8000 种类不同 29 → 28，off 那 1 件是 `BOX 1 /1-LNR-Q005-PJ` 500 mm——又是另一条线的测试当时
  在改活库（与 w7t8 同一件），重跑 `w7u2b`：2614 对 2614 exact、off 0、种类不同 28（余 28 是整圈 SREV 的写法，ISSUE_09）；
  aps7200（2352 / 2348 / off 1）、aps250209（4623 / 4622 / off 1）、aps250212（6 / 6）与 `w7u1` 同。

至此 W7i / W7j 的 5 条支管全部对上；建议顺序里剩下的「只差条数的」是整圈 SREV / REVO 的记录写法（ISSUE_09 既有类）和圆弧切分，
按计划不改。各库现状：ams8000 2614 / 2614；APS 10 本 8591 对里 off 只剩 aps250110 那只 1:620 放坡直梁的 0.54 mm（W7f 归因）；
  aps7200 off 1、aps250209 off 1 都是圆弧切分 / 整圈 REVO；只在 E3D 全部归零。

### M8 · ISSUE_09 记录写法（2026-10-25，用户「继续」：W7t / W7j 收口后对拍表里剩下的那一类）

计划原把这一类记为低优先级（D4）；用户 10-24 要求「直到所有的模型问题都修复完成」、次日「继续」，按影响面从它里挑有 E3D 写法可取证的做。
每步先拿 TTY 导出的面片坐实 E3D 的规则，再改，再过全部回归门（跳回归脚本前先显式 `cargo build --release --bin gen_ams` 进
`target-e3d-model`：`CARGO_TARGET_DIR` 环境变量指着共享池，`run_gates.ps1` 自己只在 V6 才重编，`w7v1` 那轮的 V1–V5 / V8 就是拿旧
二进制跑的，作废）：

1. **扫角超过 180° 的回转图元拆两段写两条**（e3d-model `38db801`）。libgm `gm_CreateRevolution` 一次只扫 ≤ 180°，超过的拆成两段
   各扫 ANGL/2、**各按自己的扫角定段数**，`gm_CreateCombination(1)` 并起来，RVM 驱动写两条面片（矩阵相同）。取证（`output/w7d/rev_split_probe.py`，两边面片的角度格 / 封口面；变截面的逐多边形 dump 是 `taper_facet_dump.py`）：aps250209 `/AREA07-CV_0001/Belt` REVO 2（ANGL 190）E3D 95° + 95° 各 5 步（R 409.75 按 10 mm 口径整圈 16 段），我们整段
   一次算 9 步 21.1°——这就是 W7t 收口时剩的 off 1.18；`/AREA05-CV_0001/Belt` REVO 4（210°）105° + 105°；同库 180° 整的一条；ams8000
   28 只接线盒的目录 SREV（PANG −360）180° + 180° 各 4 步、E3D 先写朝 +Z 的那半；aps7200 `REVO 1 / 2 of TMPL 1 of FITT 1 of STWA 3`、
   aps250110 10 只整圈设计 REVO 同款。改法：`solid::revolution_is_split` / `revolve_rings_primitive`（整体按两段之和铺格）/
   `revolve_rings_profile_faceted_parts`（两块记录单位）；设计 REVO 没有负体时 `elmodl` 交两块 `parts`，目录 SREV 没有刀时两块各一个网格键；
   `gen_ams` 的 `e3d_exports_facets_only` 加 `Category::Revolution`。**只给回转图元**：第一轮（`gates-w7v1`）把两段的格透进了型材弧扫掠那条路，
   aps250110 / aps250112 的 18 只钢筋箍 GENSEC（整圈弧 SPINE）偏 5.7–6.2 mm，收回到 `revolve_rings_primitive` 后全部 exact。
   真库门 `a_revolution_over_180_degrees_writes_two_facet_groups_like_e3d`。回归门 `w7v2`（对 `w7u2`）：V1–V5、V7 0 变化，V6 521 通过；
   V8 ams8000 种类不同 28 → 0（2614 / 2614）；V9 aps250110 种类不同 10 → 0；aps250209 4623 / 4623 exact、off 0、种类不同 0；aps7200 种类不同 8 → 6。
2. **多面体逐连通体一条**（e3d-model `5443ff6`）。aps7200 `POHE 1 / 2 of SUBS 1 of /STAIRC.TOP` 各 8 块互不相连的薄板，E3D 各 8 条；我们按
   拉伸体「一只元素一条」合成一条。`gen_ams` 给 `Category::Polyhedron` 开例外；真库门 `a_polyhedron_writes_one_facet_group_per_body_like_e3d`。
   aps7200 种类不同 6 → 4（同一轮 `w7v2`）。
3. **变截面型材按两端截面放样**（e3d-model `947b954`）。aps7200 `/TAPER-SCTN1..4`的 SPRF `/OD-CONE`（`GTYP VTUB`）两个 SANN 半环带
   `DRAD = 0.5·DESP[2] − 0.5·DESP[1]`（−500）、`DWID` / `DX` / `DY`：E3D 两端各求一张截面放样（外径 1000 → 500）、弧按 10 mm 口径起点半径定段
   （外 24 / 内 20 段）、整只一条面片，面片体积 1.625157e9 mm³；我们不读这四个属性，挤成 2.26e9 的直管、两条记录——包围盒恰好相同，
   对拍一直 exact，只在种类不同里露头（这是一个被 AABB 尺子遮住的真几何错误）。新 `solid::loft_rings`，`section::sann_rings` 读四个增量、
   成员带 `taper`，`section_local_solid` 放样，`SectionSolid::tapered` → 不逐成员交 `parts`。读取集合 169 → 171（gen-model 一起重生成，
   `model_impact` 的钉数 165 → 173：`86c0cf9c5` / `988cb525b` 当时漏改）。真库门 `a_tapered_section_lofts_between_its_end_sections_in_one_facet_group`。
   回归门 `w7v3`（对 `w7v2`）：V1–V5、V7–V9 0 变化，V6 524 通过，aps7200 种类不同 4 → 0（2352 对 2348 exact、chord 3、off 1）。
   三步之后的现状：ams8000 2614 / 2614、种类不同 0；aps250209 4623 / 4623；aps250212 6 / 6；aps7200 余 chord 3（PCLA 0.42）+ off 1（`/INNER-KICKPLATE`）；
   aps250110 余 off 1（W7f）；aps250112 余种类不同 10（带 FRAD 的 RAIL 逐段写）+ chord 4；aps250104 chord 21、aps250162 chord 5。
4. **带 FRAD 圆角的详图式 RAIL 逐节写**（e3d-model `da5aeb7`）。aps250112 `RAIL 1 / 2 of RAILSET 1 of /EL(+)104880_TOS_PLATE/HR_003`（3 直段 + 2 弧）E3D 各 7 条：
   直段一条（整张截面一次挤出，两个 SANN 半环在同一条里）、弧每个 GMSS 成员一条（各走一次回转）；16 / 13 / 10 条的那几根同款。我们整根一条
   （W7a 时记为不改）。`handrail::detailed_item_local_solid` 本来就是逐节建再 compose：弧改成逐成员各扫一块，`RailBuild::Solid` 把逐节的块交出，
   `elmodl` 对 RAIL（不含 KICKPL，没有样本）没有负体时交 `parts`。只有直段的折线 E3D 仍是一条（aps250110 的 3 / 5 / 6 节栏杆、aps250112 102470
   RAILSET 1 那根带一只 0.085 mm 退化圆角的），带 `CURTYP THRU` 弧的也是一条（aps250110 `/BUILDING_01_LEVEL_03/HR_001`）4 直段 + 1 段 R≈14 m 的 THRU 弧）：
   第一版把它们也拆了（`gates-w7v4` aps250110 种类不同 0 → 8、aps250112 剩 2；`w7v6` 还剩 THRU 那 2），改成只在路径带弧长 ≥ 1 mm 的 FRAD 圆角弧
   时才逐节写。真库门 `a_detailed_rail_with_fillets_writes_one_facet_group_per_piece_like_e3d`（7 / 7 / 1 / 13 / 1 条）。回归门 `w7v7`（对 `w7v3`）：
   只有 aps250112 种类不同 10 → 0 这一处变，V6 525 通过。

四步之后对拍表的现状（`gates-w7v7.txt`）：ams8000 2614 / 2614、种类不同 0；aps250209 4623 / 4623；aps250212 6 / 6；aps7200 2352 对 2348 exact、
种类不同 0、余 chord 3（PCLA 0.42）+ off 1（`/INNER-KICKPLATE` 5.6）；aps250110 余 off 1（W7f 0.54）；aps250112 774 对 770 exact、chord 4、种类不同 0；
其余 APS 8 本、MDS 三套框、土建、S1 都是旧样。种类不同全库归零。
   评论草稿 `output/github-issues-20261021/16-e3d-model-7-comment.md`（`issues.json` 键 `COMMENT_E3D7`）。**2026-10-25 17:58 已推、已发**：用户「按推荐继续」、卡上明确同意由我推（D7 (b) 的口径，仅这一次）：fetch 核过远端仍是 `d0fe6d1` 后快进推 e3d-model `main` → `da5aeb7`（4 个），发布分支 `ci/gen-model-linux-20260918` 仍是 `b758b28`、线上不受影响，这四步随下一版交付；`submit.ps1 -Only COMMENT_E3D7` 发出 https://github.com/happyrust/e3d-model/issues/7#issuecomment-6396231429（发前修掉草稿里表格中间那个空行）。剩余项（下段）用户选「到此收口」。

未动的（仍归 ISSUE_09）：aps7200 `/INNER-KICKPLATE` off 5.6——不是弧切分密度，是 CENT 端点落在圆内那一形的**弧终角**：E3D 的弧到 358.93°
（11 步 14.975°）就改直段连到终点，我们的弧一直走到终点方向 360.68°（12 步 13.872°），最低点的顶点落在不同的格上；弧在哪里收尾还没有
第二个样本。aps7200 3 只 PCLA chord 0.42、aps250104 21 件 chord、aps250162 5 件 GENPRI chord（弦高级，验收口径内）、
整圈 TUBE 滚角与 Pyramid 等价写法（记录矩阵 / 参数，对拍表看不见）。

### M7 · 交付准备（2026-10-22，待你推送）

用户「先交付：推送和部署」；D12 / D13 回复「全按推荐：merge + 升 0.1.43」。本地已做（都未推送）：

| 仓 | 提交 | 内容 |
|---|---|---|
| gen-model `refactor` | `c16b99c6b` | `Cargo.lock` 随 vendor：e3d-io 0.4.0 → 0.5.0、e3d-model 新依赖 `spade`（`windows-binary.yml` 用 `--locked`） |
| gen-model `refactor` | `b0477ffd7` | merge `origin/refactor`（0.1.42 的 3 个提交：中心线逐段外径来源、中心线读取会话、发布），只共改 `handlers.rs`，自动合并 |
| gen-model `refactor` | `7fdc4c15c` | `chore(release): 0.1.43`：`Cargo.toml` / `Cargo.lock` 版本、`changelog.md` 一段、`python/aios_client.py` 的期望版本 |

推送前核对（fetch 后）：e3d-model `main` 领先 `github/main` 39、e3d-io 1，两边的发布分支（`f2ecfe9` / `5245654`）都是新 main 的祖先；
e3d-attlib、plant-mbd、old-aios-core 的发布分支已等于本地提交；plant-ui `master` 有 3 个本地提交，不在这次交付里。gen-model
`refactor` 领先 21、不落后；`origin/publish` = `origin/refactor` = `220a6ec87`（0.1.42，CI run 39726501255 成功，约 12 分钟）。

干净检出验证（D11；`D:\work\plant-code\deliver` 下的 git worktree，按将推送的提交检出 gen-model、五个 vendor 与 plant-ui
发布分支，`vendor/shared` 照 CI 指到同级 vendor）：

- e3d-model 全量测试（`cargo-test-deliver.txt`）：472 通过、0 失败、16 忽略；比工作区少的 4 条是未入库的 `relocation_noise_real`。
- 干净 `gen_ams` 生成 aps250110：`generated.rvm` 与 `w7c` 逐字节相同，对拍同为 3427 对 3425 exact（`aps250110_lv6-deliver.txt`）。
  工作区未提交的 e3d-model `element_diff.rs` 只在增量规划 / 账本里用，e3d-attlib 的 `dict` 模块没有生成代码调用，生成结果
  不受影响，`w7c` 那轮 V1–V9 的结论对干净代码同样成立。
- gen-model 服务端二进制按 CI 同款 `--bin aios-database --no-default-features --features ws,gen_model,manifold,project_hd,http_api`
  `cargo check`：合并后的树通过（`deliver-gen-model-check.txt`），`7fdc4c15c` 加 `--locked` 再过一次
  （`deliver-gen-model-check-locked.txt`），锁文件不变。检查在 Windows 上做，Linux 专有的编译路径以 CI 为准。

推送顺序见 W6 第 2–5 步（一律按提交号）。部署后：从 CI 日志与 `/api/v1/health` 核版本 `0.1.43+g7fdc4c15c…`、从 CI 日志核 vendor 短哈希（W6 第 5 步），再做第 6 步的重建与抽查，
最后跑 `submit.ps1`。部署核完后用 `git worktree remove` 逐个删掉干净检出（gen-model 那份登记在 `.git/worktrees/gen-model`），不要直接删目录。

### M7 · 补交付（D18，2026-10-23，待你推送）

用户「结合 plannotator 分析进度」时只读核查发现（`git ls-remote`、reflog、`gh run view 39803133375 --log`）：10-22 23:13 净距那条线把 `fix/nearby-branches` 挑到当时的本地 `refactor`（`635bf7a90`）上得 `d3145d26a`，快进推了 `refactor` 与 `publish`；CI run 39803133375 构建部署成功，「Checkout vendor path dependencies」打印 `e3d-model -> f2ecfe9`、`e3d-io -> 5245654`、`old-aios-core -> a438029`（e3d-attlib `f37be2e`、plant-mbd `f888a50`、plant-ui `490da62`），编译时 `Downgrading e3d-io v0.5.0 -> v0.3.0`，新二进制自报 `0.1.43+gd3145d26a71a.1792683148.dirty`。即线上是 0.1.43 的 gen-model 配 09-22 的 vendor：本计划与 SCTN 方案的生成修复（e3d-model 96 个、e3d-io 149 个提交）都不在线上，D17 的两处（坏 `.mesh` 原地修复、匿名属性进增量判定）在线上不成立。用户选「按 A 补交付」（以远端现状为基线、先补验再推），补验后又选 D18 (c)。

干净检出沿用 `D:\work\plant-code\deliver`（D11 那套 worktree，只换 HEAD）。三组：

| 标签 | 组合 | gen-model `cargo test --lib` | 44 份生成（与 D14 同参数） |
|---|---|---|---|
| `a43` | gen-model `d3145d26a`；e3d-model `aa567e4`、e3d-io `3bed2ba`、old-aios-core `c9cc8bb` | 2586 通过 / **2 失败** / 133 忽略 | 43 份与 `d14`、`w7c` 逐字节相同；ams7997 RVM ↔ RVM 53 697 件全 exact、种类全同（`compare_ams7997-d14-vs-a43.json`） |
| `a43b` | gen-model `2639eff43`（= `7fdc4c15c` + `d3145d26a` 的 cherry-pick，不挂分支、未推）；vendor 同上 | 2588 / 0 / 133 | 同上（生成只看 vendor） |
| `a43w` | gen-model `d3145d26a`；e3d-model `df2aa7d`（本地分支 `release/0.1.43-w7e` = `845f4e5` + `65210cc` 的 cherry-pick，`element_diff.rs` 与 `aa567e4` 逐字节相同）、e3d-io `3bed2ba`、old-aios-core `c9cc8bb` | **2588 / 0 / 133** | 43 份与 `w7e` 逐字节相同；ams7997 RVM ↔ RVM 53 697 件全 exact、种类全同（`compare_ams7997-w7e-vs-a43w.json`）；与 `d14` 不同的是 ams7999 与 aps250110 / 104 / 112 / 124 / 132 / 142 / 160，正是 W7a–W7e 修过的库 |

- `a43` 的 2 条红：`model_impact::...the_generation_read_set_mirrors_e3d_model...`（gen-model 读取集合 161 项 = 发布版 157 项 + EZDI、PTS、SLODIR、SZDI，即 `9216e2f72` / `c6cd27a85` / `635bf7a90` 对 W7a / W7i / W7e 的同步）与 `model_update_plan::...only_units_with_implicit_tubing_regenerate_on_a_member_move`（`749681d8f` 把期望改成跟 `1f02613`；判定本身运行时调 `e3d_model::category::is_derived_unit`）。线上安全，但不是全绿；按 D18 改配 W7e 后全绿。
- 三组都另跑了 CI 同款 feature 的 `cargo check --locked`（`deliver-*-check-locked.txt`），通过、锁文件不变：换上发布分支后 CI 不会再降级 e3d-io，版本串不再带 `.dirty`。
- V5 没有重跑：生成逐字节与 `w7e` 一致，`w7e` 那轮门的 V5 是 0 变化，`df2aa7d` 与 `845f4e5` 只差 `element_diff.rs`（只在增量规划与账本里用）。
- 推送顺序见 W6「2026-10-23 起以这一段为准」。部署核完后用 `git worktree remove` 逐个删 `deliver` 下的检出；`2639eff43` 没被采用，不挂分支。`7fdc4c15c` 里 0.1.43 changelog 写的 vendor「→ `230d20b`」与「old-aios-core 不变」都不再准确（实际是 `df2aa7d` 与 `c9cc8bb`），不改已推的提交，随下一版修。
- 产物：`verification/regress-20261021/deliver-a43*`、`deliver-w7e*`、`gen-*-a43` / `gen-*-a43w`（土建在 `civil-tty-20261021`）。临时驱动 `output/tmp_a43_gen.ps1` 与 `output/tmp_*.cmd` 用完已删。

**续（2026-10-23 18:55，`publish` 又被推到 `384e67151`）**：16:54 净距线把 `publish` 推到 `384e67151`（CI run 39928011740，vendor 照旧是旧的，又一次混版本，编译时同样 `Downgrading e3d-io v0.5.0 -> v0.3.0`）。这一版的读取集合（e3d_model 163 项）与测试期望已同步到 e3d-model `28d0402`，与 main `b758b28` 的 `attribute-read-set.json` 逐项相同。干净检出换成 gen-model `384e67151`、e3d-model `b758b28`、e3d-io `8a342cb`、old-aios-core `c9cc8bb`（e3d-attlib / plant-mbd / plant-ui 与 CI 同）：`cargo metadata --locked` 与 CI 同款 `cargo check --locked` 通过；`cargo test --lib` **2596 通过 / 0 失败** / 133 忽略；45 份生成（标签 `a43q`，`deliver-a43q-gen-summary.txt`，含 aps7200）44 份与 `w7q` 逐字节相同，ams7997 RVM ↔ RVM 53 697 件全 exact、种类全同（`compare_ams7997-w7q-vs-a43q.json`）。推送顺序见 W6「2026-10-23 18:55 起以这一段为准」。`release/0.1.43-w7e`（`df2aa7d`）不再用，本地分支留着。

**续（2026-10-24，用户「按推荐继续」→ 选「交付」）**：D18 预检没过——`publish` 10-24 03:14 已被推到 `cb0fbd8a5`（CI run 40009803990 部署成功，vendor 仍是 `f2ecfe9` / `5245654` / `a438029`，`Downgrading e3d-io`、版本串 `0.1.43+gcb0fbd8a5d3d….dirty`，第四次混版本）。`cb0fbd8a5` 比 `384e67151` 只多 4 个提交（审阅附件校验 `6b5864b46`、spatial_rooms `901bcf1cd` / `cb0fbd8a5`、W7t 截图 `ed872f5ee`），读取集合 / `Cargo.lock` / `Cargo.toml` / 与 vendor 耦合的测试都没动，vendor 组合不换。用户选「重新验再推」：`deliver\gen-model` 换到 `cb0fbd8a5`（vendor worktree 仍是 b758b28 / 8a342cb / c9cc8bb / f37be2e / f888a50），`cargo metadata --locked` 过；`cargo test --lib` **2597 通过 / 0 失败 / 133 忽略**（`deliver-cb0fbd8a5-test.txt`）；CI 同款 `cargo check --locked` 过、锁文件不变（`deliver-cb0fbd8a5-check-locked.txt`）。推送（都快进、不加 `--force`，推前 `ls-remote` 核过 publish 仍是 `cb0fbd8a5`、三条发布分支仍是旧哈希）：e3d-model `main` → `d0fe6d1`（71 个）、e3d-io `main` → `8a342cb`（4 个）；e3d-io `8a342cb:ci/gen-model-linux-20260918`（152）、e3d-model `b758b28:ci/gen-model-linux-20260918`（116）、old-aios-core `c9cc8bb:ci/gen-model-linux-20260918`；gen-model `cb0fbd8a5:refs/heads/ci/linux-build-20261024` 触发 CI run 40108117494（23:41 起）。六条 issue 评论已发（`submitted.json`：gen-model#58 / #34，e3d-model#5 / #1 / #6 / #8）。

**部署核对（2026-10-25 00:37，CI run 40108117494 success）**：「Checkout vendor path dependencies」打印 `e3d-model -> b758b28`、`e3d-io -> 8a342cb`、`e3d-attlib -> f37be2e`、`plant-mbd -> f888a50`、`old-aios-core -> c9cc8bb`，`plant-ui -> 490da62`；编译日志里 `Downgrading` 0 次；新二进制自报 `aios-database 0.1.43+gcb0fbd8a5d3d.1792857888 (release 0.1.43, built 2026-10-25 00:04:48 +08:00, git cb0fbd8a5d3d)`，**不带 `.dirty`**；`/api/v1/health` 6 s 就绪、`/api/review/health` ok。线上自此是 gen-model `cb0fbd8a5` 配本计划的 vendor（e3d-model 到 `b758b28` = W7k 零尺寸 CYLI 那一版；W7k 尾「矩形 TUBI」`22c5e71` / POINTR `fb72d65`、W7t、W7j 的 10 个提交在 main 上、随下一版）。**W6 第 6 步待做**：服务端 `model/rebuild` 整库重生成与分专业抽查（先看 `failed_root_count` / `failed_roots` 与 ISSUE-029「已放弃」的根）、#59 启动续跑复核——服务器地址要你在会话里给。干净检出（`D:\work\plant-code\deliver`）按 M7 末尾待第 6 步核完后 `git worktree remove`。

**W6 第 6 步（2026-10-25 18:18，用户「按推荐继续」、卡上选「由我做」）**。服务器地址取自 `linux-binary.yml` 的 `REMOTE_HOST`（不再单独要）。只读核对：版本 0.1.43（00:37 起），29 个 DESI 库，`watch_dbnums = [7998, 8000]`，两库都在「等待手动初始化」（`auto_initialize_new_dbnums=false`，没有模型水位；8000 有 953 个生成根、ready 0），`/tasks` 为空——线上自部署起从没为这两库生成过模型，所以「整库重生成」在这台机器上就是运维面板的「初始化所选库」：`POST /api/v1/update/execute {dbnums:[7998,8000]}`，202，入队 15 条（13 条 SYS meta 随批 + 7998 + 8000）。回执（`verification/regress-20261021/server-init-w6s6-20261025.log`）：15 条全 `succeeded`；8000 解析 6639 个元素、「整库模型 953 根已直写（失败 0）」，953 个工作单全 `generated`，`ingest_and_generate` 1.7 s、`dependency_closure` 9.7 s，18:18:37 → 18:18:50；7998 只有 4 个元素、生成根 0。之后 `/health`：`initialization.status = model_ready`、`dbnum_consistency` judged 2 / in_sync 2 / dead_roots 0（ISSUE-029 「已放弃」的根 0）、`batch_failures` / `geom_errors` 空、`model_scope_regen.pending` 0。

抽查（`server-spotcheck-w6s6-20261025.txt`，脚本临时、用完已删）：用 `POST /api/v1/model/records` 把 8000 的 953 个根全部取回，按 refno 对 `gen-ams8000-w7v7/elements.json`（本地 `gen_ams`，e3d-model `da5aeb7`）比世界 AABB（容差 1 mm）。服务端唯一记录 2688（接口按祖先取，STRU 根会把名下 FRMW / SUBS 根的记录再返一遍，回 2819 条里 131 条是这样的重复），本地 2683：**2681 条逐一对上**（HVAC / 桥架 / MDS 支撑 / 设备全覆盖），差的只有两处：① `/M-OR-1RX-3081D` 服务端 13 条、本地 11（2 条 TUBI 多、另 2 条 TUBI 分段不同）——就是 W7j 的 SPKBRK 修复 `d0fe6d1`，线上的 e3d-model 是 `b758b28`、还没带上（本地 `w7u1` 以前的门也是 13），随下一版；② `/C-OR-1RX-3040A` 服务端多 3 条 500 × 550 的隐式管身 TUBI（y −695 → 721 / 703 → 2120 / 2102 → 3269，落在 MDS 支撑 ATTA `/1RX03-MSUP-E0033..E0037` 的 DATUM / SREF 之间、覆在 FTUB 2990 / 3000 上），本地 `gen_ams` **同一 e3d-model `b758b28`**（`a43q`）与所有门、E3D 自己的导出（zone 2498，5 条）都没有；排除了数据（本地 ams8000 到会话 714、服务器 651，`element_probe --sesno 651` 该支成员与属性逐字相同；FTUB / BEND 的 AABB 两边一致）和目录（服务端能解 7320 的 SPCO `/MDS/.RACK-REST.500` → SCOM `/RACK-REST-500` `GTYP=ANCI`，ATTA `SPKBRK=true`），剩下的只能是服务端生成管线调 e3d-model 时的取数 / 选项差别（LSTU 未设的「绘制门」没拦住、走了到达侧通径 HBOR 500 / BOXI 的兜底），要服务器日志或本地起 gen-model 复现，不在本计划里修，已单开 gen-model issue 跟：https://github.com/happyrust/gen-model/issues/70（草稿 `output/github-issues-20261021/17-gen-model-server-implied-tubes.md`，`issues.json` 键 `ISSUE_SERVER_TUBES`，用户卡上选「现在发」后由 `submit.ps1` 发出）。W6 第 6 步至此做完；#59 的「启动续跑」这次没有触发条件（没有中断过的批次），没复核。

**只换 vendor 的交付（2026-10-25 18:54 起，用户「继续下一步」、卡上选「只换 vendor」）**。范围：e3d-model 发布分支 `b758b28` → `da5aeb7`（15 个：W7k 尾、W7t、W7j、M8 ISSUE_09 四步），e3d-io / old-aios-core 不动，gen-model 仍是线上的 `cb0fbd8a5`（`origin/refactor` 已到 `79e318cab`、多 19 个其它线的提交，不带）。干净检出（`deliver\`，e3d-model worktree 换到 `da5aeb7`，其余不动）：CI 同款 `cargo check --locked` 通过、锁文件不变（`deliver-cb0fbd8a5-e3dmodel-da5aeb7-check-locked.txt`）；`cargo test --lib` **2596 通过 / 1 失败 / 133 忽略**，红的只有 `model_impact::...the_generation_read_set_mirrors_e3d_model...`（cb0fbd8a5 的读取集合 169 项对 da5aeb7 的 171，测试钉、不进生成，同步提交 `13b406bbe` 已在 refactor、随下次 gen-model 交付），预判如此、用户知情（`deliver-cb0fbd8a5-e3dmodel-da5aeb7-test.txt`）。生成物没有重跑：回归门 `w7v7` 就是在 `da5aeb7` 的工作树上跑的（未提交的只有另一条线的 `rvm_compare.rs` / 测试 / examples，不进 `gen_ams`）。推送（推前 `ls-remote` 核过发布分支仍 `b758b28`、`publish` 仍 `cb0fbd8a5`）：e3d-model `da5aeb7:ci/gen-model-linux-20260918`（快进）；gen-model `cb0fbd8a5:refs/heads/ci/linux-build-20261025` 触发 CI run 40186936076（18:58 起）。**19:37 部署成功**（run success 19:37:50，构建步 38 分钟）：CI 日志「Checkout vendor path dependencies」打印 `e3d-model -> da5aeb7`、`e3d-io -> 8a342cb`、`e3d-attlib -> f37be2e`、`plant-mbd -> f888a50`、`old-aios-core -> c9cc8bb`、`plant-ui -> 490da62`，`Downgrading` 0 次；新二进制自报 `aios-database 0.1.43+gcb0fbd8a5d3d.1792927251 (built 2026-10-25 19:20:51 +08:00)`，`/api/v1/health` `started_at 19:37:12`、`initialization = model_ready`、`awaiting_manual_initialization` 空，`/api/review/health` ok。随后 `POST /api/v1/dbnums/8000/model/rebuild`（202，`db-20261025-193908-000000`）：19:39:42 `succeeded`，953 根全 `generated`、失败 0、`dead_roots 0`，回执的「未产管」清单把每条没画的槽位连原因列出（misaligned / reversed / missing ppoint / leave stub unset）。重跑抽查：**服务端唯一记录 2683 = 本地 2683，2683 条全部对上，0 多 0 少**——`/M-OR-1RX-3081D` 回到 11，`/C-OR-1RX-3040A` 也回到 5（回执里那三个槽位记为 `leave stub unset`）。即 3040A 那三根是 `b758b28` → `da5aeb7` 之间 e3d-model 自己的变化，不是服务端管线的问题，上一段的推断错了：已在 #70 补更正评论（https://github.com/happyrust/gen-model/issues/70#issuecomment-6397182660 ，草稿 `18-gen-model-70-comment.md`，键 `COMMENT_GEN70`）；用户「按推荐继续」后 19:48 由我 `gh issue close 70 --reason completed` 关闭。干净检出 `deliver\vendor\e3d-model` 留在 `da5aeb7`。

### M9 · 布尔结果的运行间确定性（2026-10-25 晚，e3d-model #9，用户卡上选「开新一轮」）

起点是 D14 补验留下的那条「顺带察觉」：ams7997 `EQUI /Copy-of-1RCS000BA-BODY-NEW`（`24381/156976`）`SUBE 8` 下六只扣过负体的 CYLI 逐次运行三角剖分不同。

- 取证：`gen_ams --root 24381/156976` 连跑两遍（2 s / 遍），`generated.rvm` 哈希不同，`elements.json` 逐 refno 比 5 只 CYLI（`24381/157772 / 157785 / 157798 / 157811 / 157824`）
  三角数不同、体积到 1e-6 相同，其余 727 件逐字节相同（已记到 #9）。第一个怀疑对象是 manifold-csg 缺省的 `parallel`（MANIFOLD_PAR / TBB）：改 `default-features = false`
  重编（布尔慢了一半：negatives 1.49 s → 2.15 s）还是不同（3 只），排除；manifold 并行与否都是确定的。
- 根因在我们这边：`solid::face_planes`（近共面吸刀 `snap_negatives_to_host_faces` 的面表）按（量化法向，量化偏移）用 `HashMap` 分桶再 `into_values()`，
  Rust 的 `HashMap` 每个进程换一个哈希种子，面表顺序随进程变；`snap_one_negative` 把外推按距离稳定排序、同量的保持面表顺序，`neg ∪ (neg + n·d)` 的先后因此随进程变，
  差集的剖分跟着变。整个 e3d-model 的布尔路径上只有这一个 `HashMap`。
- 改法（e3d-model `src/solid.rs`）：桶换 `BTreeMap`（键是 `[i64; 4]`，按键定序）；外推排序同量时再按法向三个分量定序。几何不变。`Cargo.toml` 不动（`parallel` 保留）。
  修后开着 `parallel` 连跑五遍逐字节相同，哈希还与不并行的那份相同（`5353B9DD6A985F12`）。
- 门：真库门 `tests/boolean_determinism_real.rs::generating_the_same_root_twice_writes_byte_identical_rvm`（连跑两遍，三角数与 RVM 字节必须相同），
  对着改前的 `solid.rs` 跑它确实红（5 只三角数不同），改后绿；`cargo test --lib solid` 55 通过。回归门 `w7v8`（对 `w7v7`，`gates-w7v8.txt`）：V1–V5、V7–V9 **0 变化**，V6 525 → 526 通过 / 0 失败；
  aps7200 2352 / 2348 / chord 3 / off 1、aps250209 4623 / 4623、aps250212 6 / 6 与 `w7v7` 同（布尔结果只换了剖分，包围盒 / 体积不动）。
- 提交：e3d-model `dbe3d76`。#9 已补根因与改法的评论。**2026-10-26 04:47 用户「按推荐继续」→ 推 + 只换 vendor 再交付**：干净检出 e3d-model 换到 `dbe3d76`、
  gen-model 仍 `cb0fbd8a5`，CI 同款 `cargo check --locked` 通过、锁文件不变（`deliver-cb0fbd8a5-e3dmodel-dbe3d76-check-locked.txt`；读取集合没变，`cargo test --lib` 不重跑）；
  推 e3d-model `main` `da5aeb7` → `dbe3d76`、发布分支 `da5aeb7` → `dbe3d76`（快进）、gen-model `cb0fbd8a5:ci/linux-build-20261025b` 触发 CI run 40232462675（04:48 起）。**05:23 部署成功**（run success 05:24）：CI 日志 `e3d-model -> dbe3d76`、`e3d-io -> 8a342cb`、
  `old-aios-core -> c9cc8bb`，`Downgrading` 0；新二进制 `0.1.43+gcb0fbd8a5d3d.1792962483 (built 2026-10-26 05:08:03 +08:00)`，`started_at 05:23:21`、`model_ready`。
  8000 `model/rebuild` 连做两次（`db-20261026-053619-000000` / `db-20261026-053655-000001`，各 953 根 `generated`、失败 0）：两次的 2683 条记录的网格 id 逐条相同；
  与 `da5aeb7` 那版的快照比也全相同——ams8000 里没有会逐次变的布尔面片（#9 的样本在 ams7997，不在监听范围），所以这次上线对 8000 没有可见变化，
  只是把确定性钉住；抽查 2683 / 2683 全对上（`server-init-w6s6-20261025.log`、`server-spotcheck-w6s6-20261025.txt` 末段）。干净检出 `deliver\vendor\e3d-model` 留在 `dbe3d76`。
  上线后要让线上已有的烘焙 id 收敛，还得对带负体的根重生成一次（`model/rebuild`）；之后同一件的 id 就不再漂。

### M10 · 线上范围扩到 MDB /ALL（2026-10-26，用户卡上选「全部 29 个 DESI 库 + 同步放开监听并重部署」）

- 先试点名 `POST /api/v1/update/execute {dbnums:[7 个]}`：202 但一个都没入队——回执 warnings「本进程处于监听限定模式…」「dbnum=1112: 在执行范围内，但本次扫描没有找到它的候选文件」。线上这版的 execute 仍走 watcher 扫描、受 `watch_dbnums` 门管
  （配置注释 09-15 说的不受门管的是 `bin/initialize_ams_dbnums` 那类直接调 `initialize_project_dbnum_baseline` 的入口），所以改成先放开再初始化。
- 配置：`deploy/backend-20260918` 的 `DbOption.toml` 注释掉 `watch_dbnums = [7998, 8000]`（= 全范围），`render_dboption.py` 本地渲染自检通过，提交 `953acee43` 快进推送；`ci/linux-build-20261026` = `cb0fbd8a5` 触发 run 40260209151，11:23 部署成功。
  16:13 另一条线又把 `publish` 推到 `83aa3c250`（0.1.44，run 40288504724）部署，vendor 不变（e3d-model `dbe3d76` / e3d-io `8a342cb` / old-aios-core `c9cc8bb`），线上现为 `0.1.44+g83aa3c250088.1793001325`；/health `watch_dbnums` 空、startup scope=all 29/29。
- 初始化：两批 execute（20:41 七个有基线的库、20:56 其余），AMS 20 个 DESI 库全部入队跑完：1112 18 根、7324 2890、7329 2672、7350 2878、7997 6772、7999 4435、7326 5435、7327 6217、7331 / 7332 各 292、7333 32038（4.6 min）、7352 32、7354 644、7356 954，
  7334 / 7353 / 7741 无生成根；**7330 partial：360 generated / 4 failed**。其它项目的 9 个 DESI（ACP ×8、scb6003）不收：「本次扫描没有它的候选文件」，execute 只扫主项目 ams000，design 阶段总数也只算 20（gen-model 既定口径，记在 #63 更新里）。
  `/health` 最终 model_ready、design 20/20，`dbnum_consistency` judged 20 / in_sync 19 / lagging 1（7330）。回执见 `server-init-scope29-20261026.log`。
- 逐 refno 抽查（`server-spotcheck-scope29-20261026.txt`，对 `w7v8`，容差 1 mm）：7324 4071 / 4071、7350 4059 / 4059 直接对上；另四库挖出两个服务端问题：
  1. **旧行不随 vendor 升级重算**：1112 153 / 190 墙上 FIXING 偏 19–28 m（根 `/1RS-WF04-W`、`/1RS-WF05-W` 全偏、`/1RS-WF03-W` 全对，同根内是同一个刚体变换，服务端 `world_trans` = 沿 owner 链只乘 POS/ORI——e3d-model 09-15 修掉的 `fixing_host_frame` 那个 bug 的形状），7997 1259 件（最大 64 m）、7999 15（VALV / OLET 100–350 mm）、7329 1。
     本地 `gen_ams --root 17496/105987` 是对的；对四库强制 `model/rebuild`（`db-20261026-211904-000047` / `-212129-000048` / `-212219-000049` / `-212357-000050`）后 **1112 5135 / 5135、7329 3764 / 3764、7999 15845 / 15845、7997 53698 / 53698**。机制：`model_target` 只含源 / 依赖会话，没有生成器身份，目标没变就走 `cached_root_commit`。草稿 `21-gen-model-stale-rows-after-vendor-upgrade.md`（ISSUE_STALE_ROWS）。
  2. **一根 failed 整库不可见**：7330 的 4 根失败→`model_verdict lagging`→`model/records` 对整库返回 0 条（`model_source memory / model_lagging`），已生成的 360 根一件不可见；那四根自己也 0 条。草稿 `22-gen-model-lagging-library-invisible.md`（ISSUE_LAGGING_INVISIBLE）。
     另观察：0.1.44 重启后 `model/roots` 对所有库报 `ready_total 0`（记录完好），做过 rebuild 的库才变回。
- 7330 那 4 根的根因在 e3d-model（本线）：两框 `/MDS/SPECIALS/PIPE/3`、`/4` 里零长 SCTN（`23714/1011`、`1025`，POSS=POSE）名下 SNOD→SJOI（`23714/1013`、`1027`）的标架从宿主轴架现算拿到 `Degenerate` 记 failed；零长 SCTN 本身早就记 skipped。
  E3D 真值：`mds_xfr_lv6_23714_1.rvm`（`/MDS/SPECIALS` 整棵 TTY 导出）里这些件都没有图元，同框 `23714/1001` / `1024` / `1029` exact（compare JSON `only_baseline` 0）→ 按「E3D 同样不出几何才记 skipped」该记 skipped。
  改法：`section_axis_frame` 与 `placement_between` 同分法（有限零长报 `EmptyLength`）；`transform::section_host_without_axis`（只认 SCTN→SNOD、SNOD→SJOI 两种有样本的拓扑）；`model_element_with_cache` 在目录件臂之前记 `Skipped(NoShape)`。几何不动。
  门：库内 `snod_joints_on_a_zero_length_section_are_no_shape_not_failed`（含反面 ams7324 `SJOI 23708/2412` 照出实体）、真库门 `tests/zero_length_section_joint_real.rs`（`--root 23714/998` / `1023`：failed 空、关节在 skipped、同框杆件照出；对着改前代码确实红）。
  回归门 `w7v9` 对 `w7v8`（`gates-w7v9.txt`）：V1–V5 / V7–V9 逐对 0 变化（ams7330 仍 959 / 959），V6 526 → 528 / 0 失败；aps7200 2352 / 2348 / chord 3 / off 1、aps250209 4623 / 4623、aps250212 6 / 6 同前。
  提交 e3d-model `797eee9`。干净检出 `gen-model 83aa3c250 + e3d-model 797eee9` 的 `cargo check --locked` 通过（`deliver-83aa3c250-e3dmodel-797eee9-check-locked.txt`）。
  **2026-10-26 22:1x 用户「继续」（卡上推荐项）→ 推 `main` / `ci/gen-model-linux-20260918` `dbe3d76 → 797eee9`，gen-model `83aa3c250:ci/linux-build-20261026b`（线上已是 0.1.44，只换 vendor）→ run 40337662161，22:48 部署成功（CI `e3d-model -> 797eee9`，二进制 `0.1.44+g83aa3c250088.1793025237`）。**
  22:55 `model/rebuild` 7330（`db-20261026-225539-000000`）：364 / 364 generated、failed 0；抽查 959 / 959；/health model_ready、dbnum_consistency 20 / 20 in_sync。gen-model #73（旧行不重算）/ #74（lagging 整库不可见）已建。

### M11 · #7 第 3 类：Pyramid 记录照属性 / 目录参数直出（2026-10-26 晚，用户卡上选「做 #7 第③类」）

- 取证：ams7324 `/SUPPORT/FRAMES/BS/FT48B/6-SUBS1` 的 4 只设计 PYRA（`23708/46999..47002`）element_probe 读属性 XBOT 10 / YBOT 0 / XTOP 10 / YTOP 51 / XOFF 0 / YOFF −25.5 / HEIG 51 = E3D 的 `Pyramid[10, 0, 10, 51, 0, -25.5, 51]`（`mds_bs_lv6_23708_46910.rvm`）；
  ams7997 `/-VES-E-4-L-2501` OFST 1（`24381/30502`）/ `/-VES-E-4-L-2506` TAPER 1 的目录 LPYR，E3D `[500, 500, 500, 500, 400, 0, 800]` / `[500, 500, 500, 500, 0, 200, 900]` = `lpyr_geometry` 的七个目录参数。我们此前由网格识别器猜轴，挑到斜边。
- 改法（e3d-model `5a72d3e`）：`AnalyticPrimitive::Pyramid { parameters }`；设计 PYRA 在 `primitive_local_solid` 带上属性（独立正体才带）；`mesh_key_analytic` 对 `CatalogueMeshKey::Pyramid` 取回目录参数；`analytic_from_primitive` 直出记录，记录盒照真几何，自校闸照过。几何不动。
- 门：`rvm_record_kinds_real::design_pyramids_and_offset_catalogue_pyramids_write_their_parameters_like_e3d`；Pyramid 普查（`rvm_pyramid_census.py`，93 对文件）62 / 62 逐参数相同（此前 52 / 58）；回归门 `w7v10` 对 `w7v9`（`gates-w7v10.txt`）V1–V5 / V7–V9 0 变化、V6 529 / 0、aps 三门同前。
  **2026-10-26 23:1x 用户卡上选「推 + 发评论、不重部署」→ 推 `main` / `ci/gen-model-linux-20260918` `797eee9 → 5a72d3e`（快进；线上仍 797eee9，记录写法不影响线上网格，随下次部署）；
  #7 评论（COMMENT_E3D7_PYRAMID）与 #63 状态更新（COMMENT_GEN63）已发（`submitted.json`）。** #7 剩第 1 类（67 只 TUBE 滚角）、第 2 类（C-IY 3 条 SREV Z 反号）。
  #8 收口评论草稿 `24-e3d-model-8-comment-close.md`（COMMENT_E3D8_CLOSE）已写、未发；关票由用户。

### 第二次审核（2026-10-22）

按用户要求对第二次送审版做了一次只读核对：gen-model 的 reflog 与远端引用、`linux-binary.yml` / `windows-binary.yml` / `windows-tests.yml`、`build.rs`、`Cargo.toml`、`changelog.md`、`python/aios_client.py`、读取集合条数钉、干净检出的登记。vendor 仓、`output/`、`verification/` 不在 gen-model 目录里，只核了计划内部的前后一致。用户回复「继续」后按下表落进计划；W7 表、W7 的执行记录和 W6 末段的提交清单由正在做 W7 的那条线维护，这次没动。

| 发现 | 处理 |
|---|---|
| 本地 `refactor` 已越过 `7fdc4c15c`（之后的提交见 W6 末段），W6 第 5 步的 `refactor:publish` 会把未验证的提交推上去并触发部署；按提交号推只写在 W6 末段 | W6 第 2–5 步正文改为按提交号，e3d-io 也钉到 `3bed2ba`；推前先 fetch 核远端 |
| D11 批的是干净工作树重跑 V1–V9，实际只做了 V6、aps250110 逐字节与 `cargo check` | 新增 D14 |
| gen-model 这次的 21 个提交（含 13 个其他工作线与 `handlers.rs` 自动合并）只过了 `cargo check`，CI 不在 `refactor` / `publish` 上跑测试；W7d 那条整组失败的测试没有跟踪项 | W6 第 1 步补 |
| `partition_splits_pose_from_regen_and_respects_cancellation` 整组跑红、单跑绿的根因：`options::model_entry_drop_no_geometry()` 的测试覆盖是进程级 `AtomicU8`，只有 `ModelEntryDropNoGeometryOverride::set` 一侧拿锁，读门的一侧不拿。同模块的 `no_geometry_entry_gate_only_removes_model_work` 把门按开的那一窗里首次加载 `default_noun_capabilities()`，窗被拉得很宽；并行跑的这条测试在窗内调 `partition_operation_impacts`，辅助函数 `modified_op` 的 noun `SCOM` 被判为无几何入口整个丢掉，两条断言都红；单跑没有别的测试按门所以绿。与 SUPC 无关 | 测试里持覆盖把门钉在关（持着覆盖即与按门的测试互斥）：`src/data_interface/model_update_plan.rs` 工作区改动、未提交，只改测试、不动产品代码，不进 0.1.43，随下一版。验证：`cargo test --lib model_update_plan` 整组连跑几次都绿。`window_root_plan` 里两处读同一道门的测试若将来也飘，同一处方 |
| 版本串只含 gen-model 的提交，证明不了 vendor | W6 第 5 步加核 CI 打印的 vendor 短哈希 |
| vendor 发布分支推进后，旧 run 的 Re-run 会混版本；部署回滚没有步骤 | §7 补；新增 D15 |
| 0.1.43 起 ISSUE-029 连败 5 次判死并撤旧模型，第 6 步没看失败根 | W6 第 6 步补 |
| W7d 改了对拍工具，V4 / V8 / V9 换了口径，§6 的基线与判据没跟上；并集包围盒会掩盖单根管的位置错 | §6 改基线写法、补工具改动的判法；新增 D16 |
| W4 验收仍写 2591 只 FIXING 全有真值；M5 小标题仍是「待定」 | 改成 D6 (b) 的口径 |
| M5 写「全部 10 本」，V9 原写「aps250110 + 其余 10 本」，本数差一（合计对数自洽） | V9 不再写本数；本数待下次跑 `aps_survey_compare.ps1` 时核 |
| 0.1.43 的 changelog 写「快进到各自 main」，而 main 已越过 `230d20b` | 不改 `7fdc4c15c`（会让验证过的哈希作废）；按提交号推后「→ `230d20b`」仍准确，措辞随下一版修 |
| 推 vendor 的 `main` 会改变 gen-model `windows-tests.yml` 的输入 | §7 补一条，属预期 |
| 干净检出直接删目录会留下失效的 worktree 登记 | M7 末尾改为 `git worktree remove` |
| W6 末段「第 2–5 步照上面的分支名推会把它们一起带上」一句，在第 2–5 步改为按提交号之后已过时 | 该段由 W7 那条线维护（审核期间它刚补上 W7i / W7e 的提交），留给它顺手改；续审时还在，已改正 |

核对无误的：`7fdc4c15c` 之前本地 21 个提交与 W6 第 4 步的拆分一致（含 #59 的 `aa6ccb44e`）；`origin/refactor` = `origin/publish` = `220a6ec87`；`Cargo.toml` 与 `python/aios_client.py` 的 `EXPECTED_SERVER_VERSION` 都是 0.1.43；干净检出 `deliver/gen-model` 的 HEAD 是 `7fdc4c15c`；`linux-binary.yml` 的触发、vendor 默认分支与两个旧哈希的跳编译逻辑与 W6 的描述一致；pre-push 守卫只拦推 `main`；读取集合条数钉随每次同步跟进（`c6cd27a85` 时为 162）。本次改了文档和上表那一处测试，没有动产品代码。

#### 续（2026-10-22，W7h / W7k 入计划之后）

用户「继续批注」。只核计划内部的前后一致与 gen-model 侧能核的文件；e3d-io / e3d-model 的提交与 `output/`、`verification/` 仍不在绑定目录里。

| 发现 | 处理 |
|---|---|
| aps7200 的账对得上：2294 对按堆拆——只在 E3D 34 + 20 + 3 + 1 = 58，off 14 + 6 + 5 + 5 = 30，chord 3，种类不同 9 + 11 + 1 + 6 = 27；W7k 修后 exact +12（11 弯头 + REDU）、off 18、种类不同 16，与根因表 12 行逐项相加一致（off 2 + 6 + 4 + 1 + 1 + 1 + 1 + 2 = 18，只在 E3D 34 + 20 + 3 + 1 = 58，种类不同 9 + 6 + 1 = 16） | 无需改 |
| W7h 没报「只在我们」。查了工具：普查模式是拿整库生成逐 ZONE 对单份导出，逐 ZONE 日志里的「只在生成」是其他 ZONE 的全部元素（如 `/A-3` 那份 276），所以 `aps_survey_aps7200-w7h.txt` 的 TOTAL 行根本不含这一项；2406 − 2294 = 112 里有没有真的「我们多画」，要把生成侧限到导出 ZONE 的子树里再数（W7d 那轮的「只在我们 33 → 0」就是这样得来的） | 按 W7d 的做法补一次 aps7200 的「只在我们」，写进 W7h（W7 那条线；要跑对拍） |
| W7k 根因表的「TRNS 1」「头管 1」与 W7h 堆表、「矩形风管余 2 件」（`/HVAC/RECT/M1/B2` 的管身、`/HVAC2-B1/ITEM8`）的对应没写明。ISSUE_13 草稿第 9 类第 10 / 11 条已点名：`TRNS /HVAC2-B1/ITEM8`（off 375）与 `BRAN /HVAC/RECT/M1/B2` 的管身（off 49.5，我们写的是截面求不出时的 1 mm 兑底圆管，所以「疑同头管 UDA」成立，不是扁圆风管那一条） | 计划表里已同步点名 |
| W6 末段的提交清单 W7 那条线已补上 W7k（e3d-model `3a5dadb`、e3d-io `ca949ef`）；「第 2–5 步照分支名推会带上」一句仍在 | 本次改正那一句 |
| W7k 的两处 e3d-io 改动（`ca949ef` 命名角轴的角度字按操作数解码；吊架那条待做的 `decode_number` 负指数定点数）都在目录解码层，影响面不止 aps7200。gen-model 自己的代码不直接用 `catalogue_pml` / `catalogue_expr`，只经 e3d-model 的几何受影响，V1–V9 是对的门；但 e3d-io 自己的测试没在记录里（干净检出编不过，见 §7 的 `dict.rs`；工作区能跑） | W7k 记录补 e3d-io 工作区 `cargo test` 的结果；`decode_number` 先加「±64 内定点数 / 双精度位」两组单测再改（W7 那条线） |
| W7f 的 off 1 是永久的（连着主体的薄片，`reference_artifact` 滤不掉），V9 的 `-Prev` 基线会一直带着它 | 已写进汇总贴「已知、不改的」（`03-gen-model-issue-summary.md`）；ISSUE_13 第 7 类本就记了 |
| 读取集合：W7e 的 `PTS` 已在 `generation_read_set.json`，`model_impact` 条数钉 163，与 W7e 记录一致 | 无需改 |
| 抬头「第二次送审看这几处」没列 W7h / W7k | 本次补 |
| W7k 第二批的账也对得上：只在 E3D 58 → 55（PCLAMP 3 配上，对数 2294 → 2297），off 18 → 8（吊架 6 + 踢脚板 FIXING 3 + SBFI 1），exact 2273 → 2286（+13），种类不同 16 → 9（吊架 6 + `FITT 1 STWA 4` 1）；余 off 8 = `/INNER-KICKPLATE` 梁身 1 + WALL 1 + `/B-BC-1A` / `/C-BC-1A` 2 + RTOR 2 + 矩形风管余 2。e3d-io 领先 3（3bed2ba + ca949ef + 9440bae）、e3d-model 53（39 + 12 + 2）也对 | 无需改 |
| 第二批回归门 `w7l2` 的 V5：45 只 MDS FIXING 体积变了 1e-15 量级，记录按噪音放行。§6 写的是「任一变差就停、不放宽判据」，这是第一次把一次变化当噪音放过；来源应是 e3d-io `9440bae` 改了定点数解码，影响面是所有走 `decode_number` 的目录数字 | §6 V5 已补显式判据（位姿、包围盒、三角形数不变、体积相对差 < 1e-12 视为不变）；那 45 只是哪个目录参数解码变了、`decode_number` 是否进 gen-model 落库的属性文本，要看 e3d-io 代码，留给 W7 那条线 |
| 板上套管第一版被 `w7l` 拦下（ams1112 47 只、`/SSC测试` 12 只），收窄到 PANE 后才过——回归门起作用的一次，记录如实 | 无需改；建议给 GWALL 上 `SJUS DBOT` 的 CMPF 补一条真库门，把这条边界钉住（W7 那条线） |
| 抬头与 §2 第 4 项还写「W7k 修完矩形风管弯头」，第二批（吊架、踢脚板、板上套管）没跟上；W7 表的 W7k 行同 | 抬头 W7 那条线已改；§2 第 4 项与 W7 表 W7k 行本次同步 |

用户「开始实现所有的修复」、「按推荐继续」后（绑定目录换到 gen-model 的上级，能够到 vendor）：

| 事项 | 结果 |
|---|---|
| 测试串扰修复的验证 | 工作区 `cargo test --lib model_update_plan` 连跑 3 次：34 条里 29 通过、5 忽略（`live_*`）、0 失败，每次 0.3 s（修前整组跑红是 W7d 那轮记的，这次没有再复现一遍）。已提交 gen-model `376ce770d`（refactor，在 `7fdc4c15c` 之后，不进 0.1.43） |
| D16 容器组内逐根配对 | e3d-model `db34054`（`src/bin/rvm_compare.rs`，main，在交付的 `230d20b` 之后）：`load_rvm_side` 给每条记录留下自己的世界 AABB（`SnapshotMember::geometry_aabbs_world_mm`）；`merge_container_tubes` 并成一件时把每根管身留在 `Generated::container_tubes`；新函数 `pair_container_tubes`——记录按中心落在哪根管身的盒里认领（一根可认领多条，扁圆风管 Box + Cylinder 那种），认领到的取并集盒与这根比；两边剩下的按中心最近（半对角线内）再配一轮；还剩的记为 E3D 多画 / 我们多画。组内有一根 off 或哪侧多画，这只容器改记 off（原来的 exact / chord 计数回退）；逐根明细在 `pairs[].container_tubes`，合计在 `report.container_tubes` 与控制台那行「组内逐根 …」。单测 `container_tubes_are_judged_record_by_record_inside_the_group`（中间一根挪 20 mm 并集盒不变但两边各剩一根；偏 3 mm 认领后 off 带差值；一根认领三条记录），`implied_tubes_merge_into_the_container_group_e3d_wrote_them_in` 加验留下的逐根盒。验证（用户「开始执行」后已跑）：`cargo test --bin rvm_compare` 17 通过、0 失败；用 debug 构建的新 `rvm_compare`（`D:\Rust\target\debug`，没动回归门用的 `target-e3d-model\release` 那份）对着**已有的** generated.rvm 重做对拍，生成器不变、只换工具，输出标签 `d16-<db>`（`compare_*-d16-*.json`）：aps250142（w7l2）、aps250124（w7l2）、ams8000（w7m2）、aps7200（w7m）四本的 TOTAL 行与旧工具同标签的逐字相同（521 / 519 / off 2；232 全 exact；2614 / 2613 / off 1；2331 / 2322 / chord 3 / off 6 / 只在 E3D 21），没有新的 off。组内逐根：aps250124 33 只容器 189 根全 exact；ams8000 52 只 121 根 exact、我们多画 2（`/M-OR-1RX-3081D` 的 TUBI 2 / 5，其余 3 根 exact）；aps7200 57 只 168 根 exact、off 1（`/HVAC/RECT/M1/B2` 头管 49.5）；aps250142 4 只 7 根 exact，`/RETURN_LEVEL02-002/01` 一根 exact + E3D 多画 1（少的头管），`/SUPPLY_LEVEL02-001/02` 我们那根认领了 E3D 第 5 条 `Cylinder` 差 150、Box + 3 条 Cylinder 没人认领——W7j 三条的条数账从此逐根可见。回归门用的 `target-e3d-model\release\rvm_compare.exe` 在 10-23 01:42 已被重建过（W7 那条线跑门时连带编的），之后再 `cargo build --release --bin rvm_compare --target-dir …\target-e3d-model` 是 up to date，即回归门已在用新工具；下一轮起每份对拍多一行「组内逐根 …」 |

用户「按推荐继续」：D14、D15 取 (a)。D15 的步骤在 §7；D14 的补验要用 `D:\work\plant-code\deliver` 里的干净 `gen_ams` 重跑各门生成、与 w7c 的 `generated.rvm` 逐字节比，待做。

**D14 补验怎么跑**（运行手册，谁跑都照这个）：

1. 核干净检出：`deliver\gen-model` HEAD = `7fdc4c15c`、`deliver\vendor\e3d-model` = `230d20b`、`deliver\vendor\e3d-io` = `3bed2ba`，e3d-attlib / plant-mbd / old-aios-core 在各自发布分支的提交，`git status --porcelain` 都为空；干净 `gen_ams.exe` 是 D11 那次编的（在 deliver 的 target 目录里，找不到就在 `deliver\vendor\e3d-model` 重编 `cargo build --release --bin gen_ams`）。
2. 回归脚本都把二进制写死在 `old\target-e3d-model\release`（`civil_regen.py` / `s1_regress.py` 的 `BIN`，`aps_regen.ps1` / `aps_survey_compare.ps1` / `mds_xfr_regress.ps1` 的字面路径），不要覆盖那份 exe（W7 那条线还在用）：给这五个脚本各做一份临时副本（`tmp_d14_*`）把路径改成干净 exe，标签用 `d14`，跑完删副本。用临时副本按 `run_gates.ps1` 的 V1、V2（含 ams8000）、V7、V9 跑生成；V3 / V4 / V8 只是对拍，生成物来自 V2。
3. 逐字节比：`regress-20261021\gen-*-d14\generated.rvm` 与同名 `gen-*-w7c\generated.rvm`（土建的在 `civil-tty-20261021` 下同样对）按 `Get-FileHash` 逐个比；全部相同即视同 V1–V4、V7–V9 在干净代码上通过（对拍工具换了 D16 的版本，所以别比 TOTAL 行，比生成物）。哪份不同就拿新工具对那一份跑对拍、逐件归因。
4. V5：把 `output\e3d-model-patch\examples\zz_tmp_fixing_truth_check.rs` 复制进 `deliver\vendor\e3d-model\examples`，`cargo build --release --example zz_tmp_fixing_truth_check`，按 `run_gates.ps1` V5 那三组参数跑，输出 `check_*_d14.jsonl`，用 `fixing_check_diff.py` 对 `check_*_w7c.jsonl`；跑完删掉复制进去的 example。
5. `deliver\gen-model` 里 `cargo test --lib`：预期全绿，只有 `partition_splits_pose_from_regen_and_respects_cancellation` 可能因串扰红（修复在 `376ce770d`，不在 `7fdc4c15c`），红了单跑复核。
6. 结果写进这一节，然后才推送（W6 第 2–5 步）。

**D14 补验结果（2026-10-23）**。绑定目录没换，改在 `old\d14-clean\` 下按交付提交另建了一套干净 worktree（e3d-model `230d20b`、e3d-io `3bed2ba`、e3d-attlib `f37be2e`，后来又加 gen-model `7fdc4c15c`、plant-mbd `f888a50`、old-aios-core `a438029`、plant-ui 发布分支 `490da6217`，`vendor\shared` junction 指到同级 vendor），`gen_ams` 在 `d14-clean\target` 里重编（1 分 39 秒），没动回归门用的二进制。

- 生成：用干净 `gen_ams` 按各脚本同样的参数跑了 44 份（土建 23：core 5 + ssc 12 + ptu 6；S1 七份含 ams8000 / ams8000-obst / ams7324 / ams7997；MDS 三套框；APS 11 本），标签 `d14`，逐份 `Get-FileHash` 比 `*-w7c\generated.rvm`：**43 份逐字节相同，1 份不同**（`regress-20261021\gen-ams7997`，差 21 760 字节）。
- 那 1 份：`rvm_compare` RVM ↔ RVM（`compare_ams7997-w7c-vs-d14.json`）两侧 53 697 件全 exact、记录种类全同；逐成员比 PRIM 载荷哈希只有 6 个成员不同——`CYLI 105 / 107 / 109 / 111 / 113 / 115 of SUBE 8 of EQUI /Copy-of-1RCS000BA-BODY-NEW`，都是扣过负体的布尔结果面片，只是三角数不同。**同一个干净二进制再跑一遍（`gen-ams7997-d14b`），这 6 只里又有 5 只与上一遍不同**——是布尔剖分的运行间非确定性，不是代码差异；其余 53 691 件与 43 份库全部逐字节相同，说明生成本身是确定的。
- V5：把 `zz_tmp_fixing_truth_check` 编进干净 worktree（跑完已删掉复制进去的 example），三组都对 w7c：mdb_all 255 行 0 变、rest 2150 行 0 变、datums 3678 行 0 变，`datum_truth_count.py` 3678 / 3678 match。
- 结论：V1–V5、V7–V9 在干净代码上成立（D14 (a) 的判据：除那 6 只非确定性面片外逐字节相同）。
- **干净 gen-model `cargo test --lib`（W6 第 1 步 ②）：红，而且是交付阻塞。** 2721 条里 2582 通过、133 忽略、**5 失败**（整跑时 `a_damaged_content_addressed_file_is_rewritten_in_place` 还会把测试进程整个带崩，`--skip` 它才跑得到底）；同一批测试在主工作区全绿。差的就是 D11 点名过的两处 vendor 未提交改动，它们不是「不进生成路径就没事」，而是 0.1.43 已提交的代码所依赖的行为：
  1. old-aios-core `src/shape/pdms_shape.rs` + `Cargo.toml`（rkyv 加 `validation`）：`PlantMesh::des_from_bytes` 改成 `check_archived_root` + 三角索引越界检查，坏文件返 `Err`。没有它，a438029 的 `des_from_bytes` 对坏字节走 `unsafe archived_root`：gen-model `f40e8b3d5`（ADR-087 D3，已在 0.1.42 里）的「.mesh 坏了就原地重写」在部署环境里根本不成立，debug 下 rkyv 断言直接 abort，release 下是未定义行为。红的测试：`e3d_mesh_store::tests::a_damaged_content_addressed_file_is_rewritten_in_place`、`checked_mesh_reader_preserves_legacy_archive_bytes_and_rejects_corruption`（`concurrent_repairs_rewrite_once_and_nobody_fails` 同源）。
  2. e3d-model `src/element_diff.rs`（+114 / −3）：匿名显式属性的 diff 按哈希伪名（`#2902D6E2`）报出。没有它，gen-model `0350bac97`（Issue #50，0.1.43 里）的四条 `orientation_source_diff_fixture` 测试红：`synthetic_anonymous_explicit_attribute_reports_hash_pseudo_name`（期望 `["#2902D6E2"]`、得 `[]`）、`synthetic_anonymous_attribute_alongside_ori_blocks_cheap_path`（匿名属性与 ORI 同窗时本该挡住便宜路径，现在挡不住）、`synthetic_orientation_source_same_value_save_is_not_a_model_change`、`window_plan_cache_is_scoped_by_source_file_and_keeps_both_projects`。部署的增量规划会按没有它的行为走。
  干净 worktree 里的这两个库就是 CI 会克隆的发布分支提交（old-aios-core a438029 = `ci/gen-model-linux-20260918`；e3d-model 230d20b 将成为发布分支），所以线上 0.1.43 就会是这个红的组合。见 D17。
- **D17 (a) 已做（2026-10-23）**：old-aios-core 在 a438029 上提交 `c9cc8bb`（只含 `pdms_shape.rs` 与 `Cargo.toml` 的 rkyv 那一行，换行符改动没带；本地分支 `ci/gen-model-linux-20260918` 指到它）；e3d-model 从 230d20b 开 `release/0.1.43`，cherry-pick 了 main 上新提的 `65210cc`（element_diff）得 `aa567e4`，文件内容与 main 逐字节相同。干净 worktree 换到这两个提交后重跑 gen-model `cargo test --lib`：**2588 通过、0 失败、133 忽略**。D14 至此全部过；推送按 W6 第 2–5 步（e3d-model 发布分支推 `release/0.1.43`、old-aios-core 补一条）。`d14-clean\` 的 worktree 验完已删，`gen-*-d14*` 输出留在 verification 里。0.1.43 changelog 里「old-aios-core 不变」那半句不再准确，不改 `7fdc4c15c`，随下一版修。
- 顺带两条工具察觉：① `check_datums_*.jsonl` 从 v3 起每一行的 `status` 都是 `error`（「模板里没有属性 SPRE」，JLDATU / PLDATU 上读 SPRE 失败），位姿列 `wm_dpos_max_mm` / `wm_daxis` 照样有值、判据也只看这两列，所以结论不受影响，但 `status` 该改成只把 SPRE 列留空；② 布尔结果的三角剖分逐次运行不同，对拍没事，但 gen-model 的烘焙网格按内容哈希（`baked_mesh_id`）存，同一件每次重生成可能得到不同的 baked id，要不要当一条 issue 跟，由你定。→ 2026-10-25 用户「按推荐继续」、此项「由你定」，已开 e3d-model issue https://github.com/happyrust/e3d-model/issues/9（草稿 `output/github-issues-20261021/19-e3d-model-boolean-nondeterminism.md`，键 `ISSUE_BOOL_NONDET`），当晚已修（§9 M9）；① 的 `status` 列是探针工具的小毛病，没动。

## 10. 模型遗留清单（2026-10-26 凌晨，按 `w7v8` 门的对拍表与开着的 issue 盘的）

对拍表（V1–V4 / V7–V9 + aps7200 + aps2502xx，30 222 对 + APS 普查）里只剩下面这些；`only_baseline` / `only_generated` / 种类不同全库都是 0。

| 类 | 件 | 现状 | 要不要动 |
|---|---|---|---|
| A1 真几何差 | aps7200 `/INNER-KICKPLATE` off 5.63 | CENT 端点落在圆内那一形的弧终角：E3D 到 358.93° 改直段连终点，我们走到 360.68°；语料里没有第二个样本 | 要修得在 E3D 里造一个同形样本取证；用户 10-25 选「收口」 |
| A2 归因不改 | aps250110 `GENS 1 /AREA02_PR_AXIS_P2.2` off 0.54 | W7f：1:620 放坡直梁退刀近共面布尔的零厚楔尖端位置，追微米级没意义 | 不改（汇总贴「已知、不改」） |
| A3 chord（弦高容差内） | 49 件：aps250104 21（PS-01 支架件 0.10–0.46）、aps250162 5（GENPRI `/MEI-JCAN01..05` 0.49）、aps250112 4（带圆角 RAIL 0.13–0.15）、aps7200 3（PCLA 0.42）、土建 4（GWALL 6 0.41、FLOOR `/1RX-FN03-FN` 2.18、CFLO 0.08 / 0.05）、MDS `FT44/11-V5..V8` × BS / AISC / DIN（0.115） | 圆弧切分密度 / 相位差，验收口径内 | 不改 |
| A4 film / reference_artifact | film 5（≤ 0.0104 mm 亚容差皮：GWALL 13 `/1RS-WF03-W-C-RR001`、FLOOR 12 `/1RX-FN01-F001`、FLOOR 6 `/1LR-WF05-S-C`）；reference_artifact 4（E3D 导出自己的伪影：FLOOR 12 `/1RS-WF03-F-C-F001`、FLOOR 9 `/1RS-WF05-F-C-F001`、FLOOR 2 `/1RX-FN09-F001`、FLOOR 5 `/1RX-FN03-FN`） | FN09 那条是 NXTR `24381/42378` 的退化环（D8，修正清单 `fn09-nxtr-24381-42378-fix.md`） | FN09 等你在 E3D 里改后重导；其余不改 |
| A5 基准数据 | S1 `/MDS/FT01/1-BAR-1` off 7.2 | 10-11 基准 RVM 里是另一根同名杆 | 不改 |
| B 记录写法（几何一致，e3d-model #7） | 隐式 TUBE 滚角 67 / 585、C-IY 3 条 SREV 的 Z 反号（partial 负回转起始相位无样本）；~~6 只 Pyramid 的等价写法~~（10-26 已修，M11，62 / 62） | 对拍表看不见，只影响记录矩阵 / 参数 | 剩两类留 #7 |
| C 生成稳定性 | e3d-model #9 布尔确定性 | `dbe3d76` 已推已上线（10-26 05:23）；7330 零长关节分类 `797eee9` 已推已上线（10-26 22:48） | 线上 20 库全部重算过 / 首次生成，已收敛 |
| D 数据侧 | FN09 退化环；本地 ams8000 会话 714 vs 服务器 651（CRUD 测试留下的） | 后者不是问题，但 E3D 导出都来自本地副本，对拍时要记得 | FN09 由你改 |
| E 不在本计划 | gen-model #34 结构连接级联默认关（关节负体正确性缺口，卡外部前置）；#59 `0xc0000409` 残余根因 | 已有票 | 由你定 |
| F 票面 | e3d-model #1 / #5 / #6 / #9 **已关**（2026-10-26 10:2x，用户卡上选「关已修的票」后由我 `gh issue close --reason completed`）；#8 票面停在「五堆待修」（已全修，10-24 评论已说）、#7 保留；gen-model #62 / #63 的交付项过期（另一条线 10-26 的 `docs/plans/2026-10-26-open-issues-audit.md` 已列） | — | #7 / #8 由你 |
| G 下一版要带 | gen-model `13b406bbe` 读取集合同步（不进生成，只是测试钉） | 在 `origin/refactor` | 随下次 gen-model 交付 |

结论：几何层还能动的只有 A1（要造样本）；其余不是已归因不改，就是写法 / 数据 / 票面。
