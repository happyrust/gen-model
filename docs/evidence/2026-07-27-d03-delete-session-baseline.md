# D-03 真实删除会话：执行前基线（2026-07-27）

对应 `2026-07-25_test-plan-core-dll-model-update-complete-matrix-v2.md` §4 批次 D 的 D-03。
§1–§3 是**执行前**的库状态与目标选型，§4 是执行记录与逐项对拍。会话已于当日 13:59 执行。

## 1. 为什么补这一项

`2026-07-27-projams-incremental-update-validation.md` 的 D-01～D-15 审计表里，D-03 的
「尚缺证据」写的是「E3D 实际 Deleted session」。把六个变化桶（v2 §1.2）对照真实会话清点，
只有 Deleted 是空的：

| 变化桶 | 真实 E3D 会话 |
|---|---|
| Modified | 8000/25-26 `BOX.XLEN`、8000/27-28 `FTUB.POS`、7997/75 `WALL.JUSL`、7997/82 `DAMP.NAME` |
| Created | 8000/21 两个 `GENSEC` Add |
| Moved | 8000 sesno 31/32（SAVEWORK） |
| Reordered | 8000 sesno 33/34（SAVEWORK） |
| MemberChanged | 随 Created / Moved 一并覆盖 |
| **Deleted** | **无** |

`live_real_ftub_delete_move_and_reorder`（`increment_pipeline.rs`）确实跑了真实文件窗口，
但删除部分是 `FTUB 24384/30939` 的瞬态 Add→Deleted。FTUB 是伪类型（v2 §2.4）、BRAN 内的
组件，删它只调度所属 BRAN，不覆盖「元素自身带模型且被删」的清理路径。

## 2. 选定目标

`BRAN 24381/107104`（`/Copy-(2)-of-1TFM055MN-TUBE/941VL`，dbnum 7997），46 个子件全部已
生成模型。选它的另一个理由是它的子件里有三个**从未被任何用例覆盖过的变化等价类**代表
（v2 §2.3）：

| refno | noun | changeType 等价类 | 该类此前覆盖 | SPRE | inst_relate |
|---|---|---|---|---|---:|
| `24381/107146` | VTWA | `INLINE`（FILT/TRAP/VALV/VTWA） | 无 | `pe:13246_525801` | 1 |
| `24381/107145` | UNIO | `PCONN`（COUP/FLAN/GASK/WELD/UNIO…） | 无 | `pe:13246_525570` | 1 |
| `24381/107148` | TEE | `TEE`（BRCO/CROS/OLET/PTAP/TEE/THRE） | 无 | `pe:13246_525285` | 1 |
| `24381/107118` | CROS | `TEE` | 无 | `pe:13246_525299` | 1 |
| `24381/107147` | REDU | `LINEAR` | FTUB 已覆盖 | `pe:13246_525417` | 1 |

已执行的 D-01 用的是 `SPCO 23274/295504` 的 72 个 DAMP 消费者，而 DAMP 属于 `MULTC` 类，
并非计划 D-01 行要求的 `PCONN`/`INLINE`。删除目标选 VTWA 或 UNIO，可让这两个类第一次拿到
真实证据。

## 3. 执行前基线

隔离库 `127.0.0.1:8009`，NS `1516` / DB `AvevaMarineSample`。

| 项 | 值 |
|---|---:|
| `dbnum_watermark:7997` `file_latest_sesno` | 83 |
| `dbnum_watermark:7997` `applied_sesno` | 83 |
| `BRAN 24381/107104` 子件数（`pe_owner`） | 46 |
| `BRAN 24381/107104` 子件 `inst_relate` 数 | 46 |
| 子件 noun 分布 | ATTA 18 / BEND 23 / CROS 1 / REDU 1 / TEE 1 / UNIO 1 / VTWA 1 |
| `ref_rev` 总边数 | 91,459 |
| `ref_rev` 中以 `24381/107146` 为端点的边 | 0 |
| `ref_rev` 中以 `24381/107145` 为端点的边 | 0 |
| `inst_relate` 全库 | 1,097 |

`ref_rev` 边形如 `ref_rev:[pe:<被引用>, pe:<引用者>]`。目标元素当前没有反向边，所以
「删除后反向边清零」这条断言在本例中是弱断言，需要在结论里如实标注，不能当作级联清理
已验证。

## 4. 执行记录（2026-07-27 13:59）

### 4.1 会话怎么产生的

三条驱动通道里只有一条真的能用，这里如实记下，省得下次再试一遍：

| 通道 | 结论 |
|---|---|
| `des.exe < 文本文件`（stdin 重定向） | **不可用**。core.dll 自己 `CreateProcess` 出 `pdmsconsole.exe` 并把自身 std 句柄重指到那条管道上，喂进去的 stdin 被丢弃。`output/slots_stdin_run.txt` 有 42KB 启动输出，但 `$P GMSTDIN-ALIVE` 标记一次都没打印 |
| `console_inject.ps1`（AttachConsole 写 CONIN$） | **未验证可用**。`output/console_inject.log` 最近一次是 `AttachConsole=False err=6` |
| `AVEVA_DESIGN_ENTRYMACRO` | **可用**，本次采用。该变量由 `Startup.dll` 读取，会在事件循环起来后排入宏——宏成了会话启动的一部分，不是往会话里注入 |

装载路径必须用 C: 那份：`D:\AVEVA\Everything3D3.1` 的 `Startup.dll` 被改过（770048 vs
774656 字节），会话起来没有 CAF、没有命令循环。C: 的 `custom_evars.bat` 已经把
projects_dir 指向 `D:\AVEVA\Projects\E3D3.1`，ams 工程在这边同样可见。

新增 `scripts/e3d/run_ams_c_entrymacro.bat` 封装这条链路，
`scripts/e3d/projams_d03_probe.mac` 是配套的只读干跑。

先跑只读探针再跑破坏性宏：探针回报 `Type VTWA`、`Owner
/Copy-(2)-of-1TFM055MN-TUBE/941VL`、`Spref /YK-SS/1ML21:YKS-AFBCEBNR`，与 §2 完全一致；
跑完 `ams7997_0001` 与备份逐字节比对**只差 1 个字节**（偏移 `0x17F`，页 0 文件头的占用
标志），文件大小不变，确认只读会话不产生会话号。

### 4.2 删除会话

| 项 | 值 |
|---|---|
| 宏 | `scripts/e3d/projams_incr_delete_apply.mac`（作为入口宏，`QUIT` 收尾） |
| 宏日志 | `scripts/e3d/projams_incr_delete_apply.log` |
| 会话号 | **84** |
| `SAVEWORK` 注释 | `CODEX D-03 delete VTWA 24381/107146` |
| 删除后 `Q CE` | `/Copy-(2)-of-1TFM055MN-TUBE/941VL`（CE 上浮到属主，元素确已消失） |
| 文件 | 57,886,720 → 57,946,112 字节 |

`preview` 对 84 号窗口的解析：`+0 新增 / ~1 修改 / -1 删除`，`model_affecting=2`，
生成单元 `BRAN`。修改的那一笔是属主 BRAN 的成员列表变化，与设计预期一致。

### 4.3 逐项对拍

| 项 | 基线（sesno 83） | 执行后（sesno 84） |
|---|---:|---:|
| `pe:24381_107146` `deleted` | false | **true** |
| `inst_relate:24381_107146` | 存在 | **不存在** |
| `ref_rev` 以其为端点的边 | 0 | 0 |
| `pe_owner` 指向它的边 | 1 | **0** |
| `BRAN 24381/107104` 子件数 | 46 | **45** |
| 其中有模型的子件数 | 46 | **45** |
| `dbnum_watermark:7997` `applied_sesno` | 83 | **84** |

旧生成根 `BRAN 24381/107104` 经产品入口重生成，`execute_manual_update` 返回
`status=success`、`units=[{7997, 24381/107104, BRAN, generated}]`、`batches=[]`。
紧接着再执行一次返回 `status=up_to_date`，批次与单元均为空。
`model_update_pending` / `increment_update_attempt` / `incr_side_effect_pending` 全为 0。

§3 标注的弱断言仍然成立：该 VTWA 执行前在 `ref_rev` 里就没有反向边，所以「删除后反向边
清零」在本例中不构成级联清理已验证的证据。

### 4.4 一处断言层次错位（已修正）

`live_real_delete_session_cleans_up_model_and_regenerates_branch` 最初的收尾断言要求
「同区间重放后待办队列为空」，实测失败。核对后这是断言写错了，不是产品缺陷：

`IncrementPipeline::apply` 是**不带水位闸门**的底层原语。崩溃恢复本就要求它能把一个已
部分落库的固定区间原样重跑；而两个生产调用方
（`manual_update::execute_one_dbnum`、`increment_manager::execute_incr_update`）的区间
都是从水位算出来的，谁都不会拿已应用的窗口去调它。水位闸门在手动更新编排层——上面
`up_to_date` 那次执行正是它在起作用。

同文件 `live_real_ftub_delete_move_and_reorder` 早就把约定写死成「重放固定区间**不得
重复**建立模型工作」，比对的是前后数量而非归零。断言已按同一约定改写为：重放必须按
同样的键重建同一批工作（`delete_cleanup 24381/107146` + `regen_root 24381/107104`，
不多不少），排空后队列为空，且墓碑、`inst_relate`、水位、子件数全部与首次一致。

### 4.5 回滚：已决定不回滚

**这次删除按决定予以保留，文件与隔离库都停在 sesno 84。** 下面的备份只作为保险留存，
不要因为看到它们就顺手还原。

删除不可逆、refno 不会被重新发放。真要回滚只能靠删除前的文件备份
`ams000/ams7997_0001.codex-before-d03-delete-20260727`（57,886,720 字节，停在 sesno 83），
且必须在 E3D 不持有该库 claim 时替换。**代价**：还原文件只会把文件退回 83，隔离库里的
删除结果不会跟着回退，必须对 dbnum 7997 重新同步才能让两边重新一致；同时 D-03 的真实
会话证据会从文件里消失，要复验就得重做一次删除。会话登记库另存有
`ams000/amscom.codex-before-d03-relaunch-20260727`。
