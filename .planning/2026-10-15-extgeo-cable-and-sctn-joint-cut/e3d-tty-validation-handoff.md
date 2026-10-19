# #34 SCTN/SJOI/PJOI 端切：交给另一 Agent 的 E3D TTY 验证需求

> 2026-10-18 交接稿。**本次只写需求，不复制项目、不启动或关闭 E3D、不运行设计操作、不恢复原库。**执行 Agent 须先与用户确认安全边界。主计划：`task_plan.md` §7、§10–11；本稿不是通过证明。

## 1. 要回答的问题和判定口径

用**同一个项目副本、同一个目标 SCTN refno**，取得原生 E3D TTY 在受控 RECONNECT/SAVEWORK 前、后的属性与 RVM。分清：

1. 关节 `CUTP/CUTB` 是否通过原生 `RECON0 → STRCNT → JTCONN → PUTEND` 更新目标 SCTN 的 `POSS/POSE/DRNS/DRNE`，且现有正体端面已足以表达原生结果；
2. 是否存在**这些已保存端面之外**的独立负体/额外减法几何。如果没有原生前后 RVM、不能量化同根差异，结论写 `ambiguous`，**不得因 `negatives_skipped`、CUTP 非零或 CUTB 读取而补刀**；
3. PJOI 的无目录 DELP+PNOD.NPOS 路径、SJOI 的 SNOD.ZDIS 路径及有目录分支不可混同；GENSEC 的 FIXING/ENDATU/CUTB 路径不可冒充 SCTN。

验收时区分“现状样本已吻合”与“受控重连前后证明”；`JFRE` 模板默认 false 不等于目标运行时读取值。D1–D3 在满足动态证据前不得标完成。

## 2. 样本及现有事实

| 优先级 | 目标 | 现有信息 | 必须补的证据 |
|---|---|---|---|
| 控制 | APS db 250110 SCTN `2013286654/68`；其 JOIE SJOI `/72` | `/72` 的 owner 是另一 SCTN `/70` 名下 SNOD `/71`，CREF 指向 `/68`；CUTP=(0,0,-1)、CUTB=0、SPRE=0。`/68` 已保存斜端面，既有原生/生成 RVM 现状表面采样差约 0.007071 mm；skipped=1。 | 同根操作前后 Q `/68` 四字段、`/72` CUTP/CUTB/JFRE/CREF、完整原生 RVM。不得用现状吻合推断历史调用。 |
| 无源混接 | APS db 7200 SCTN `23584/5902`，JOIS PJOI `/5899`、JOIE PJOI `/5901`；SNOD `/5903` 下 SJOI `/5904` | 两 PJOI 的 CREF=/5902、CUTB=0、SPRE=0；`/5904.CREF=/5910`，也是 **`/5910.JOIS`**，不是 `/5902` 的直接 JOIS。`/5902` 现状生成侧 skipped=4。 | 导出明确包含 `/5902`（可另取 `/5910` 作为关联宿主）；之前失败的 `sctn_7200_host_cuts` 宏只列 `/5907,/5910,/8622,/8626`，没有 `/5902`，不可当其原生基准。 |
| 跨宿主反例 | APS db 250102 PJOI `2013286646/500` | CUTB=8、SPRE 非零，但其 CREF `/495` 是 **GENSEC**、非 SCTN。 | 仅作为目录源/GENSEC 对照，不列为 SCTN CUTB 非零通过样本；若要证明 SCTN 非零 CUTB，须另选正确宿主。 |

完整属性来源：`verification/joint-corpus-attrs.log`、`verification/sctn-neg-20261015/{aps250110-new,aps7200-new}/report.json`；旧原生现状基准：`verification/e3d-tty-native-20261015-075723/sctn_250110_68-20261015-080136-790/` 及 `sctn_250110_68-surface-compare.json`。旧基准**非**新会话前后试验。

## 3. 安全前置门：未过门就停止

- **禁止直接对原项目做任何新 TTY 试验，包括貌似只读的查询/导出。**原项目 `D:/AVEVA/Projects/E3D3.1/AvevaPlantSample/aps000/aps250110_0001` 在一次原项目 TTY 启动前后，大小仍 9,932,800 B，但 SHA-256 从 `b7a7ea08d47891d6b9369e8eae01c6dfa9b2dc855fae5e84062f748a03ede9ff` 变为 `bfd1cff75fb27bc69da992bfef071eb1ea71a61eb6d3f07c0a6866197ecb484d`；同目录 112 个文件 mtime 落在启动窗口。因果未确定；不得擅自回滚/覆盖原文件。
- 另有用户侧 `des.exe` PID 68320 曾在运行（启动路径 `E:/reverse/e3d/shadow_e3d31_aps_all/des.exe`）；执行前由用户确认源项目静止、复制来源和副本一致性。**不要代用户结束进程。**用户曾选择候选新副本 `D:/work/plant-code/old/e3d31-runtime/projects`，从当前事后状态的 APS+AvevaCatalogue 复制；当时未执行复制。目标不存在，估算 2663 文件/3,770,057,192 B，D: 可用约 94.77 GiB。复制前重新检查容量、目标空目录、源文件稳定及项目权限；复制后提供源/目标清单及哈希，明确标注“事后副本”，不冒充启动前基准。
- **此前配置不安全用于直接启动：**`scripts/e3d/run_ams_c_entrymacro.bat` 当前默认程序是 `D:/work/plant-code/old/e3d31-runtime`、默认项目是原始 `D:/AVEVA/Projects/E3D3.1`；`verification/e3d-tty-native-20261015-075723/export_isolated.ps1` 虽仍带 `isolated` 名字，已改为**原项目**路径，显式 `-AllowOriginalReadOnly` 才可启动。不要直接执行这两个入口的默认组合。执行 Agent 应创建/验证独立的**副本专用 TTY 入口**，显式设置项目副本路径及 evars，确认宏/环境/日志全部指向同一副本，且出错即停。不要复用误导性的脚本名作为隔离证明。
- 用户要求“仍使用之前的 E3D TTY”取证通道。既有成功导出使用 `verification/e3d-tty-native-20261015-075723/` 下的 TTY 方式；**取证通道 ≠ 程序版本已确认**。新 `e3d31-runtime` 在隔离副本及原项目各试一次均于查询前以 `0xC0020001` 崩溃，无 RVM；已知旧 shadow 与新运行时的 `core.dll` 哈希不同，其余抽查 DLL 相同，不可因此擅换厂商 DLL。另一 Agent 须先与用户确认允许哪个运行时，以及只对副本做哪种读写，优先恢复可成功且可复现的 TTY 基准。
- 原生 IDB、厂商 DLL 和原项目保持只读；只有经用户批准的**副本**可执行 RECONNECT、SAVEWORK、FLUSH。保存源/副本目录身份、前后哈希和会话日志；不得把 `QUIT`、启动或“宏无 SAVEWORK”视作文件绝不会变化的保证。

## 4. 可执行的取证流程（仅供另一 Agent 在获准后实施）

1. 对副本记录 project/MDB、dbno、源文件与快照哈希；确认无其他会话写副本。针对每个样本先导出 T0 TTY 属性：目标 SCTN 的 owner、JOIS/JOIE、POSS/POSE/DRNS/DRNE、SPRE；相关 PJOI/SJOI 的 owner、CREF、CUTP/CUTB、JFRE、SPRE，PNOD.NPOS/DELP 或 SNOD.ZDIS；记录表现级和几何选项。
2. T0 原生 RVM 按**具名同根 refno**导出、解析，记录实体/面片、端面法向、AABB、体积及输出哈希。导出宏先静态审查只读；单态导出成功不等于 D3 通过。
3. 仅在副本上、经过单独批准的受控操作与一个清楚标记的 SAVEWORK/FLUSH 周期内对同一连接做 RECONNECT；保留原始宏、标准输出、错误码、是否命中成功门。若启动或命令失败，停止；不得把崩溃后文件修改当成有效重连。
4. T1 用同一 MDB、表现级、导出参数复采同一组属性与 RVM，并比 T0/T1：字段变化、端面方向、体积/AABB、双向表面采样与几何差体。`/72` 原始 CUTP 必须经实际标架变换后才能比较 `/68.DRNE`；同时保留 skipped 用途位置，不按 refno 粗暴去重。
5. 判定：若原生 T1 端面完全由更新的 SCTN 字段解释，标“现有正体覆盖、无新增刀证据”；若有字段外独立减体，定位原生 builder 与正确宿主后再提最窄修复；若 TTY/属性/RVM 或副本一致性失败，标 `ambiguous`，不修改生产几何。切勿把失败的 7200 导出、skipped 总数或 GENSEC `/495` 当 SCTN 通过/失败证据。

## 5. 交付与停止条件

交付：副本来源与清单、运行时版本/哈希、完整 T0/T1 TTY 宏和日志、每样本具名 RVM/解析记录、属性差表、几何对拍 JSON、明确三态裁决及失败/未覆盖项；更新 `task_plan.md`，仅在动态证据完成时勾 D1–D3。**本交接稿未运行任何 E3D 会话或设计写入。**若源库仍有活动写者、复制校验不一致、新运行时再崩溃、原项目路径泄漏到副本会话，立即停机并请用户裁决，不自动重试。
