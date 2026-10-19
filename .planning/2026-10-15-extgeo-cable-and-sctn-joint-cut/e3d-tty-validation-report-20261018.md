# E3D TTY 受控动态验证报告（APS，隔离副本，2026-10-18）

> 计划：`2026-10-15-extgeo-cable-and-sctn-joint-cut`（task_plan.md §2 Phase D / §11）
> 证据根：`verification/e3d-tty-native-20261015-075723/`（下称 `<EV>`）
> 本报告覆盖：项目身份核实、会话清单、属性/RVM 对照、字节级影响、崩溃分析、三态裁决。

## 1. 项目身份（先决纠正项）

**会话实际加载的是 APS 项目 + 隔离副本，已多路核实，不是 AMS/原项目：**

- 每次启动命令行：`des.exe -tty APS SYSTEM/XXXXXX /ALL`（`run_copy_tty.ps1` 固定 `L3_E3D_PROJECT=APS`、`L3_E3D_MDB=/ALL`、`L3_E3D_PROJECTS_DIR=<EV>/projects`）。
- 各会话 `launcher.log`：`sampsi set to …\verification\e3d-tty-native-20261015-075723\projects\AVEVAP~1\sampsi\`（副本路径，非 `D:\AVEVA\Projects`）。
- 各会话 `result.json`：`project: APS`、`env_leak: false`、`runtime_pinned/copy_pinned: true`。
- 落盘反证：写会话崩溃后**副本内** `aps000\aps250110_0001` 出现 +2048B 追加页——写入落点就是副本文件。
- 与用户侧 GUI 会话区分：进程列表中的 `des.exe ams … /ALL`（PID 80420→46228，`shadow_e3d31_aps_all`）是另一条 GUI 会话，本任务会话全部 `-tty APS`、程序在 `shadow_e3d31_gen_model_test`；用户已确认其不影响。

## 2. 基线与快照

- 副本：`<EV>/projects`（APS + AvevaCatalogue，10-15 复制）；T0 清单 `copy-manifest-t0.json`：2663 文件 / 3,770,243,560 B。
- 写前备份：`<EV>/aps000-backup-post-t0/`（aps000 全量 240 文件 / 1,640,480,768 B，T0 只读会话之后、首个写会话之前）。
- 运行时：`E:\reverse\e3d\shadow_e3d31_gen_model_test`
  `des.exe` 3.1.9.0 (sha16 `0B94792B6958E655`)；`core.dll` 1.3.13.0 (sha16 `7C047A8B20B85B56`)；`Core3D.dll` 1.3.13.0 (sha16 `E43EC507026B6BBD`)；`pdmsconsole.exe` 1.3.13.0 (sha16 `3865F1B193CBD735`)。
- 弃用运行时：`D:\work\plant-code\old\e3d31-runtime`（副本/原项目各崩 `0xC0020001`）。
- 原项目 `D:\AVEVA\Projects\E3D3.1`：禁用（曾被「只读」启动改变 SHA）；未触碰。

## 3. 会话清单（全部 `-tty APS … /ALL` + 隔离副本 + gen_model_test）

| 会话 | 宏 | 写命令 | 退出 | 完成标记/分段覆盖 | 结论 |
|---|---|---|---|---|---|
| `sctn_250110_68-t0-20261018` | T0 查询 + /68 RVM 导出 | 无 | 0 | query+export done 齐 | **干净成功** |
| `sctn_250110_68-t1write-*`（首次） | RECONNECT RECALCULATE + SAVEWORK + FLUSH | 有 | 1 | 无 done | 崩溃；aps250110 +2048B |
| `sctn_250110_68-t1verify-*` | T1 复采属性 + /68 RVM | 无 | 0 | 双 done | **干净成功** |
| `aux-probe` / `aux2-probe` | 7200 refno / 具名路径探针 | 无 | 1 | 无 | `0xC0000005`（见 §6） |
| `t1w3-sctn68` | 分段日志：RECONNECT RECALCULATE | 有 | 1 | seg1(pre) 完整；seg2 3B 空 | **崩在 RECONNECT 段内** |
| `t1w4-sctn68` | 分段日志：RECONNECT END | 有 | 1 | seg1 完整；seg2 3B 空 | **崩在 RECONNECT END 段内** |
| `t1w5-sctn70` | 分段日志：/70 RECONNECT END + SAVEWORK | 有 | 1 | seg1–seg4 完整（RECONNECT-ISSUED、POST-QUERY-DONE、SAVEWORK-DONE）；seg5(FLUSH) 3B 空 | **重连+保存成功；崩在 FLUSH/收尾** |
| `t1v70-sctn70` | /70 复采 + RVM 导出 | 无 | 1 | query done；export 段内崩 | 属性复采成功；/70 RVM 导出崩 |

分段日志（每段 `ALPHA LOG … OVER` + `ALPHA LOG END` 即落盘）使崩溃点定位到具体命令段——这是本次取证的关键手段，后续 TTY 宏应沿用。

## 4. 属性对照表（同副本、同 /ALL、同 refno）

**SCTN /68（2013286654/68）与其 JOIE 关节 /72（SJOINT，属主 = SNOD /71，属 SCTN /70）：**

| 字段 | T0 (10-18 02:0x) | T1（写尝试崩溃后复采） | 变化 |
|---|---|---|---|
| /68 TYPE | SCTN | SCTN | 无 |
| /68 JOIS | Nulref | Nulref | 无 |
| /68 JOIE | SJOINT 1 of SNODE 1 of SCTN 3（=/72） | 同 | 无 |
| /68 POSS | W 800 S 342 U 8 | 同 | 无 |
| /68 POSE | W 1784 S 342 U 8 | 同 | 无 |
| /68 DRNS | E 45 N | 同 | 无 |
| /68 DRNE | W 45 N | 同 | 无 |
| /68 SPRE | /SLH/FB/50.0x8x3.14 | 同 | 无 |
| /72 TYPE/OWNER | SJOI / SNODE 1 of SCTN 3 | 同 | 无 |
| /72 CREF | SCTN 1（=/68） | 同 | 无 |
| /72 CUTP | D | D | 无 |
| /72 CUTB | 0 | 0 | 无 |
| /72 JFRE | false | false | 无 |
| /72 SPRE | Nulref | Nulref | 无 |

**SCTN /70（2013286654/70，同墙链 SCTN 3，JOIE = SJOINT on SNOD 2 of SCTN 4，同型跨属主拓扑；RECONNECT END + SAVEWORK 成功前后）：**

| 字段 | seg1 pre | seg3 post（会话内） | t1v70（独立会话复采） | 变化 |
|---|---|---|---|---|
| POSS | W 1784 S 342 U 0 | 同 | 同 | 无 |
| POSE | W 1784 N 342 U 0 | 同 | 同 | 无 |
| DRNS | E 45 S | 同 | 同 | 无 |
| DRNE | S | 同 | 同 | 无 |
| JOIS/JOIE | Nulref / SJOINT on SNOD2-SCTN4 | 同 | 同 | 无 |
| SPRE | /SLH/FB/50.0x8x3.14 | 同 | 同 | 无 |

→ **三次成功执行的 RECONNECT 结果中，/70 这条干净样本证明：在已一致元素上原生 reconnect 是字段幂等的（无改写）；/68 的写路径被崩溃阻断（§6），但其 T0/T1 全字段与 RVM 均无变化。**

## 5. RVM 对照

- /68 T0 RVM：`sctn_250110_68-t0-20261018/t0_sctn_68.rvm` — 1 FacetGroup，体积 ~413600.0 mm³（既有对拍：双向表面最大差 0.007071 mm，体积相对差 4.8356e-6）。
- /68 T1 RVM：`sctn_250110_68-t1verify-*/t1v_sctn_68.rvm` — 与 T0 逐字节仅 HEAD 头块（时间戳/会话信息）不同；实体表、顶点、面片完全一致。
- /70 RVM：导出段崩溃，未获得（见 §6）——/70 的几何对照缺失，不缺口 B 裁决基础（/68 已有）。

## 6. 崩溃分析（core.dll `0xC0000005` @ +0x74D982）

WER 记录统一：des.exe 3.1.9.0、core.dll 1.3.13.0、偏移 `0074d982`、异常 `c0000005`——与 10-15 原项目那次崩溃**完全同址**。分段日志定位的死亡点：

| 会话 | 死亡段 | 段内唯一新操作 |
|---|---|---|
| t1w3 (/68) | seg2 | `=2013286654/68` + `RECONNECT RECALCULATE` |
| t1w4 (/68) | seg2 | `=2013286654/68` + `RECONNECT END` |
| t1w5 (/70) | seg5 | `FLUSH`（RECONNECT/SAVEWORK 已成功） |
| t1v70 (/70) | export 段 | `OUTPUT RVM`（`=` 与 `FILE` 已完成） |
| aux/aux2 | 查询段 | `=23584/…`（未挂载库 refno）/ `=SJOINT 1 of SNODE 2 of SCTN 4 …`（具名路径） |

判定：
- **崩溃与「元素引用解算」同族**：未挂载库 refno、跨属主具名路径、/68 的 reconnect、/70 的 RVM 导出均命中同一偏移；并非单个命令语义错误（ONERROR 无机会——直接 AV）。
- **/68 特异性**：/70 的 RECONNECT END 能跑通，说明运行时具备 SCTN reconnect 能力；/68 两次（RECALCULATE/END）都死在重连段——/68 的 JOIE=/72 是跨属主（宿 /70.SNOD1）且 `CUTP=D`（对角切面）的关节，或受首次写尝试追加页后状态影响，二者未进一步区分。
- **崩溃可重复但非全局**：读+查+T0 导出可稳定成功；写/导出/未挂载 refno 解算是高危点。

## 7. 字节级影响清单（副本 vs `aps000-backup-post-t0`）

- 240 文件逐一比 size+SHA-256：**唯一语义变化** = `aps250110_0001` 9,932,800 → 9,934,848（+2048B 一页追加，发生于首次写会话）；其后 t1w3/t1w4/t1w5（含完成 SAVEWORK 的 t1w5）均未再追加——/70 的干净 SAVEWORK 无任何记录页增长，与「reconnect 幂等无改动」自洽。
- 其余 111 个文件：size 不变、hash 变——全部是各会话打开库时的 session stamp（每次只读会话也如此，属已知行为，非数据变更）。
- `apscom`、SYS 类文件均仅 stamp；目录项目录未比对（未写）。

## 8. 三态裁决

**裁决对象：SCTN/SJOI（aps250110 墙链样本类，拓扑=JOIE 指向下一 SCTN 之 SNOD 上的 SJOINT）**

**结论：① 现有正体覆盖、无新增刀证据（带一处显式 caveat）**

1. 原生 RVM：/68 保存态为单 FacetGroup，斜切端面已含于正体边界；与生成正体双向表面最大差 0.007071 mm——无独立负体/刀实体存在。
2. 动态：/70 证明原生 `RECONNECT END` 在一致元素上可执行且字段幂等（SAVEWORK 零追加页）；/68 的 T0/T1 字段与 RVM 在写尝试前后零变化。
3. 静态：`RECON0 → STRCNT → JTCONN → PUTEND` 链写 SCTN POS/DRN 字段，链内无 cutter/负体 builder；GENSEC 的 FIXING→ENDATU CUTP/CUTB 路径与 PJOI `DELP`+`PNOD.NPOS`、SJOI `SNOD.ZDIS` 无源回退属不同宿主分支，不混入本裁决。
4. **caveat**：/68 自身 reconnect 被运行时崩溃阻断（未证明「/68 reconnect 也无改动」——系从 /70 + T0/T1 不变推断）；「扰动后 reconnect 是否回写字段」未测（需要先制造不一致状态，属更重的写实验，未执行）。

**分支归属（防混淆登记）**：/72 为 SJOI（属主 SNOD /71，`CUTP=D/CUTB=0/JFRE=false/SPRE=Nulref`）→ SJOI 无源链；PJOI /500（`SPRE` 非零、`CUTB=8`）宿主 CREF=/495 为 **GENSEC** → 非 SCTN CUTB 反例；aps7200 `23584/*` 未挂载于 /ALL → 7200 语料本轮零证据，其 PJOI `DELP`/`PNOD.NPOS` 回退分支仍未动态验证。

## 9. 未覆盖 / 失败项（如实登记）

- /68 的 RECONNECT 动态证据：被 0x74D982 崩溃阻断（两次、两种参数形式）——如需闭合，建议：另起干净副本（未经历首次写）复测 /68，或先 `MODIFY` 轻微扰动再 reconnect 以同时证「回写」语义；或换其它可用运行时。
- /70 原生 RVM 导出：崩溃未获得。
- 7200 语料（23584/5902 SCTN、/5899、/5901 PJOI、/5904 SJOI）：不在 /ALL 内，本轮零接触；失败导出不作证据。
- 首次写会话 +2048B 页语义：未解析页内记录内容（属性层已证明对 /68 无可观察影响）。
- 崩溃根因：锁定同址 AV + 段落定位，未做反汇编级归因。
