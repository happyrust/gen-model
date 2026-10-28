# PML 增量测试套件设计

本文件由两批内容拼成：
- **第一批**（第 1 节至 2.1 节）：覆盖矩阵、全局约定、`common/`、`create/`、`delete/` 前三条——由 Lead 从 CT 第一段回复补齐。
- **第二批**（2.2 节至第五部分）：CT 窗口（claude-fable-5-1-thinking-max）直接落盘，宏头注带 `登记/语义/expected_net/restore 搭档` 与来源行号。

---

# 第一部分 覆盖矩阵（56 条）

分类：**a** = 现有宏可触发；**b** = 需新宏；**c** = PML 触发不了。
登记表行号指 `tests/fixtures/increment_cases.json`。

| ID | 增量语义 | 类 | 触发 | 断言 |
|---|---|---|---|---|
| A01 :6 | FTUB 位移/增减成员/删 BRAN/删 PIPE | a+b | `db8000_bran_scratch_*` 5 腿链（`db8000_bran_scratch_create.mac:8-13`）；本套 connect/pipe 链 | 根集=该 BRAN；删 BRAN → DeleteCleanup；删 PIPE → 每 BRAN 一条 |
| A02 :7 | 设备子件增删改移/整台复制/删除/跨设备改挂 | a+b | `db8000_equi_copy_apply.mac`、`increment_fixture` owner/add/delete；本套 create/delete/attr | 根只 regen 一次；复制=新根；删整台=1 DeleteCleanup（`equi_increment.rs:132-187`） |
| A03 :8 | HVAC 风管件增删改移 | a | `hvac_100817_damp_pos/desp_apply+restore` | 容器不进根集 |
| A04 :9 | CTRAY 初始导入窗口/阈值 | c | 阈值是配置，PML 只能造保存不能造阈值 | `test:ctray_increment` |
| A05 :10 | 属性字典 totality | c | 纯 lib | — |
| A06 :11 | 只记账会话 → no_model | a | `replay/equi_login.mac`（只登录，:1-3） | `unit_count==0`（`main.rs:271-273`） |
| A07 :12 | NAME/DESC/PURP 不重算 | a+b | `data_apply.mac`（NAME）；本套 `attr/` | 任务 succeeded 且 `unit_count==0`，模型行不变 |
| A08 :13 | 钉死真窗口分桶 | c | 真窗口语料；本套 compound 链可补新样本 | lib |
| B01 :15 | 跨会话父子删除 | a+b | `db8000-two-delete` 夹具；本套 `delete/` 2 腿 | 净窗口=终态，只一条 DeleteCleanup |
| B02-B05 :16-19 | 会话对/基线流/读透同源/双全量 | c | 工具层性质 | — |
| B06 :20 | 回退 → reinitialize | c | E3D 没有「回退 DB」；只能 `replay-step --rollback`（`main.rs:1682`） | — |
| B07 :21 | 同路径换文件 | c | 已撤回/待定；文件层 | — |
| C01/C02 :23-24 | republish_gate / 路径前缀 | c | lib | — |
| C03 :25 | regen→位移→transform | a | `replay/equi_apply.mac:12` `BY U 500mm`；本套 `xform/move` | `unit_kinds=transform`、`aabb_shift`、行数不变 |
| C04 :26 | 登录一次再位移仍 transform | a | `equi_login` + `equi_apply` | 同上 |
| C05 :27 | 整条 PIPE 位移 | b | 本套 `pipe/whole_move` | 每 BRAN `unit_kinds=regen`（刚体拒），O1 RVM 对拍 |
| C06 :28 | 带 NOZZ 的 EQUI 位移 | b | 本套 `create/equi_copy`（带 NOZZ）+ `xform/move` 于其上 | `unit_kinds=transform`，日志无 `[模型目标核对]` |
| C07 :29 | 目录项摘要跨登录不漂 | c | 要目录库语料 | — |
| C08 :30 | 只动方位 | a+b | `replay/equi_rotate_apply.mac:21`；本套 `xform/rot_u/e/n` + compound | transform；`rebuild_records_equal` |
| D01 :32 | 只登录不改 | a | `equi_login` | `generate_already_available`，预算 |
| D02/D03/D05 :33-34,:36 | 文件身份/回执承诺/房间点查 | c | lib | — |
| D04 :35 | 未触根跨窗仍 AlreadyAvailable | a | move-chain（另一根不动） | `generate_already_available` |
| E01/E02/E03/E05 :38-40,:42 | 目录落款分域/stamp_only/域外 | c | 落款只能 `seed` SQL 造（`main.rs:81-86`） | — |
| E04 :41 | 目录件真改→反查引用者 | c | 要 PARAGON 写目录库，db 13244 未定位（`db8000_pipe_spre_swap_apply.mac:7-8`） | — |
| F01 :44 | 整片删换老行 | c | lib | — |
| F02 :45 | 冷重启对拍 | c | harness restart（`main.rs:136`），PML 无关 | — |
| F03 :46 | /tasks 历史 | a | 任何链 | `history_root` |
| F04 :47 | 单元回执几何差 | a | `xform/move` | `geometry_moved`/`aabb_shift` |
| F05 :48 | 生成失败不推 model_sesno | a | 任意 apply + `AIOS_TEST_FAIL_ROOTS` | `expect_model_sesno_stay` |
| G01 :50 | 边集==重建 | a | room 链 | `rebuild_edges_equal` |
| G02 :51 | 跨房位移 | a | `replay/equi_room_apply`；本套 `room/member_move` | `rooms_changed`/`room_edges` |
| G03 :52 | PANE/FRMW 变 → room_rebuild | a+b | `replay/room_pane_apply.mac:13` `BY D 200mm`（宏在、夹具未 pack）；本套 `room/pane_move`/`frmw_rename` | 一条 room_rebuild 任务，边数同全量 |
| G04 :53 | 点查不扫表 | c | lib | — |
| H01 :55 | 保存中途被看见 | c（文件层） | 「先页后头」只有 `replay-step --half` 造得出（`main.rs:1674-1675`）；PML 任何 apply 可当被追的那窗 | race |
| H02 :56 | 数据失败 park | a | 任意 apply + `AIOS_TEST_FAIL_DATA_ENDS` | `expect_batch_parked` |
| H03 :57 | 一库失败不阻另一库 | b | 本套 `h03/` 链 + companions | 见 2.10 节 |
| H04 :58 | 回退重建 | c | 同 B06 | — |
| H05 :59 | 生成失败保留重试 | a | 同 F05 + `clear_fail_injection` | — |
| I01/I02/I04/I05 :61-65 | 性能预算 | a | 现有链自带 `budget_gate` | `increment_budget.py` |
| I03 :63 | SYS 批次 ≤ 2 s | a | `equi_login`（登录也给 SYST 库写 DBMDAT） | 还缺门（todo 是门不是触发） |
| I06 :66 | room.lookup ≤ 100 ms | c | API 计时 | — |
| J01-J03 :68-70 | API 契约 | c | API | — |

**统计：a 16、b 8、c 32。**

`b` 的 8 条：A01 / A02 / A07 / B01 / C05 / C06 / C08 / G03 / H03（其中 A01、A02 部分可用现有宏）。

**关键结论：(c) 类 32 条里，没有一条是「元件增删改」本身。** 它们全是文件层（身份、会话对、回退）、工具层（`/tasks`、`/dbnums`、`/health` 契约）、目录库语料（Paragon、13244）、或 API 计时。也就是说——**PML 能覆盖的元件增删改面，就是 a+b 这 24 条，而这 24 条全部可以用 PML 造出来。**

---

# 第二部分 宏套件

## 全局约定

- 目录：`scripts/e3d/increment_suite/<group>/<name>.mac`
- 元素名前缀：`/AIOS-SUITE-`
- 导航：一律按名字（`db8000_recording_cases.json:9` 的教训：db8000 的 TTY 会话里按 refno 导航不成立）
- 纪律（每腿必须满足，`Record-Db8000SessionChain.ps1:74-93` 会静态审查）：
  1. 恰好一个 `SAVEWORK`
  2. 不得自带 `QUIT` / `FINISH`（会话归 driver 的 wrapper）
  3. `Q REF` 与 `Q NAME` 成对（脚本靠这对相邻输出把名字解析成 refno，`:99-110`）
  4. `ALPHA LOG` 落在宏旁同名 `.log`（不闭合则整段日志不落盘）
- `★` = 需真机验证
- 被改的元素全部是 setup 自建的 scratch（理由见 `db8000_scratch_create.mac:4-8`：生产元素的当前 POS/XLEN 离线不可知，自建元素才能让 before/after 全部自决，restore 才能精确）
- `LOGDIR` = `D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite`

## 2.0 common/

### `common/setup_8000.mac`

一次 SAVEWORK 造全套基线；幂等（先删同名 SITE，同 `setup.mac:3-8`）。

```
-- 基线：SITE /AIOS-SUITE-8000 下一个 ZONE；EQ-A(+BOX-A1)、EQ-B(空)、EQ-NBX(BOX-N 下挂 NBOX)、EQ-NOZ（COPY 生产带 NOZZ 设备）、
-- PIPE-P（COPY 生产 PIPE）、BRAN-S（COPY /C-OR-1R345-C）、房间 STRU/FRMW R201,R202 + EQ-RM。
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/common/setup_8000.log" OVER
$P SUITE-SETUP-8000-ALIVE
/*
VAR !old COLLECT ALL SITE WITH (MATCHWILD(NAME,'/AIOS-SUITE-8000*')) FOR CE
DO !s VALUES !old
  !!CE = !s.dbref()
  DELETE SITE
ENDDO
=24384/0                                   $* ★db8000 的 WORL refno：用 preflight.json 的 world_refno 替换（fixture.rs:1056-1060）
NEW SITE /AIOS-SUITE-8000
NEW ZONE /AIOS-SUITE-8000-Z
!zone = !!CE
NEW EQUIPMENT /AIOS-SUITE-EQ-A
POS E 40000 N 5000 U 0
Q REF
Q NAME
NEW BOX XLEN 1000 YLEN 1000 ZLEN 1000
NAME /AIOS-SUITE-BOX-A1
Q REF
Q NAME
!!CE = !zone
NEW EQUIPMENT /AIOS-SUITE-EQ-B
POS E 43000 N 5000 U 0
Q REF
Q NAME
!!CE = !zone
NEW EQUIPMENT /AIOS-SUITE-EQ-NBX
POS E 46000 N 5000 U 0
Q REF
Q NAME
NEW BOX XLEN 2000 YLEN 2000 ZLEN 2000
NAME /AIOS-SUITE-BOX-N
Q REF
Q NAME
NEW NBOX XLEN 500 YLEN 500 ZLEN 2500      $* setup.mac:42-46 / s4_db8000_nbox_in_box_create.mac:30 同款
NAME /AIOS-SUITE-NBOX-N1
Q REF
Q NAME
!!CE = !zone
NEW EQUIPMENT /AIOS-SUITE-EQ-NOZ
COPY /1-LNR-Q005-PJ                        $* db8000_equi_copy_apply.mac:7-8 同款；★该设备确有 NOZZ（看 Q MEMB）
Q REF
Q NAME
Q MEMB
BY E 49000                                 $* ★COPY 后是否保留源 POS；挖开距离避免与源设备重叠影响房间归属
!!CE = !zone
NEW PIPE /AIOS-SUITE-PIPE-P
COPY /1RX-345                              $* ★PIPE 级 COPY 连带全部 BRAN（db8000_pipe_scratch_a_create.mac:1 提到该 PIPE）
Q REF
Q NAME
Q MEMB
NEW BRAN /AIOS-SUITE-BRAN-S
COPY /C-OR-1R345-C                         $* db8000_bran_scratch_create.mac:28-29 同款（7 FTUB + 2 BEND）
Q REF
Q NAME
Q MEMB
!!CE = !zone
NEW STRU /AIOS-SUITE-ROOM-STRU
!stru = !!CE
NEW FRMW /AIOS-SUITE-RM-R201
NEW PANE /AIOS-SUITE-RM-R201-P1
NEW PLOO
HEIG 2000
NEW PAVE
POS E 58000 N 3000 U 0
NEW PAVE
POS E 62000 N 3000 U 0
NEW PAVE
POS E 62000 N 7000 U 0
NEW PAVE
POS E 58000 N 7000 U 0
!!CE = !stru
NEW FRMW /AIOS-SUITE-RM-R202
NEW PANE /AIOS-SUITE-RM-R202-P1
NEW PLOO
HEIG 2000
NEW PAVE
POS E 68000 N 3000 U 0
NEW PAVE
POS E 72000 N 3000 U 0
NEW PAVE
POS E 72000 N 7000 U 0
NEW PAVE
POS E 68000 N 7000 U 0
!!CE = !zone
NEW EQUIPMENT /AIOS-SUITE-EQ-RM
POS E 60000 N 5000 U 1000
Q REF
Q NAME
NEW BOX XLEN 500 YLEN 500 ZLEN 500
NAME /AIOS-SUITE-BOX-RM
Q REF
Q NAME
SAVEWORK 'AIOS suite: baseline setup db8000'
$P SUITE-SETUP-8000-DONE
ALPHA LOG END
```

### `common/teardown_8000.mac`

```
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/common/teardown_8000.log" OVER
$P SUITE-TEARDOWN-8000-ALIVE
/*
VAR !sites COLLECT ALL SITE WITH (MATCHWILD(NAME,'/AIOS-SUITE-8000*')) FOR CE
DO !s VALUES !sites
  !!CE = !s.dbref()
  Q REF
  Q NAME
  DELETE SITE
ENDDO
SAVEWORK 'AIOS suite: teardown db8000'
$P SUITE-TEARDOWN-8000-DONE
ALPHA LOG END
```

### `common/probe_8000.mac`

只读、无 SAVEWORK，单走 `l3_suite --check-driver`；产出 name→refno 表。

```
ONERROR CONTINUE
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/common/probe_8000.log" OVER
$P SUITE-PROBE-8000-ALIVE
/AIOS-SUITE-8000
VAR !all COLLECT ALL FOR CE
DO !e VALUES !all
  !!CE = !e.dbref()
  Q REF
  Q NAME
  Q TYPE
  Q OWNE
ENDDO
$P SUITE-PROBE-8000-DONE
ALPHA LOG END
```

### `common/setup_7998.mac` / `common/teardown_7998.mac`

与 8000 同形，只建 `/AIOS-SUITE-7998` + ZONE + `/AIOS-SUITE-7998-EQ`(+BOX)，WORL 换成 7998 的（★用 `db_session_fixture inspect` 或 preflight 取）。

## 2.1 create/（登记：A02 / A01 / F03）

### `create/box_apply.mac` — net: cancelled

```
-- A02「增子件」：EQ-A 下新 BOX → 根 EQ-A regen 一次，行数 +1。
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/create/box_apply.log" OVER
$P SUITE-CREATE-BOX-APPLY-ALIVE
/AIOS-SUITE-EQ-A
NEW BOX XLEN 700 YLEN 800 ZLEN 900
NAME /AIOS-SUITE-BOX-A2
POS E 1500 N 0 U 0
Q REF
Q NAME
Q OWNE
SAVEWORK 'AIOS suite create/box apply'
$P SUITE-CREATE-BOX-APPLY-DONE
ALPHA LOG END
```

### `create/box_restore.mac`

```
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/create/box_restore.log" OVER
$P SUITE-CREATE-BOX-RESTORE-ALIVE
VAR !b COLLECT ALL BOX WITH (NAME EQ '/AIOS-SUITE-BOX-A2')
DO !x VALUES !b
  !!CE = !x.dbref()
  Q REF
  Q NAME
  DELETE BOX
ENDDO
SAVEWORK 'AIOS suite create/box restore'
$P SUITE-CREATE-BOX-RESTORE-DONE
ALPHA LOG END
```

### `create/subtree_apply.mac` — net: cancelled

```
-- A02/F03：新 EQUI 根带两个 BOX → created_subtree_roots=1，一次 regen，/tasks/{id}/changes 每条有 root_refno。
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/create/subtree_apply.log" OVER
$P SUITE-CREATE-SUBTREE-APPLY-ALIVE
/AIOS-SUITE-8000-Z
NEW EQUIPMENT /AIOS-SUITE-EQ-NEW
POS E 52000 N 5000 U 0
Q REF
Q NAME
NEW BOX XLEN 1000 YLEN 600 ZLEN 400
NAME /AIOS-SUITE-BOX-NEW1
Q REF
Q NAME
/AIOS-SUITE-EQ-NEW
NEW BOX XLEN 400 YLEN 400 ZLEN 1200
NAME /AIOS-SUITE-BOX-NEW2
POS E 1200 N 0 U 600
Q REF
Q NAME
SAVEWORK 'AIOS suite create/subtree apply'
$P SUITE-CREATE-SUBTREE-APPLY-DONE
ALPHA LOG END
```

### `create/subtree_restore.mac` — 兼作「删整台」用例（一条 DeleteCleanup）

```
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/create/subtree_restore.log" OVER
$P SUITE-CREATE-SUBTREE-RESTORE-ALIVE
VAR !t COLLECT ALL EQUI WITH (NAME EQ '/AIOS-SUITE-EQ-NEW')
DO !x VALUES !t
  !!CE = !x.dbref()
  Q REF
  Q NAME
  Q MEMB
  DELETE EQUI
ENDDO
SAVEWORK 'AIOS suite create/subtree restore (delete unit)'
$P SUITE-CREATE-SUBTREE-RESTORE-DONE
ALPHA LOG END
```

### `create/equi_copy_apply.mac` — net: cancelled

```
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/create/equi_copy_apply.log" OVER
$P SUITE-CREATE-EQCOPY-APPLY-ALIVE
/AIOS-SUITE-8000-Z
NEW EQUIPMENT /AIOS-SUITE-EQ-COPY
COPY /AIOS-SUITE-EQ-NOZ
BY E 3000
Q REF
Q NAME
Q MEMB
SAVEWORK 'AIOS suite create/equi_copy apply'
$P SUITE-CREATE-EQCOPY-APPLY-DONE
ALPHA LOG END
```

### `create/equi_copy_restore.mac`

同 `subtree_restore`，名字换 `/AIOS-SUITE-EQ-COPY`。

### `create/bran_copy_apply.mac` — net: cancelled

```
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/create/bran_copy_apply.log" OVER
$P SUITE-CREATE-BRANCOPY-APPLY-ALIVE
/AIOS-SUITE-PIPE-P
NEW BRAN /AIOS-SUITE-BRAN-NEW
COPY /AIOS-SUITE-BRAN-S
Q REF
Q NAME
Q HPOS
Q TPOS
Q MEMB
SAVEWORK 'AIOS suite create/bran_copy apply'
$P SUITE-CREATE-BRANCOPY-APPLY-DONE
ALPHA LOG END
```

### `create/bran_copy_restore.mac`

按名 `COLLECT ALL BRAN` → `DELETE BRAN`。

### `create/cross_db_copy_apply.mac` — 跨库移动（net: 8000 deleted / 7998 added）

同一 SAVEWORK 写两个库文件。要求 MDB 里 7998 可写。

```
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/create/cross_db_copy_apply.log" OVER
$P SUITE-CROSSDB-APPLY-ALIVE
/AIOS-SUITE-7998-Z
NEW EQUIPMENT /AIOS-SUITE-EQ-X
COPY /AIOS-SUITE-EQ-A                      $* ★跨库 COPY：源在 8000、CE 在 7998，COPY 按 MDB 解名
Q REF
Q NAME
Q MEMB
/AIOS-SUITE-EQ-A
Q REF
Q NAME
DELETE EQUI
SAVEWORK 'AIOS suite cross-db: EQ-A copied into 7998 and deleted from 8000'
$P SUITE-CROSSDB-APPLY-DONE
ALPHA LOG END
```

### `create/cross_db_copy_restore.mac`

对称：在 8000 的 ZONE `NEW EQUIPMENT /AIOS-SUITE-EQ-A` + `COPY /AIOS-SUITE-EQ-X`，再 `/AIOS-SUITE-EQ-X` `DELETE EQUI`。
注意：恢复后 EQ-A 是新 refno，后续链一律按名解。

## 2.2 delete/（前半，接 CT 第二批的 `container_keep_child_restore` 之前）

### `delete/box_apply.mac` — net: deleted

restore 重建同名同尺寸 → 按名 cancelled，按 refno 不可逆。

```
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/delete/box_apply.log" OVER
$P SUITE-DELETE-BOX-APPLY-ALIVE
/AIOS-SUITE-BOX-A1
Q REF
Q NAME
Q OWNE
DELETE BOX
SAVEWORK 'AIOS suite delete/box apply'
$P SUITE-DELETE-BOX-APPLY-DONE
ALPHA LOG END
```

### `delete/box_restore.mac`

```
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/delete/box_restore.log" OVER
$P SUITE-DELETE-BOX-RESTORE-ALIVE
/AIOS-SUITE-EQ-A
NEW BOX XLEN 1000 YLEN 1000 ZLEN 1000
NAME /AIOS-SUITE-BOX-A1
Q REF
Q NAME
SAVEWORK 'AIOS suite delete/box restore (recreate)'
$P SUITE-DELETE-BOX-RESTORE-DONE
ALPHA LOG END
```

### `delete/container_keep_child_apply.mac`

删中间容器保留子件：`INCLUDE` 走 + `DELETE` 旧容器，同窗。预期 EQ-A DeleteCleanup + EQ-B regen。
`INCLUDE` 语法见 `db8000_pipe_reparent_apply.mac:29-30`（别写 `OWNER /X`，`:6-9` 的教训）。

```
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/delete/container_keep_child_apply.log" OVER
$P SUITE-DELETE-KEEPCHILD-APPLY-ALIVE
/AIOS-SUITE-BOX-A1
!box = !!CE
Q REF
Q NAME
/AIOS-SUITE-EQ-B
INCLUDE $!box
$!box
Q OWNE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
DELETE EQUI
SAVEWORK 'AIOS suite delete/container_keep_child apply: BOX-A1 moved to EQ-B, EQ-A deleted'
$P SUITE-DELETE-KEEPCHILD-APPLY-DONE
ALPHA LOG END
```

---

# 第二批（CT 直接落盘，2.2 节续至第五部分）

全套约定（与第一段一致）：

- 目录 `scripts/e3d/increment_suite/<group>/<name>.mac`；日志落在宏旁同名 `.log`（`Record-Db8000SessionChain.ps1:99-100` 按 `.log` 回读 refno）。下文 `LOGDIR` 一律展开成字面路径 `D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite`。
- 元素名前缀 `/AIOS-SUITE-`；按名字导航（`scripts/e3d/db8000_recording_cases.json:9`：db8000 TTY 里 refno 导航不成立）。
- 每腿恰好一个 SAVEWORK、无 QUIT/FINISH、`Q REF` / `Q NAME` 成对（`Record-Db8000SessionChain.ps1:74-93` 的审查）。
- 每条宏头注：`登记` ID、`语义`、`expected_net`、`restore` 搭档。
- ★ = 需真机验证的语句；其余语句在仓内已跑过的宏里都有同款（头注里给出来源）。
- 基线由 `common/setup_8000.mac`（第一段）建立：SITE `/AIOS-SUITE-8000` → ZONE `/AIOS-SUITE-8000-Z`；`EQ-A`（POS E 40000 N 5000 U 0，子件 `BOX-A1` 1000³）、`EQ-B`（E 43000）、`EQ-NBX`（E 46000，`BOX-N` 2000³ 下 `NBOX-N1` 500×500×2500）、`EQ-NOZ`（COPY /1-LNR-Q005-PJ 后 BY E 49000）、`PIPE-P`（COPY /1RX-345）、`BRAN-S`（COPY /C-OR-1R345-C，7 FTUB + 2 BEND）、房间 `ROOM-STRU` 下 FRMW `RM-R201`（PANE `RM-R201-P1`，轮廓 E 58000..62000 / N 3000..7000）与 `RM-R202`（E 68000..72000）、`EQ-RM`（E 60000 N 5000 U 1000，子件 `BOX-RM` 500³）。

---

## 2.2 delete/（续）

### delete/container_keep_child_restore.mac

```
-- 登记: A02 | 语义: 把「删容器保子件」还原——重建 EQ-A（新 refno），把 BOX-A1 INCLUDE 回来 | expected_net: cancelled（按名；EQ-A 的 refno 已变） | restore 搭档: 本文件是 delete/container_keep_child_apply.mac 的 restore
-- INCLUDE 写法: scripts/e3d/db8000_pipe_reparent_apply.mac:29-30（别写 OWNER /X，:6-9 的教训）
-- 预期 gen-model: EQ-A 新根 regen；EQ-B 失去成员 regen（两根一窗）
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/delete/container_keep_child_restore.log" OVER
$P SUITE-DELETE-KEEPCHILD-RESTORE-ALIVE
/AIOS-SUITE-8000-Z
NEW EQUIPMENT /AIOS-SUITE-EQ-A
POS E 40000 N 5000 U 0
Q REF
Q NAME
/AIOS-SUITE-BOX-A1
!box = !!CE
Q REF
Q NAME
Q OWNE
/AIOS-SUITE-EQ-A
INCLUDE $!box
/AIOS-SUITE-BOX-A1
Q OWNE
SAVEWORK 'AIOS suite delete/container_keep_child restore'
$P SUITE-DELETE-KEEPCHILD-RESTORE-DONE
ALPHA LOG END
```

### delete/recreate_same_name_apply.mac

```
-- 登记: A02（补缺口「删后同名重建」，登记表无单独 ID） | 语义: 同一 SAVEWORK 里删 BOX-A1 再建同名同尺寸的 BOX-A1（新 refno） | expected_net: modified（同名新 refno；按 refno 是 deleted + added） | restore 搭档: 无（终态与基线按名相等）
-- 预期 gen-model: datacenter_delete reason=deleted 指旧 refno；新 refno 有模型行；EQ-A regen 一次。后续链按名重解 refno。
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/delete/recreate_same_name_apply.log" OVER
$P SUITE-DELETE-RECREATE-APPLY-ALIVE
/AIOS-SUITE-BOX-A1
Q REF
Q NAME
DELETE BOX
/AIOS-SUITE-EQ-A
NEW BOX XLEN 1000 YLEN 1000 ZLEN 1000
NAME /AIOS-SUITE-BOX-A1
Q REF
Q NAME
Q OWNE
SAVEWORK 'AIOS suite delete/recreate_same_name apply'
$P SUITE-DELETE-RECREATE-APPLY-DONE
ALPHA LOG END
```

### delete/child_then_parent_1_apply.mac

```
-- 登记: B01 | 语义: 跨会话父子删除的第 1 腿——先删子件 BOX-A1 | expected_net: deleted（与第 2 腿合计后净窗口 = 整根 deleted） | restore 搭档: delete/child_then_parent_restore.mac（两腿共用一条 restore）
-- 来源: scripts/e3d/increment_fixture/cases/delete_apply.mac:2-6
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/delete/child_then_parent_1_apply.log" OVER
$P SUITE-DELETE-CTP1-APPLY-ALIVE
/AIOS-SUITE-BOX-A1
Q REF
Q NAME
Q OWNE
DELETE BOX
SAVEWORK 'AIOS suite delete/child_then_parent leg 1: delete BOX-A1'
$P SUITE-DELETE-CTP1-APPLY-DONE
ALPHA LOG END
```

### delete/child_then_parent_2_apply.mac

```
-- 登记: B01 | 语义: 第 2 腿——再删父件 EQ-A；两会话的净窗口必须只有一条 DeleteCleanup（tests/equi_increment.rs:170 契约；db8000-two-delete 夹具同形） | expected_net: deleted | restore 搭档: delete/child_then_parent_restore.mac
-- 来源: scripts/e3d/increment_fixture/cases/delete_unit_apply.mac:2-6
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/delete/child_then_parent_2_apply.log" OVER
$P SUITE-DELETE-CTP2-APPLY-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
Q MEMB
DELETE EQUI
SAVEWORK 'AIOS suite delete/child_then_parent leg 2: delete EQ-A'
$P SUITE-DELETE-CTP2-APPLY-DONE
ALPHA LOG END
```

### delete/child_then_parent_restore.mac

```
-- 登记: B01 | 语义: 重建 EQ-A + BOX-A1（与 setup 同值） | expected_net: cancelled（按名；两个 refno 都变了） | restore 搭档: 本文件是 child_then_parent_1/2 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/delete/child_then_parent_restore.log" OVER
$P SUITE-DELETE-CTP-RESTORE-ALIVE
/AIOS-SUITE-8000-Z
NEW EQUIPMENT /AIOS-SUITE-EQ-A
POS E 40000 N 5000 U 0
Q REF
Q NAME
NEW BOX XLEN 1000 YLEN 1000 ZLEN 1000
NAME /AIOS-SUITE-BOX-A1
Q REF
Q NAME
Q OWNE
SAVEWORK 'AIOS suite delete/child_then_parent restore: recreate EQ-A + BOX-A1'
$P SUITE-DELETE-CTP-RESTORE-DONE
ALPHA LOG END
```

---

## 2.3 attr/（改名 / 改 DESC / 改属主）

### attr/rename_apply.mac

```
-- 登记: A07（元数据门） | 语义: 只改 NAME；预期任务 succeeded、unit_count 0、datacenter_delete reason=renamed 一条（increment_fixture manifest :262-269 同款） | expected_net: modified | restore 搭档: attr/rename_restore.mac
-- 来源: scripts/e3d/increment_fixture/cases/data_apply.mac:2-3
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/attr/rename_apply.log" OVER
$P SUITE-ATTR-RENAME-APPLY-ALIVE
/AIOS-SUITE-EQ-B
Q REF
Q NAME
NAME /AIOS-SUITE-EQ-B-RENAMED
Q REF
Q NAME
SAVEWORK 'AIOS suite attr/rename apply'
$P SUITE-ATTR-RENAME-APPLY-DONE
ALPHA LOG END
```

### attr/rename_restore.mac

```
-- 登记: A07 | 语义: 改回原名（refno 不变，E3D 层真逆；gen-model 层 renamed 删除记录仍在，fixture.rs:124-126 的 INSERT IGNORE 容忍） | expected_net: cancelled | restore 搭档: 本文件是 attr/rename_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/attr/rename_restore.log" OVER
$P SUITE-ATTR-RENAME-RESTORE-ALIVE
/AIOS-SUITE-EQ-B-RENAMED
Q REF
Q NAME
NAME /AIOS-SUITE-EQ-B
Q REF
Q NAME
SAVEWORK 'AIOS suite attr/rename restore'
$P SUITE-ATTR-RENAME-RESTORE-DONE
ALPHA LOG END
```

### attr/desc_apply.mac

```
-- 登记: A07 / A06 | 语义: 纯元数据（DESC）改动；预期任务 succeeded、unit_count 0、模型行不变、无 datacenter_delete | expected_net: modified | restore 搭档: attr/desc_restore.mac
-- setup 没有给 EQ-A 设 DESC，所以 restore 写回空串
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/attr/desc_apply.log" OVER
$P SUITE-ATTR-DESC-APPLY-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
Q DESC
DESC 'AIOS suite desc changed'
Q DESC
SAVEWORK 'AIOS suite attr/desc apply'
$P SUITE-ATTR-DESC-APPLY-DONE
ALPHA LOG END
```

### attr/desc_restore.mac

```
-- 登记: A07 | 语义: DESC 清回空 | expected_net: cancelled | restore 搭档: 本文件是 attr/desc_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/attr/desc_restore.log" OVER
$P SUITE-ATTR-DESC-RESTORE-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
DESC ''                                    $* ★空串写法；被拒就改 DESC 'AIOS baseline' 并在 setup 里同步设这个值
Q DESC
SAVEWORK 'AIOS suite attr/desc restore'
$P SUITE-ATTR-DESC-RESTORE-DONE
ALPHA LOG END
```

### attr/owner_apply.mac

```
-- 登记: A02（跨设备改挂） | 语义: BOX-A1 从 EQ-A 改挂到 EQ-B，refno 不变；预期两根都 regen（tests/equi_increment.rs:187） | expected_net: modified（owner） | restore 搭档: attr/owner_restore.mac
-- 来源: scripts/e3d/increment_fixture/cases/owner_apply.mac:2-3（INCLUDE 写法）
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/attr/owner_apply.log" OVER
$P SUITE-ATTR-OWNER-APPLY-ALIVE
/AIOS-SUITE-BOX-A1
!box = !!CE
Q REF
Q NAME
Q OWNE
/AIOS-SUITE-EQ-B
INCLUDE $!box
/AIOS-SUITE-BOX-A1
Q REF
Q NAME
Q OWNE
SAVEWORK 'AIOS suite attr/owner apply: BOX-A1 -> EQ-B'
$P SUITE-ATTR-OWNER-APPLY-DONE
ALPHA LOG END
```

### attr/owner_restore.mac

```
-- 登记: A02 | 语义: BOX-A1 改挂回 EQ-A | expected_net: cancelled | restore 搭档: 本文件是 attr/owner_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/attr/owner_restore.log" OVER
$P SUITE-ATTR-OWNER-RESTORE-ALIVE
/AIOS-SUITE-BOX-A1
!box = !!CE
Q REF
Q NAME
Q OWNE
/AIOS-SUITE-EQ-A
INCLUDE $!box
/AIOS-SUITE-BOX-A1
Q REF
Q NAME
Q OWNE
SAVEWORK 'AIOS suite attr/owner restore: BOX-A1 -> EQ-A'
$P SUITE-ATTR-OWNER-RESTORE-DONE
ALPHA LOG END
```

---

## 2.4 xform/（位移 / 三轴旋转 / 复合 / 带 NOZZ 整台位移）

基线方位：setup 没设 ORI，默认 `Y IS N AND Z IS U`（increment_fixture setup.mac:22-23 同样没设）。旋转腿写整份终态、不写反向增量（scripts/e3d/replay/equi_rotate_apply.mac:2 的纪律）。

### xform/move_apply.mac

```
-- 登记: C03 / F04 / D04 | 语义: 整根平移；预期 unit_kinds=transform、geometry_moved=1、aabb_shift [500,0,0]、行数不变（ADR-066） | expected_net: modified | restore 搭档: xform/move_restore.mac
-- 来源: scripts/e3d/replay/equi_apply.mac:12（BY）
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/xform/move_apply.log" OVER
$P SUITE-XFORM-MOVE-APPLY-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
Q POS WRT /*
BY E 500 WRT /*
Q POS WRT /*
SAVEWORK 'AIOS suite xform/move apply: EQ-A BY E 500'
$P SUITE-XFORM-MOVE-APPLY-DONE
ALPHA LOG END
```

### xform/move_restore.mac

```
-- 登记: C03 | 语义: 反向平移（同 binade 加减精确可逆，scripts/e3d/s4_aps_strflt_move.mac:7） | expected_net: cancelled | restore 搭档: 本文件是 xform/move_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/xform/move_restore.log" OVER
$P SUITE-XFORM-MOVE-RESTORE-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
Q POS WRT /*
BY W 500 WRT /*
Q POS WRT /*
SAVEWORK 'AIOS suite xform/move restore: EQ-A BY W 500'
$P SUITE-XFORM-MOVE-RESTORE-DONE
ALPHA LOG END
```

### xform/rot_u_apply.mac

```
-- 登记: C08 | 语义: 绕 U（铅垂）轴 +90°，POS 不动；预期 transform，/model/records 与整库重建逐条相等（四元数不分符号） | expected_net: modified | restore 搭档: xform/rot_u_restore.mac
-- 来源: scripts/e3d/replay/equi_rotate_apply.mac:21（ORI 写法）
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/xform/rot_u_apply.log" OVER
$P SUITE-XFORM-ROTU-APPLY-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
Q ORI WRT /*
ORI Y IS W AND Z IS U WRT /*
Q ORI WRT /*
SAVEWORK 'AIOS suite xform/rot_u apply: EQ-A +90 about U'
$P SUITE-XFORM-ROTU-APPLY-DONE
ALPHA LOG END
```

### xform/rot_u_restore.mac

```
-- 登记: C08 | 语义: 写回基线方位 | expected_net: cancelled | restore 搭档: 本文件是 xform/rot_u_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/xform/rot_u_restore.log" OVER
$P SUITE-XFORM-ROTU-RESTORE-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
Q ORI WRT /*
ORI Y IS N AND Z IS U WRT /*
Q ORI WRT /*
SAVEWORK 'AIOS suite xform/rot_u restore'
$P SUITE-XFORM-ROTU-RESTORE-DONE
ALPHA LOG END
```

### xform/rot_e_apply.mac

```
-- 登记: C08（三轴独立之二） | 语义: 绕 E 轴 +90°（Y→U，Z→S）；预期 transform | expected_net: modified | restore 搭档: xform/rot_e_restore.mac
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/xform/rot_e_apply.log" OVER
$P SUITE-XFORM-ROTE-APPLY-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
Q ORI WRT /*
ORI Y IS U AND Z IS S WRT /*
Q ORI WRT /*
SAVEWORK 'AIOS suite xform/rot_e apply: EQ-A +90 about E'
$P SUITE-XFORM-ROTE-APPLY-DONE
ALPHA LOG END
```

### xform/rot_e_restore.mac

```
-- 登记: C08 | 语义: 写回基线方位 | expected_net: cancelled | restore 搭档: 本文件是 xform/rot_e_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/xform/rot_e_restore.log" OVER
$P SUITE-XFORM-ROTE-RESTORE-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
ORI Y IS N AND Z IS U WRT /*
Q ORI WRT /*
SAVEWORK 'AIOS suite xform/rot_e restore'
$P SUITE-XFORM-ROTE-RESTORE-DONE
ALPHA LOG END
```

### xform/rot_n_apply.mac

```
-- 登记: C08（三轴独立之三） | 语义: 绕 N 轴 +90°（Y 不动，Z→E）；预期 transform | expected_net: modified | restore 搭档: xform/rot_n_restore.mac
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/xform/rot_n_apply.log" OVER
$P SUITE-XFORM-ROTN-APPLY-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
Q ORI WRT /*
ORI Y IS N AND Z IS E WRT /*
Q ORI WRT /*
SAVEWORK 'AIOS suite xform/rot_n apply: EQ-A +90 about N'
$P SUITE-XFORM-ROTN-APPLY-DONE
ALPHA LOG END
```

### xform/rot_n_restore.mac

```
-- 登记: C08 | 语义: 写回基线方位 | expected_net: cancelled | restore 搭档: 本文件是 xform/rot_n_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/xform/rot_n_restore.log" OVER
$P SUITE-XFORM-ROTN-RESTORE-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
ORI Y IS N AND Z IS U WRT /*
Q ORI WRT /*
SAVEWORK 'AIOS suite xform/rot_n restore'
$P SUITE-XFORM-ROTN-RESTORE-DONE
ALPHA LOG END
```

### xform/compound_apply.mac

```
-- 登记: C08 / C03（转+移复合） | 语义: 同一 SAVEWORK 平移 + 旋转；预期仍 transform（刚体路覆盖全姿态） | expected_net: modified | restore 搭档: xform/compound_restore.mac
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/xform/compound_apply.log" OVER
$P SUITE-XFORM-COMPOUND-APPLY-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
Q POS WRT /*
Q ORI WRT /*
BY N 300 WRT /*
ORI Y IS W AND Z IS U WRT /*
Q POS WRT /*
Q ORI WRT /*
SAVEWORK 'AIOS suite xform/compound apply: EQ-A BY N 300 + rot U'
$P SUITE-XFORM-COMPOUND-APPLY-DONE
ALPHA LOG END
```

### xform/compound_restore.mac

```
-- 登记: C08 | 语义: 反向平移 + 写回基线方位 | expected_net: cancelled | restore 搭档: 本文件是 xform/compound_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/xform/compound_restore.log" OVER
$P SUITE-XFORM-COMPOUND-RESTORE-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
BY S 300 WRT /*
ORI Y IS N AND Z IS U WRT /*
Q POS WRT /*
Q ORI WRT /*
SAVEWORK 'AIOS suite xform/compound restore'
$P SUITE-XFORM-COMPOUND-RESTORE-DONE
ALPHA LOG END
```

### xform/noz_move_apply.mac

```
-- 登记: C06 | 语义: 带 NOZZ 的整台 EQUI 平移；预期 unit_kinds=transform，NOZZ 的目录字典项按身份比不踢 regen（INC-D03），日志无 [模型目标核对] | expected_net: modified | restore 搭档: xform/noz_move_restore.mac
-- 前置: setup 的 EQ-NOZ 是 COPY /1-LNR-Q005-PJ（★该设备确有 NOZZ，由 common/probe_8000.mac 的 Q MEMB 证实）
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/xform/noz_move_apply.log" OVER
$P SUITE-XFORM-NOZMOVE-APPLY-ALIVE
/AIOS-SUITE-EQ-NOZ
Q REF
Q NAME
Q MEMB
Q POS WRT /*
BY E 500 WRT /*
Q POS WRT /*
SAVEWORK 'AIOS suite xform/noz_move apply: EQ-NOZ BY E 500'
$P SUITE-XFORM-NOZMOVE-APPLY-DONE
ALPHA LOG END
```

### xform/noz_move_restore.mac

```
-- 登记: C06 | 语义: 反向平移 | expected_net: cancelled | restore 搭档: 本文件是 xform/noz_move_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/xform/noz_move_restore.log" OVER
$P SUITE-XFORM-NOZMOVE-RESTORE-ALIVE
/AIOS-SUITE-EQ-NOZ
Q REF
Q NAME
BY W 500 WRT /*
Q POS WRT /*
SAVEWORK 'AIOS suite xform/noz_move restore'
$P SUITE-XFORM-NOZMOVE-RESTORE-DONE
ALPHA LOG END
```

---

## 2.5 geom/（改尺寸 / 布尔 / 负实体增删）

### geom/resize_apply.mac

```
-- 登记: A02 | 语义: 子件尺寸变；预期 EQ-A regen（kind=regen），行数不变、包围盒变 | expected_net: modified | restore 搭档: geom/resize_restore.mac
-- 来源: scripts/e3d/increment_fixture/cases/geometry_apply.mac:2-5
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/geom/resize_apply.log" OVER
$P SUITE-GEOM-RESIZE-APPLY-ALIVE
/AIOS-SUITE-BOX-A1
Q REF
Q NAME
XLEN 1600
YLEN 1200
ZLEN 800
Q XLEN
Q YLEN
Q ZLEN
SAVEWORK 'AIOS suite geom/resize apply'
$P SUITE-GEOM-RESIZE-APPLY-DONE
ALPHA LOG END
```

### geom/resize_restore.mac

```
-- 登记: A02 | 语义: 尺寸写回 1000³（setup 值） | expected_net: cancelled | restore 搭档: 本文件是 geom/resize_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/geom/resize_restore.log" OVER
$P SUITE-GEOM-RESIZE-RESTORE-ALIVE
/AIOS-SUITE-BOX-A1
Q REF
Q NAME
XLEN 1000
YLEN 1000
ZLEN 1000
Q XLEN
Q YLEN
Q ZLEN
SAVEWORK 'AIOS suite geom/resize restore'
$P SUITE-GEOM-RESIZE-RESTORE-DONE
ALPHA LOG END
```

### geom/nbox_resize_apply.mac

```
-- 登记: A02（布尔） | 语义: 负体 NBOX-N1 尺寸变；预期根 EQ-NBX regen，模型行数不变（负体不单独成行） | expected_net: modified | restore 搭档: geom/nbox_resize_restore.mac
-- 来源: scripts/e3d/increment_fixture/cases/boolean_apply.mac:2-3
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/geom/nbox_resize_apply.log" OVER
$P SUITE-GEOM-NBOXRESIZE-APPLY-ALIVE
/AIOS-SUITE-NBOX-N1
Q REF
Q NAME
XLEN 900
Q XLEN
SAVEWORK 'AIOS suite geom/nbox_resize apply'
$P SUITE-GEOM-NBOXRESIZE-APPLY-DONE
ALPHA LOG END
```

### geom/nbox_resize_restore.mac

```
-- 登记: A02 | 语义: NBOX-N1 XLEN 写回 500 | expected_net: cancelled | restore 搭档: 本文件是 geom/nbox_resize_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/geom/nbox_resize_restore.log" OVER
$P SUITE-GEOM-NBOXRESIZE-RESTORE-ALIVE
/AIOS-SUITE-NBOX-N1
Q REF
Q NAME
XLEN 500
Q XLEN
SAVEWORK 'AIOS suite geom/nbox_resize restore'
$P SUITE-GEOM-NBOXRESIZE-RESTORE-DONE
ALPHA LOG END
```

### geom/nbox_add_apply.mac

```
-- 登记: A02（负实体增删，登记表无单独 ID） | 语义: BOX-N 下再挂一个 NBOX；预期 EQ-NBX regen、行数不增 | expected_net: cancelled（由 restore 删除） | restore 搭档: geom/nbox_add_restore.mac
-- 来源: scripts/e3d/s4_db8000_nbox_in_box_create.mac:30（BOX 下 NEW NBOX）
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/geom/nbox_add_apply.log" OVER
$P SUITE-GEOM-NBOXADD-APPLY-ALIVE
/AIOS-SUITE-BOX-N
Q REF
Q NAME
NEW NBOX XLEN 300 YLEN 300 ZLEN 2500
NAME /AIOS-SUITE-NBOX-N2
POS E 600 N 0 U 0
Q REF
Q NAME
Q OWNE
SAVEWORK 'AIOS suite geom/nbox_add apply'
$P SUITE-GEOM-NBOXADD-APPLY-DONE
ALPHA LOG END
```

### geom/nbox_add_restore.mac

```
-- 登记: A02 | 语义: 删掉 NBOX-N2 | expected_net: cancelled | restore 搭档: 本文件是 geom/nbox_add_apply.mac 的 restore
-- 来源: scripts/e3d/increment_fixture/cases/add_restore.mac:2-9（按名 COLLECT 后删）
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/geom/nbox_add_restore.log" OVER
$P SUITE-GEOM-NBOXADD-RESTORE-ALIVE
VAR !n COLLECT ALL NBOX WITH (NAME EQ '/AIOS-SUITE-NBOX-N2')
DO !x VALUES !n
  !!CE = !x.dbref()
  Q REF
  Q NAME
  DELETE NBOX
ENDDO
SAVEWORK 'AIOS suite geom/nbox_add restore'
$P SUITE-GEOM-NBOXADD-RESTORE-DONE
ALPHA LOG END
```

---

## 2.6 connect/（增成员 / 删成员 / 换序 / 改挂 / NOZZ）

靶：setup 复制出的 `BRAN-S`（7 FTUB + 2 BEND，成员带 SPRE 才有几何：scripts/e3d/db8000_pipe_new_member_apply.mac:3）与 create/bran_copy_apply.mac 建的 `BRAN-NEW`。

### connect/bran_add_member_apply.mac

```
-- 登记: A01（增成员） | 语义: 用 COPY 带 SPRE 的成员往 BRAN-S 末尾加一段 FTUB；预期 BRAN-S regen，管身行重排 | expected_net: cancelled（由 restore 删除） | restore 搭档: connect/bran_add_member_restore.mac
-- 来源: scripts/e3d/db8000_pipe_new_member_apply.mac:20（COPY =已有 FTUB）；scripts/e3d/db8000_bran_scratch_create.mac:37-41（FTUB n 导航、OWNER 上跳）
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/connect/bran_add_member_apply.log" OVER
$P SUITE-CONN-ADDMEM-APPLY-ALIVE
/AIOS-SUITE-BRAN-S
Q REF
Q NAME
Q MEMB
FTUB 7
!src = !!CE
Q REF
Q SPRE
OWNER
NEW FTUB /AIOS-SUITE-FTUB-NEW
COPY $!src                                 $* ★复制成员后的排序与头尾连接由 E3D 自动处理；要看 Q MEMB 里它落在末尾
Q REF
Q NAME
Q SPRE
Q POS
/AIOS-SUITE-BRAN-S
Q MEMB
SAVEWORK 'AIOS suite connect/bran_add_member apply'
$P SUITE-CONN-ADDMEM-APPLY-DONE
ALPHA LOG END
```

### connect/bran_add_member_restore.mac

```
-- 登记: A01 | 语义: 删掉新加的 FTUB-NEW | expected_net: cancelled | restore 搭档: 本文件是 connect/bran_add_member_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/connect/bran_add_member_restore.log" OVER
$P SUITE-CONN-ADDMEM-RESTORE-ALIVE
VAR !f COLLECT ALL FTUB WITH (NAME EQ '/AIOS-SUITE-FTUB-NEW')
DO !x VALUES !f
  !!CE = !x.dbref()
  Q REF
  Q NAME
  DELETE FTUB
ENDDO
/AIOS-SUITE-BRAN-S
Q MEMB
SAVEWORK 'AIOS suite connect/bran_add_member restore'
$P SUITE-CONN-ADDMEM-RESTORE-DONE
ALPHA LOG END
```

### connect/bran_del_member_apply.mac

```
-- 登记: A01（删成员） | 语义: 删 BRAN-S 的 FTUB 4；预期 BRAN-S regen，管身行重排（复刻 scripts/e3d/db8000_bran_scratch_create.mac:11 的「删·成员」腿） | expected_net: deleted（成员） | restore 搭档: connect/bran_del_member_restore.mac
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/connect/bran_del_member_apply.log" OVER
$P SUITE-CONN-DELMEM-APPLY-ALIVE
/AIOS-SUITE-BRAN-S
Q REF
Q NAME
Q MEMB
FTUB 4
Q REF
Q POS
DELETE FTUB
/AIOS-SUITE-BRAN-S
Q MEMB
SAVEWORK 'AIOS suite connect/bran_del_member apply'
$P SUITE-CONN-DELMEM-APPLY-DONE
ALPHA LOG END
```

### connect/bran_del_member_restore.mac

```
-- 登记: A01 | 语义: 删成员后按 refno 不可逆，按几何可逆——删掉整条 BRAN-S 再从生产支管重新 COPY 一条（与 setup 同值） | expected_net: cancelled（按名；BRAN-S 及全部成员 refno 已变） | restore 搭档: 本文件是 connect/bran_del_member_apply.mac 的 restore
-- 来源: scripts/e3d/db8000_bran_scratch_create.mac:28-29
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/connect/bran_del_member_restore.log" OVER
$P SUITE-CONN-DELMEM-RESTORE-ALIVE
/AIOS-SUITE-BRAN-S
Q REF
Q NAME
DELETE BRAN
/AIOS-SUITE-PIPE-P
NEW BRAN /AIOS-SUITE-BRAN-S
COPY /C-OR-1R345-C
Q REF
Q NAME
Q MEMB
SAVEWORK 'AIOS suite connect/bran_del_member restore: recopy BRAN-S'
$P SUITE-CONN-DELMEM-RESTORE-DONE
ALPHA LOG END
```

### connect/bran_reorder_apply.mac

```
-- 登记: A01（成员换序，登记表无单独 ID） | 语义: 只换成员序、不动几何；预期模型面不变或仅管身重排，不得删库行 | expected_net: modified（成员序） | restore 搭档: connect/bran_reorder_restore.mac
-- ★整条宏待验：REORDER 语法未在仓内任何已跑过的宏里出现；被拒就改用 INCLUDE … BEFORE/AFTER。其它链不依赖它。
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/connect/bran_reorder_apply.log" OVER
$P SUITE-CONN-REORDER-APPLY-ALIVE
/AIOS-SUITE-BRAN-S
Q REF
Q NAME
Q MEMB
FTUB 2
Q REF
REORDER AFTER FTUB 3                       $* ★
/AIOS-SUITE-BRAN-S
Q MEMB
SAVEWORK 'AIOS suite connect/bran_reorder apply'
$P SUITE-CONN-REORDER-APPLY-DONE
ALPHA LOG END
```

### connect/bran_reorder_restore.mac

```
-- 登记: A01 | 语义: 把换到后面的那段再换回原位 | expected_net: cancelled | restore 搭档: 本文件是 connect/bran_reorder_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/connect/bran_reorder_restore.log" OVER
$P SUITE-CONN-REORDER-RESTORE-ALIVE
/AIOS-SUITE-BRAN-S
Q REF
Q NAME
Q MEMB
FTUB 3                                     $* 换序后原 FTUB 2 在第 3 位
Q REF
REORDER BEFORE FTUB 2                      $* ★
/AIOS-SUITE-BRAN-S
Q MEMB
SAVEWORK 'AIOS suite connect/bran_reorder restore'
$P SUITE-CONN-REORDER-RESTORE-DONE
ALPHA LOG END
```

### connect/bran_reparent_apply.mac

```
-- 登记: A01 / A02（跨 BRAN 改挂） | 语义: BRAN-S 的 FTUB 4 INCLUDE 到 BRAN-NEW；预期两根都 regen（scripts/e3d/db8000_pipe_reparent_apply.mac:11-13） | expected_net: modified（owner） | restore 搭档: connect/bran_reparent_restore.mac
-- 前置: create/bran_copy_apply.mac 已建 BRAN-NEW（不要先跑它的 restore）
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/connect/bran_reparent_apply.log" OVER
$P SUITE-CONN-REPARENT-APPLY-ALIVE
/AIOS-SUITE-BRAN-S
Q REF
Q NAME
FTUB 4
!ftub = !!CE
Q REF
Q OWNE
/AIOS-SUITE-BRAN-NEW
Q REF
Q NAME
INCLUDE $!ftub
$!ftub
Q REF
Q OWNE
SAVEWORK 'AIOS suite connect/bran_reparent apply: FTUB 4 BRAN-S -> BRAN-NEW'
$P SUITE-CONN-REPARENT-APPLY-DONE
ALPHA LOG END
```

### connect/bran_reparent_restore.mac

```
-- 登记: A01 / A02 | 语义: 把那段 FTUB INCLUDE 回 BRAN-S（它在 BRAN-NEW 里是末位成员） | expected_net: cancelled（成员序回到末位而非第 4 位：几何序列变，若要严格还原再跟一条 bran_del_member_restore） | restore 搭档: 本文件是 connect/bran_reparent_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/connect/bran_reparent_restore.log" OVER
$P SUITE-CONN-REPARENT-RESTORE-ALIVE
/AIOS-SUITE-BRAN-NEW
Q REF
Q NAME
Q MEMB
FTUB LAST                                  $* ★LAST 关键字；被拒就用 Q MEMB 数出的序号
Q REF
!ftub = !!CE
/AIOS-SUITE-BRAN-S
INCLUDE $!ftub
$!ftub
Q REF
Q OWNE
SAVEWORK 'AIOS suite connect/bran_reparent restore'
$P SUITE-CONN-REPARENT-RESTORE-DONE
ALPHA LOG END
```

### connect/nozz_move_apply.mac

```
-- 登记: C06（半边）/ A02 | 语义: NOZZ 在设备内单独移动；预期只 EQ-NOZ regen（tests/equi_increment.rs:218） | expected_net: modified | restore 搭档: connect/nozz_move_restore.mac
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/connect/nozz_move_apply.log" OVER
$P SUITE-CONN-NOZZ-APPLY-ALIVE
/AIOS-SUITE-EQ-NOZ
Q REF
Q NAME
Q MEMB
NOZZ 1                                     $* ★该设备确有 NOZZ
Q REF
Q NAME
Q POS
BY U 100
Q POS
SAVEWORK 'AIOS suite connect/nozz_move apply'
$P SUITE-CONN-NOZZ-APPLY-DONE
ALPHA LOG END
```

### connect/nozz_move_restore.mac

```
-- 登记: C06 / A02 | 语义: NOZZ 移回 | expected_net: cancelled | restore 搭档: 本文件是 connect/nozz_move_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/connect/nozz_move_restore.log" OVER
$P SUITE-CONN-NOZZ-RESTORE-ALIVE
/AIOS-SUITE-EQ-NOZ
Q REF
Q NAME
NOZZ 1                                     $* ★
Q REF
Q NAME
BY D 100
Q POS
SAVEWORK 'AIOS suite connect/nozz_move restore'
$P SUITE-CONN-NOZZ-RESTORE-DONE
ALPHA LOG END
```

---

## 2.7 pipe/（整条 PIPE 位移 / BRAN 头位移 / 换 SPRE）

### pipe/whole_move_apply.mac

```
-- 登记: C05 | 语义: 整条 PIPE 平移；预期每条 BRAN 的 HPOS/TPOS 变 → 刚体路按设计拒 → unit_kinds=regen，O1 用 SAVEWORK 前后两份 RVM 对拍（导出段可照搬 db8000_pipe_reparent_apply.mac:38-54） | expected_net: modified | restore 搭档: pipe/whole_move_restore.mac
-- 前置: setup 的 PIPE-P 是 COPY /1RX-345（★PIPE 级 COPY 连带全部 BRAN）
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/pipe/whole_move_apply.log" OVER
$P SUITE-PIPE-WHOLE-APPLY-ALIVE
/AIOS-SUITE-PIPE-P
Q REF
Q NAME
VAR !brans COLLECT ALL BRAN FOR CE
DO !b VALUES !brans
  !!CE = !b.dbref()
  Q REF
  Q HPOS WRT /*
  Q TPOS WRT /*
ENDDO
/AIOS-SUITE-PIPE-P
BY E 500 WRT /*                            $* ★PIPE 级 BY（ZONE 级已实测：scripts/e3d/adr066_live2_zone_103249_move_apply.mac:17）；不成就把这句改进上面的 DO 里逐 BRAN BY
DO !b VALUES !brans
  !!CE = !b.dbref()
  Q REF
  Q HPOS WRT /*
  Q TPOS WRT /*
ENDDO
SAVEWORK 'AIOS suite pipe/whole_move apply: PIPE-P BY E 500'
$P SUITE-PIPE-WHOLE-APPLY-DONE
ALPHA LOG END
```

### pipe/whole_move_restore.mac

```
-- 登记: C05 | 语义: 整条 PIPE 移回 | expected_net: cancelled | restore 搭档: 本文件是 pipe/whole_move_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/pipe/whole_move_restore.log" OVER
$P SUITE-PIPE-WHOLE-RESTORE-ALIVE
/AIOS-SUITE-PIPE-P
Q REF
Q NAME
BY W 500 WRT /*                            $* ★同 apply
VAR !brans COLLECT ALL BRAN FOR CE
DO !b VALUES !brans
  !!CE = !b.dbref()
  Q REF
  Q HPOS WRT /*
  Q TPOS WRT /*
ENDDO
SAVEWORK 'AIOS suite pipe/whole_move restore'
$P SUITE-PIPE-WHOLE-RESTORE-DONE
ALPHA LOG END
```

### pipe/bran_hpos_apply.mac

```
-- 登记: A01 | 语义: 只改支管头点；预期该 BRAN regen（同 scripts/e3d/db8000_bran_hpos_apply.mac 的路） | expected_net: modified | restore 搭档: pipe/bran_hpos_restore.mac
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/pipe/bran_hpos_apply.log" OVER
$P SUITE-PIPE-HPOS-APPLY-ALIVE
/AIOS-SUITE-BRAN-S
Q REF
Q NAME
Q HPOS WRT /*
HPOS BY U 200 WRT /*                       $* ★`HPOS BY` 写法；被拒就用上一句 Q 出的绝对值写 HPOS E .. N .. U ..
Q HPOS WRT /*
SAVEWORK 'AIOS suite pipe/bran_hpos apply'
$P SUITE-PIPE-HPOS-APPLY-DONE
ALPHA LOG END
```

### pipe/bran_hpos_restore.mac

```
-- 登记: A01 | 语义: 头点移回 | expected_net: cancelled | restore 搭档: 本文件是 pipe/bran_hpos_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/pipe/bran_hpos_restore.log" OVER
$P SUITE-PIPE-HPOS-RESTORE-ALIVE
/AIOS-SUITE-BRAN-S
Q REF
Q NAME
HPOS BY D 200 WRT /*                       $* ★同 apply
Q HPOS WRT /*
SAVEWORK 'AIOS suite pipe/bran_hpos restore'
$P SUITE-PIPE-HPOS-RESTORE-DONE
ALPHA LOG END
```

### pipe/spre_swap（不编——卡语料）

「换规格件 SPRE」无法在没有核过的替换件 refno 时写成可落盘宏：`scripts/e3d/db8000_pipe_spre_swap_apply.mac:7-11` 的 `<ALT_SPRE>` 要在 db 13244 的 SPEC 13244/109234 里枚举同 bore/rating 的弯头组件，该库在已接目录集里未定位。现有宏文本可直接复用，只差那一个 refno；补齐后登记到 E04 近亲（SPRE → DependencyCascade → Regen，lib 钉 `pipe_spec_reference_regenerates_but_name_only_skips_model_work`）。

---

## 2.8 room/（成员进出 / PANE 动 / PAVE 动 / FRMW 改名）

运行器配置要把房间关键字收窄到本套：`room_key_word = ["AIOS-SUITE-RM"]`（src/bin/l3_suite/fixture.rs:951-956 对 increment_fixture 的同款处理）。

### room/member_move_apply.mac

```
-- 登记: G02 / G01 | 语义: EQ-RM 从 R201 跨入 R202；预期 rooms_changed before [R201] after [R202]，库上 room_relate 边 R201 消失、R202 出现；链尾 rebuild_edges_equal | expected_net: modified | restore 搭档: room/member_move_restore.mac
-- 来源: scripts/e3d/increment_fixture/cases/room_member_apply.mac:2-3
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/room/member_move_apply.log" OVER
$P SUITE-ROOM-MEMBER-APPLY-ALIVE
/AIOS-SUITE-EQ-RM
Q REF
Q NAME
Q POS WRT /*
POS E 70000 N 5000 U 1000 WRT /*
Q POS WRT /*
SAVEWORK 'AIOS suite room/member_move apply: EQ-RM into R202'
$P SUITE-ROOM-MEMBER-APPLY-DONE
ALPHA LOG END
```

### room/member_move_restore.mac

```
-- 登记: G02 | 语义: EQ-RM 回 R201 | expected_net: cancelled | restore 搭档: 本文件是 room/member_move_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/room/member_move_restore.log" OVER
$P SUITE-ROOM-MEMBER-RESTORE-ALIVE
/AIOS-SUITE-EQ-RM
Q REF
Q NAME
POS E 60000 N 5000 U 1000 WRT /*
Q POS WRT /*
SAVEWORK 'AIOS suite room/member_move restore: EQ-RM back to R201'
$P SUITE-ROOM-MEMBER-RESTORE-DONE
ALPHA LOG END
```

### room/pane_move_apply.mac

```
-- 登记: G03 | 语义: 房间定义元素 PANE 整块下移；预期服务排**一条** room_rebuild 整间重算，边数与全量相同 | expected_net: modified | restore 搭档: room/pane_move_restore.mac
-- 来源: scripts/e3d/replay/room_pane_apply.mac:8-14
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/room/pane_move_apply.log" OVER
$P SUITE-ROOM-PANE-APPLY-ALIVE
/AIOS-SUITE-RM-R201-P1
Q TYPE
Q REF
Q NAME
Q POS
BY D 200
Q POS
SAVEWORK 'AIOS suite room/pane_move apply: R201-P1 down 200'
$P SUITE-ROOM-PANE-APPLY-DONE
ALPHA LOG END
```

### room/pane_move_restore.mac

```
-- 登记: G03 | 语义: PANE 移回 | expected_net: cancelled | restore 搭档: 本文件是 room/pane_move_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/room/pane_move_restore.log" OVER
$P SUITE-ROOM-PANE-RESTORE-ALIVE
/AIOS-SUITE-RM-R201-P1
Q REF
Q NAME
BY U 200
Q POS
SAVEWORK 'AIOS suite room/pane_move restore'
$P SUITE-ROOM-PANE-RESTORE-DONE
ALPHA LOG END
```

### room/pave_move_apply.mac

```
-- 登记: G03 | 语义: 改房间轮廓顶点（R201 东边从 E 62000 外扩到 64000）；预期 room_rebuild 一条，EQ-RM（E 60000）归属不变 | expected_net: modified | restore 搭档: room/pave_move_restore.mac
-- PAVE 没有 NAME：对 PANE 的 Q REF / Q NAME 让宏过录制审查；PLOO/PAVE 序号导航同成员序号导航（db8000_bran_scratch_create.mac:37）
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/room/pave_move_apply.log" OVER
$P SUITE-ROOM-PAVE-APPLY-ALIVE
/AIOS-SUITE-RM-R201-P1
Q REF
Q NAME
PLOO 1
PAVE 2
Q REF
Q POS
POS E 64000 N 3000 U 0
Q POS
OWNER
PAVE 3
Q REF
Q POS
POS E 64000 N 7000 U 0
Q POS
SAVEWORK 'AIOS suite room/pave_move apply: R201 east edge to E 64000'
$P SUITE-ROOM-PAVE-APPLY-DONE
ALPHA LOG END
```

### room/pave_move_restore.mac

```
-- 登记: G03 | 语义: 两个顶点写回 E 62000 | expected_net: cancelled | restore 搭档: 本文件是 room/pave_move_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/room/pave_move_restore.log" OVER
$P SUITE-ROOM-PAVE-RESTORE-ALIVE
/AIOS-SUITE-RM-R201-P1
Q REF
Q NAME
PLOO 1
PAVE 2
Q REF
POS E 62000 N 3000 U 0
Q POS
OWNER
PAVE 3
Q REF
POS E 62000 N 7000 U 0
Q POS
SAVEWORK 'AIOS suite room/pave_move restore'
$P SUITE-ROOM-PAVE-RESTORE-DONE
ALPHA LOG END
```

### room/frmw_rename_apply.mac

```
-- 登记: G03 | 语义: 把 FRMW 改成不带房间关键字的名字 → 房间 R201 消失，EQ-RM 归属清空 | expected_net: modified | restore 搭档: room/frmw_rename_restore.mac
-- 来源: scripts/e3d/increment_fixture/cases/room_structure_apply.mac:2-3
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/room/frmw_rename_apply.log" OVER
$P SUITE-ROOM-FRMW-APPLY-ALIVE
/AIOS-SUITE-RM-R201
Q REF
Q NAME
NAME /AIOS-SUITE-NOT-A-ROOM
Q REF
Q NAME
SAVEWORK 'AIOS suite room/frmw_rename apply'
$P SUITE-ROOM-FRMW-APPLY-DONE
ALPHA LOG END
```

### room/frmw_rename_restore.mac

```
-- 登记: G03 | 语义: 改回房间名 | expected_net: cancelled | restore 搭档: 本文件是 room/frmw_rename_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/room/frmw_rename_restore.log" OVER
$P SUITE-ROOM-FRMW-RESTORE-ALIVE
/AIOS-SUITE-NOT-A-ROOM
Q REF
Q NAME
NAME /AIOS-SUITE-RM-R201
Q REF
Q NAME
SAVEWORK 'AIOS suite room/frmw_rename restore'
$P SUITE-ROOM-FRMW-RESTORE-DONE
ALPHA LOG END
```

---

## 2.9 compound/（一个 SAVEWORK 多动作）

### compound/rename_move_add_apply.mac

```
-- 登记: A08（窗口分桶）/ A07 / A02 | 语义: 同一 SAVEWORK：改名 + 整根平移 + 增子件；预期 EQ-A 一窗内一次 regen（增子件压过 transform），datacenter_delete renamed 一条 | expected_net: modified | restore 搭档: compound/rename_move_add_restore.mac
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/compound/rename_move_add_apply.log" OVER
$P SUITE-COMPOUND-RMA-APPLY-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
NAME /AIOS-SUITE-EQ-A-V2
BY E 500 WRT /*
Q POS WRT /*
NEW BOX XLEN 300 YLEN 300 ZLEN 300
NAME /AIOS-SUITE-BOX-A3
POS E -1500 N 0 U 0
Q REF
Q NAME
Q OWNE
SAVEWORK 'AIOS suite compound/rename_move_add apply'
$P SUITE-COMPOUND-RMA-APPLY-DONE
ALPHA LOG END
```

### compound/rename_move_add_restore.mac

```
-- 登记: A08 | 语义: 删新子件、移回、改回名（也是一个复合窗） | expected_net: cancelled | restore 搭档: 本文件是 compound/rename_move_add_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/compound/rename_move_add_restore.log" OVER
$P SUITE-COMPOUND-RMA-RESTORE-ALIVE
VAR !b COLLECT ALL BOX WITH (NAME EQ '/AIOS-SUITE-BOX-A3')
DO !x VALUES !b
  !!CE = !x.dbref()
  Q REF
  Q NAME
  DELETE BOX
ENDDO
/AIOS-SUITE-EQ-A-V2
Q REF
Q NAME
BY W 500 WRT /*
NAME /AIOS-SUITE-EQ-A
Q POS WRT /*
Q NAME
SAVEWORK 'AIOS suite compound/rename_move_add restore'
$P SUITE-COMPOUND-RMA-RESTORE-DONE
ALPHA LOG END
```

### compound/two_roots_apply.mac

```
-- 登记: A08 / F04 | 语义: 两根同窗各自平移；预期 unit_count 2、两条 transform、互不干扰 | expected_net: modified | restore 搭档: compound/two_roots_restore.mac
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/compound/two_roots_apply.log" OVER
$P SUITE-COMPOUND-2R-APPLY-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
BY U 400 WRT /*
Q POS WRT /*
/AIOS-SUITE-EQ-NBX
Q REF
Q NAME
BY U 400 WRT /*
Q POS WRT /*
SAVEWORK 'AIOS suite compound/two_roots apply'
$P SUITE-COMPOUND-2R-APPLY-DONE
ALPHA LOG END
```

### compound/two_roots_restore.mac

```
-- 登记: A08 | 语义: 两根移回 | expected_net: cancelled | restore 搭档: 本文件是 compound/two_roots_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/compound/two_roots_restore.log" OVER
$P SUITE-COMPOUND-2R-RESTORE-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
BY D 400 WRT /*
Q POS WRT /*
/AIOS-SUITE-EQ-NBX
Q REF
Q NAME
BY D 400 WRT /*
Q POS WRT /*
SAVEWORK 'AIOS suite compound/two_roots restore'
$P SUITE-COMPOUND-2R-RESTORE-DONE
ALPHA LOG END
```

---

## 2.10 h03/（两库：伙伴库 7998）

选 7998 而不是 7997：`db_session_fixture pack` 的 6 MiB 上限（src/bin/db_session_fixture/format.rs:25，pipeline.rs:204-205），7998 实测 1.79 MB 合格、7997 实测 12.9 MB 超限。所有写入都必须在隔离副本（E:\reverse\e3d\replay-src\E3D3.1）上做：影子根里的 ams7998_0001 是到真工程的硬链接（scripts/replay/New-ShadowProjectRoot.ps1:65-79）。

7998 的 WORL refno 未知：先跑 `l3_suite --fixture-manifest scripts/e3d/increment_fixture/fixture-manifest.json --target-db-file <副本>\ams7998_0001 --target-dbnum 7998 --fixture-check-only ...`，读 preflight.json 的 `world_refno`（src/bin/l3_suite/fixture.rs:244-246、:260-270），把下面 `=<WORL-7998>` 替换成它。

### h03/probe_7998.mac（只读、无 SAVEWORK；单走 `l3_suite --check-driver`，不进录制清单）

```
-- 登记: H03 | 语义: 探针——列出 7998 的 WORL 下全部 SITE 与已有的 AIOS-SUITE 元素，确认库号与命名不撞 | expected_net: 无（不写） | restore 搭档: 无
ONERROR CONTINUE
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/h03/probe_7998.log" OVER
$P SUITE-H03-PROBE-ALIVE
=<WORL-7998>                               $* ★用 preflight.json 的 world_refno 替换
Q TYPE
Q REF
VAR !sites COLLECT ALL SITE FOR CE
DO !s VALUES !sites
  !!CE = !s.dbref()
  Q REF
  Q NAME
  Q TYPE
ENDDO
VAR !mine COLLECT ALL WITH (MATCHWILD(NAME,'/AIOS-SUITE-7998*')) FOR CE
DO !m VALUES !mine
  !!CE = !m.dbref()
  Q REF
  Q NAME
  Q TYPE
  Q POS WRT /*
ENDDO
$P SUITE-H03-PROBE-DONE
ALPHA LOG END
```

### h03/setup_7998.mac

```
-- 登记: H03 | 语义: 伙伴库基线——SITE/ZONE/EQUI(+BOX)，幂等（先删同名 SITE，increment_fixture setup.mac:3-8 同款） | expected_net: added（基线，不计入链） | restore 搭档: h03/teardown_7998.mac
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/h03/setup_7998.log" OVER
$P SUITE-H03-SETUP-ALIVE
=<WORL-7998>                               $* ★
VAR !old COLLECT ALL SITE WITH (MATCHWILD(NAME,'/AIOS-SUITE-7998*')) FOR CE
DO !s VALUES !old
  !!CE = !s.dbref()
  DELETE SITE
ENDDO
=<WORL-7998>                               $* ★
NEW SITE /AIOS-SUITE-7998
NEW ZONE /AIOS-SUITE-7998-Z
NEW EQUIPMENT /AIOS-SUITE-7998-EQ
POS E 40000 N 5000 U 0
Q REF
Q NAME
NEW BOX XLEN 1000 YLEN 1000 ZLEN 1000
NAME /AIOS-SUITE-7998-BOX
Q REF
Q NAME
SAVEWORK 'AIOS suite H03: baseline setup db7998'
$P SUITE-H03-SETUP-DONE
ALPHA LOG END
```

### h03/teardown_7998.mac

```
-- 登记: H03 | 语义: 删掉伙伴库的 scratch SITE | expected_net: deleted（基线收场） | restore 搭档: 本文件是 h03/setup_7998.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/h03/teardown_7998.log" OVER
$P SUITE-H03-TEARDOWN-ALIVE
=<WORL-7998>                               $* ★
VAR !sites COLLECT ALL SITE WITH (MATCHWILD(NAME,'/AIOS-SUITE-7998*')) FOR CE
DO !s VALUES !sites
  !!CE = !s.dbref()
  Q REF
  Q NAME
  DELETE SITE
ENDDO
SAVEWORK 'AIOS suite H03: teardown db7998'
$P SUITE-H03-TEARDOWN-DONE
ALPHA LOG END
```

### h03/move_apply.mac

```
-- 登记: H03（T-O-1 / T-O-2） | 语义: 伙伴库上一次成功的位移窗；配 service_env `AIOS_TEST_FAIL_DATA_ENDS=8000:<右端>`（主库 park，batch_worker.rs:981-994）与 `AIOS_TEST_DELAY_BEFORE_REGISTRY_FINISH=7998/<右端>:60000`（把本窗卡在 Running，batch_worker.rs:1015-1044），由 L2 companions 用例断言「8000 park 而 7998 前进」 | expected_net: modified | restore 搭档: h03/move_restore.mac
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/h03/move_apply.log" OVER
$P SUITE-H03-MOVE-APPLY-ALIVE
/AIOS-SUITE-7998-EQ
Q REF
Q NAME
Q POS WRT /*
BY E 500 WRT /*
Q POS WRT /*
SAVEWORK 'AIOS suite H03 companion 7998: EQ BY E 500'
$P SUITE-H03-MOVE-APPLY-DONE
ALPHA LOG END
```

### h03/move_restore.mac

```
-- 登记: H03 | 语义: 伙伴库位移移回（第二个成功窗，给 T-O-1 步用） | expected_net: cancelled | restore 搭档: 本文件是 h03/move_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/h03/move_restore.log" OVER
$P SUITE-H03-MOVE-RESTORE-ALIVE
/AIOS-SUITE-7998-EQ
Q REF
Q NAME
BY W 500 WRT /*
Q POS WRT /*
SAVEWORK 'AIOS suite H03 companion 7998: EQ BY W 500'
$P SUITE-H03-MOVE-RESTORE-DONE
ALPHA LOG END
```

---

## 2.11 补全第一段里只给了摘要的三条 restore

### create/equi_copy_restore.mac

```
-- 登记: A02（整台复制的删除半边） | 语义: 删掉复制出的整台 EQ-COPY；预期一条 DeleteCleanup | expected_net: cancelled | restore 搭档: 本文件是 create/equi_copy_apply.mac 的 restore
-- 来源: scripts/e3d/db8000_equi_copy_restore.mac
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/create/equi_copy_restore.log" OVER
$P SUITE-CREATE-EQCOPY-RESTORE-ALIVE
VAR !t COLLECT ALL EQUI WITH (NAME EQ '/AIOS-SUITE-EQ-COPY')
DO !x VALUES !t
  !!CE = !x.dbref()
  Q REF
  Q NAME
  Q MEMB
  DELETE EQUI
ENDDO
SAVEWORK 'AIOS suite create/equi_copy restore (delete clone)'
$P SUITE-CREATE-EQCOPY-RESTORE-DONE
ALPHA LOG END
```

### create/bran_copy_restore.mac

```
-- 登记: A01（删 BRAN） | 语义: 删掉复制出的 BRAN-NEW；预期一条 DeleteCleanup | expected_net: cancelled | restore 搭档: 本文件是 create/bran_copy_apply.mac 的 restore（若已跑 connect/bran_reparent_apply，要先跑它的 restore，否则 FTUB 4 随此被删）
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/create/bran_copy_restore.log" OVER
$P SUITE-CREATE-BRANCOPY-RESTORE-ALIVE
VAR !t COLLECT ALL BRAN WITH (NAME EQ '/AIOS-SUITE-BRAN-NEW')
DO !x VALUES !t
  !!CE = !x.dbref()
  Q REF
  Q NAME
  Q MEMB
  DELETE BRAN
ENDDO
SAVEWORK 'AIOS suite create/bran_copy restore (delete BRAN-NEW)'
$P SUITE-CREATE-BRANCOPY-RESTORE-DONE
ALPHA LOG END
```

### create/cross_db_copy_restore.mac

```
-- 登记: （跨库移动，登记表无单独 ID；J01 `expect_unselected_non_empty` 的前置） | 语义: 把跨库复制过去的 EQ-X 复制回 8000 成为新的 EQ-A，再删掉 7998 里的 EQ-X；同一 SAVEWORK 写两个文件 | expected_net: cancelled（按名；EQ-A 是新 refno） | restore 搭档: 本文件是 create/cross_db_copy_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/create/cross_db_copy_restore.log" OVER
$P SUITE-CROSSDB-RESTORE-ALIVE
/AIOS-SUITE-8000-Z
NEW EQUIPMENT /AIOS-SUITE-EQ-A
COPY /AIOS-SUITE-EQ-X                      $* ★跨库 COPY（源在 7998、CE 在 8000）
POS E 40000 N 5000 U 0
Q REF
Q NAME
Q MEMB
/AIOS-SUITE-EQ-X
Q REF
Q NAME
DELETE EQUI
SAVEWORK 'AIOS suite cross-db restore: EQ-A back into 8000, EQ-X removed from 7998'
$P SUITE-CROSSDB-RESTORE-DONE
ALPHA LOG END
```

---

# 第三部分 目录结构与运行器

## 3.1 目录（新建一套，不往 increment_fixture/cases/ 里塞）

理由：increment_fixture 的 manifest 没有库维度和链/腿的概念（src/bin/l3_suite/fixture.rs:47-85），它的宏靠 `{{REF:xxx}}` 占位（fixture.rs:2593-2604），而本套要同时被 L3 运行器和 L2 录制器消费，后者要求按名导航 + 日志在宏旁（Record-Db8000SessionChain.ps1:99-100）。

```
scripts/e3d/increment_suite/
  suite-manifest.json            # 库 + 批次 + 链 + 腿 + expected_net + registry ids（下 3.2）
  common/   setup_8000.mac teardown_8000.mac probe_8000.mac
  create/   box_* subtree_* equi_copy_* bran_copy_* cross_db_copy_*
  delete/   box_* container_keep_child_* recreate_same_name_apply child_then_parent_1/2_apply child_then_parent_restore
  attr/     rename_* desc_* owner_*
  xform/    move_* rot_u_* rot_e_* rot_n_* compound_* noz_move_*
  geom/     resize_* nbox_resize_* nbox_add_*
  connect/  bran_add_member_* bran_del_member_* bran_reorder_* bran_reparent_* nozz_move_*
  pipe/     whole_move_* bran_hpos_*            （spre_swap 待语料，目录预留）
  room/     member_move_* pane_move_* pave_move_* frmw_rename_*
  compound/ rename_move_add_* two_roots_*
  h03/      probe_7998.mac setup_7998.mac teardown_7998.mac move_*
  recording/ 8000-batch0.json 8000-batch1.json 7998-h03.json   # 给 Record 脚本的案例清单（字段同 db8000_recording_cases.json）
```

命名：`<group>/<chain>_<leg>.mac`，腿名只有 `apply` / `restore` / `N_apply`（多腿链）/ `setup` / `teardown` / `probe`。日志同名 `.log`。元素名 `/AIOS-SUITE-<库或角色>-<编号>`。

## 3.2 suite-manifest.json（给运行器的单一事实；字段设计）

```json
{
  "version": 1,
  "prefix": "/AIOS-SUITE-",
  "databases": {
    "8000": { "db_file": "AvevaMarineSample/ams000/ams8000_0001", "setup": "common/setup_8000.mac", "teardown": "common/teardown_8000.mac", "probe": "common/probe_8000.mac", "room_key_word": "AIOS-SUITE-RM" },
    "7998": { "db_file": "AvevaMarineSample/ams000/ams7998_0001", "setup": "h03/setup_7998.mac", "teardown": "h03/teardown_7998.mac", "probe": "h03/probe_7998.mac" }
  },
  "batches": [
    { "id": "b0-primitives", "databases": ["8000"], "chains": ["create/box", "create/subtree", "attr/rename", "attr/desc", "attr/owner", "xform/move", "xform/rot_u", "xform/rot_e", "xform/rot_n", "xform/compound", "geom/resize", "geom/nbox_resize", "geom/nbox_add", "compound/rename_move_add", "compound/two_roots", "room/member_move", "room/frmw_rename", "delete/box", "delete/recreate_same_name", "delete/container_keep_child", "delete/child_then_parent"] },
    { "id": "b1-copied-production", "databases": ["8000"], "chains": ["create/equi_copy", "xform/noz_move", "connect/nozz_move", "create/bran_copy", "connect/bran_reparent", "connect/bran_add_member", "connect/bran_del_member", "connect/bran_reorder", "pipe/whole_move", "pipe/bran_hpos", "room/pane_move", "room/pave_move"] },
    { "id": "b2-two-db", "databases": ["8000", "7998"], "chains": ["h03/move", "create/cross_db_copy"] }
  ],
  "chains": {
    "xform/move": { "db": "8000", "registry": ["INC-C03", "INC-F04"], "legs": [
      { "macro": "xform/move_apply.mac", "expected_net": "modified", "elements": [{ "name": "/AIOS-SUITE-EQ-A", "noun": "EQUI", "before_apply": true, "after_apply": true }], "assert": { "unit_kinds": { "/AIOS-SUITE-EQ-A": "transform" }, "aabb_shift": { "/AIOS-SUITE-EQ-A": [500, 0, 0] } } },
      { "macro": "xform/move_restore.mac", "expected_net": "cancelled" } ] },
    "delete/child_then_parent": { "db": "8000", "registry": ["INC-B01"], "destructive_until_restore": true, "legs": [
      { "macro": "delete/child_then_parent_1_apply.mac", "expected_net": "deleted" },
      { "macro": "delete/child_then_parent_2_apply.mac", "expected_net": "deleted", "assert": { "delete_cleanups": 1 } },
      { "macro": "delete/child_then_parent_restore.mac", "expected_net": "cancelled" } ] }
  }
}
```

`elements`/`expected_net` 的字段含义与 `scripts/e3d/db8000_recording_cases.json:16-35` 一致（录制器现成消费）；`assert` 是给 L3/L2 断言的，键名沿用 L2 harness 的 Step 字段（tests/increment_replay/main.rs:152-283：unit_kinds / aabb_shift / rooms_changed / room_edges / records_present / unit_count / expect_batch_parked …），这样一份清单能生成 L2 用例 JSON。

## 3.3 运行器：两条消费路，建议以 L2 为主

**路 A（L2，无产品代码改动）**：
1. 录制器：`scripts/e3d/Record-Db8000SessionChain.ps1` 已是参数化的（`-Dbnum` :33、`-CaseManifest` :34、`-TargetDbFile`/`-ProjectDir`），按批次调用，清单用 `recording/<db>-<batch>.json`。要改两点（小）：① 允许清单里的 `setup`/`teardown` 腿（它们也是一个 SAVEWORK，现有审查 :74-93 能过，但 `elements` 为空要被允许：:187-205 对空数组本来就是空转）；② 探针腿不进清单，单走 `l3_suite --check-driver`（:116-118 的同一条通道）。录完 `db_session_fixture pack --dbnum <db> --out tests/fixtures/sessions/<db>-<batch>/`（src/bin/db_session_fixture.rs:50-65）。
2. 用例：每批次一条 `tests/increment_replay/cases/<db>-<batch>.json`，`fixture_dir` 指向 pack 产物，每腿一个 Step（`to` = 该腿的 sesno），断言字段直接是 Step 字段。两库批次用 `companions`（main.rs:105-115、:641-653、:1526-1576、:1637-1662）。分批 = 分夹具：6 MiB 上限也逼着每个批次少于十几个会话（db8000 每个会话平均 ~20 KB 压缩后）。
3. 代价：每批次要占一次 E3D 空窗录制；录好后 nightly 不要 E3D。

**路 B（L3 活机，要改 l3_suite）**：把 `--fixture-manifest` 扩成读 suite-manifest：① `Run-E3DFixtureSuite.ps1:3-4`/`:44-45` 多库参数；② `l3_suite.rs:218-222` `Vec<PathBuf>`/`Vec<u32>`；③ `fixture.rs:251-270` 逐库 inspect、`:300-305` 按库 `{{WORLD_REFNO:<db>}}`、`:933-935`/`:961-963` 配置列表、`:584-747` 多 mirror、`target_dbnum` 穿线（:493/:505/:511/:525/:527/:534/:556-558/:1183-1192/:1266/:1377/:1576/:1679/:1814/:1866/:2408-2415/:2473）；④ `fixture.rs:47-85` manifest 结构加库与链；⑤ 新断言平面：`ChangeKind`（:89-99）加 copy / connectivity / rigid_refused / metadata_only，`assert_case`（:1975-2064）加「任务回执 kind」与 `unit_count` 平面；⑥ 钉要改（:2909/:2942/:3068/:3134/:3175-3177）。估 300–400 行。代价大、收益是「真机即时」。

建议：先走路 A 把全套录成 L2 夹具（覆盖即得），路 B 只在需要「改宏即跑」的调试场景时做。

---

# 第四部分 可逆性与可重复执行（三层）

用户要求「做完增量更新后再执行删除的宏，保证最初模型数据能反复执行测试」。三层各管一件事：

## 4.1 PML 层：restore 腿 + expected_net

- 每条 apply 都有 restore（上文全部给出），链内顺序 apply → 断言 → restore → 断言；批次收尾 `teardown` 删 scratch SITE（setup 幂等，下轮可直接重建，increment_fixture setup.mac:3-8 的模式）。
- 三种「逆」要分清：① 属性逆（NAME/DESC/POS/ORI/XLEN/HPOS/INCLUDE）refno 不变，库文件逻辑态完全回到基线；② 重建逆（删后再 NEW/COPY：delete/*、create/*的 restore、connect/bran_del_member_restore）按名和几何回到基线、refno 变——下游链必须按名解 refno（运行器每轮 `probe` 重解，fixture.rs:1576 `resolve_objects` 本来就按名）；③ 不可逆：`delete/recreate_same_name_apply`（按设计终态即基线）、`cross_db_copy`（两库都换 refno）。
- gen-model 侧不是逆运算：`datacenter_delete`（renamed/deleted）记录不会因 restore 消失（fixture.rs:124-126 靠 INSERT IGNORE 容忍），`/tasks` 历史、水位单调增。所以 PML 层只保证「E3D 库态可重复」，不保证「服务态可重复」。
- 适用：L3 活机循环跑、交互调试。代价：每轮两倍会话数；会话链永远在长（append-only）。

## 4.2 文件层：夹具重切（replay-init）

- `db_session_fixture replay-init --fixture <夹具> --to S --out <工作文件>`（src/bin/db_session_fixture.rs:77-89）把 DESI 文件字节级重置到会话 S，`replay-step --to` 就地追加下一会话（:90-106）；harness 每次 init 都这么做（tests/increment_replay/main.rs:1516-1525），伙伴库同（:1526-1543）。服务态也重置：每次一台一次性 surreal（main.rs:4-6）。
- 这是唯一能让「E3D 库态 + 服务态」都回到同一起点的层，也不需要 E3D 在场。代价：语料要先录（6 MiB 上限 → 只能小库、短链）；不带字节快照的夹具是纯现切（format.rs:13），对 anchor 赛跑类用例（H01）要像 move-chain 那样录制时 `SYSCOM copy` 每会话真文件（scripts/e3d/replay/equi_apply.mac:3-4,16）。
- 适用：nightly / 回归 / 两库 H03。

## 4.3 库层：隔离副本

- 所有写入在 `E:\reverse\e3d\replay-src\E3D3.1` 这类整工程副本上做；影子根是硬链接、写穿真工程（New-ShadowProjectRoot.ps1:65-79）。L3 运行器自己还会把目标库文件拷一份给服务读（fixture.rs:667-677 `create_service_project_mirror`）。
- 彻底重置 = 把副本从母本重拷（文件复制，不走 E3D）。更激进的「每轮新库」（`CREATE DB <team>/<name> DESI ... DBNO n` + `ADD` 进 MDB，PMLLIB admin 命令族）不推荐：DBNO 不显式指定会被 NXTDBN 重分，gen-model 按文件头 db_no 认库（src/data_interface/db_file_header.rs:156），watch_dbnums / DbOption 都要跟着改。
- 适用：录制、破坏性链（跨库、删整台）、任何要「当什么都没发生过」的场合。代价：磁盘与拷贝时间，ams 整工程数 GB。

结论：日常回归用 4.2；录制和调试用 4.3 + 4.1；4.1 单独用只能保证 E3D 侧可重复。

---

# 第五部分 执行顺序与阻塞

| 批 | 内容 | 前置 | 阻塞类型 |
|---|---|---|---|
| 0 | `common/setup_8000` 的非 COPY 部分 + b0-primitives 全部链（create/box、subtree；attr/*；xform/move、rot_u/e/n、compound；geom/*；compound/*；room/member_move、frmw_rename；delete/* 四链） | 隔离副本；8000 的 WORL refno（preflight.json） | **立刻可跑**。全部语句在 increment_fixture / db8000_scratch_* 里有同款；唯一 ★ 是 attr/desc_restore 的 `DESC ''` |
| 1 | setup 的三处 COPY（EQ-NOZ / PIPE-P / BRAN-S）+ b1：create/equi_copy、xform/noz_move、connect/*、pipe/whole_move、pipe/bran_hpos、create/bran_copy、room/pane_move、pave_move | 先跑 common/probe_8000 确认 /1-LNR-Q005-PJ 有 NOZZ、/1RX-345 存在；★ 语句：PIPE 级 COPY/BY、`HPOS BY`、`COPY $!src`、`FTUB LAST`、REORDER | **卡真机语法验证**（不卡代码）。预计一次 probe + 一次试跑就能定下 |
| 2 | b2-two-db：h03/* + create/cross_db_copy；对应 L2 用例 `ams8000-h03-two-db-park`（前一份 F2，7997 换 7998） | 7998 的 WORL refno；7998 录制（Record 脚本 `-Dbnum 7998`）+ pack；影子根 `-Replayed` 加 ams7998_0001 | **卡语料**（一次录制即解）；harness 侧已就绪 |
| 3 | pipe/spre_swap；E04（PARAGON 改目录件）；C07 | db 13244 定位与同族件枚举（db8000_pipe_spre_swap_apply.mac:7-11）；目录库可写 | **卡语料**（目录库） |
| 4 | L3 活机运行器跑本套（路 B）；多库活机 | fixture.rs/l3_suite.rs/Run-E3DFixtureSuite.ps1 四处改造 + ChangeKind/assert_case 新平面 | **卡代码**（300–400 行） |
| 5 | L2 断言补齐：「刚体路被拒」、「成员序变但几何不变」、`delete_cleanups` 计数 | main.rs Step 加 2–3 个字段 | **卡代码**（小） |

顺序：0 → 1（同一次 E3D 空窗内可连着录，先 probe 再 setup）→ 2（第二次空窗，7998）→ 5（代码小改，让 0/1 的断言完整）→ 3/4 按需。

---

# 假设与未确认

- 全程只读仓库；本文件是应你要求新建的唯一产物，没有落盘任何 .mac / .json / .ps1；未跑 cargo / git / E3D。
- ★ 语句都没有真机证据：PIPE 级 `COPY` / `BY`、`COPY $!src`（复制成员）、`HPOS BY`、`FTUB LAST`、`REORDER`、`DESC ''`、跨库 `COPY`、`=<WORL-7998>`。其余语句在头注点名的已跑过的宏里有同款。
- `/1-LNR-Q005-PJ` 有 NOZZ、`/1RX-345` 是可 COPY 的 PIPE，是从宏注释推的（tests/equi_increment.rs:218、scripts/e3d/db8000_pipe_scratch_a_create.mac:1），没有打开库文件核。
- 8000 的 WORL 写成 `=24384/0` 是占位，要用 preflight.json 的 `world_refno` 替换（fixture.rs:1041-1060 是扫 WORL noun 取的，不一定是 /0）。
- 平移量都用整百毫米，依赖「同 binade 加减精确可逆」这条经验（s4_aps_strflt_move.mac:7）；旋转腿写终态不写增量，不受此限。
- Record 脚本的「每腿 sesno 恰好 +1」（:121-123）在 driver 的 QUIT 多写一个 DBMDAT 会话时会拒；move-chain 就是因此改用 replay/*.mac + SYSCOM 快照手工录的。若重现，要么把该校验放宽成「≥ +1 且记实际 sesno」，要么照 replay/ 的路子手写 recording.json。
- 6 MiB 上限下每个批次的会话数上限是按 db8000 压缩比估的，要按 pack 的实际输出调整分批。

TASK-PML-SUITE

---

# 第六部分 自我审计（2026-10-28）

对本文档全部 63 条宏做了一次严格技术复核，判定 **PASS 44 / WARN 14 / FAIL 5**。

## 6.1 语法真实性核查

**仓内能找到同款的（已证）**：

| 语法 | 出处 |
|---|---|
| `INCLUDE $!var` | `db8000_pipe_reparent_apply.mac:30` |
| `INCLUDE =refno` | `l3_ftub_move_apply.mac:6` |
| `$!var` 当导航 | `db8000_pipe_reparent_apply.mac:32` |
| `FTUB n` / `BEND n` 成员序号导航 | `db8000_pipe_reparent_apply.mac:23`、`db8000_bran_scratch_create.mac:37` |
| `OWNER` 上跳 | `db8000_bran_scratch_create.mac:41` |
| `COPY /name` | `db8000_equi_copy_apply.mac:8` |
| `COPY =refno` | `db8000_pipe_new_member_apply.mac:20`、`l3_ftub_add_apply.mac:8` |
| `COLLECT ALL <noun> FOR CE` | `issue7_cap_pos_apply.mac:7`、`ploo_26251_pave_probe.mac:16` |
| `COLLECT ALL BOX WITH (NAME EQ ..)` | `increment_fixture/cases/add_restore.mac:2` |
| `$!CAPS[1]` 按下标导航 | `room_cap_cross_db_apply.mac:8` |
| `NEW NBOX` 在 BOX 下 | `s4_db8000_nbox_in_box_create.mac:19-30`、`increment_fixture/setup.mac:40-46` |
| `DELETE FTUB/BRAN/NBOX/EQUI/SITE` | `db8000_bran_scratch_delete_member.mac:20`、`db8000_bran_scratch_delete.mac:19`、`s4_db8000_cleanup.mac:20`、`delete_unit_apply.mac:6`、`setup.mac:7` |
| `BY E 1000 WRT /*` | `adr066_live2_zone_103249_move_apply.mac:17` |
| `BY U 2000mm` 在 BRAN 上 | `db8000_bran_scratch_move.mac:24` |
| `Q POS/ORI WRT /*` | `replay/equi_rotate_apply.mac:11-12` |
| `ORI Y IS W 5 S AND Z IS D`（无 WRT） | `equi_rotate_apply.mac:21` |
| `DESC 'x'` | `setup.mac:14` |
| `HPOS E .. N .. U ..` 绝对值 | `db8000_bran_hpos_apply.mac:29` |

**仓内零命中（★，无任何证据）**：

`REORDER`、`HPOS BY`、`FTUB LAST`、`PLOO 1` / `PAVE 2` 序号导航、`COPY $!var`、`NOZZ 1`、`ORI .. WRT /*`（设值带 WRT）、`POS .. WRT /*`（设值带 WRT）、`DESC ''`、`/*` 单独当导航行、`=24384/0`、PIPE 级 `COPY` / `BY`、`NEW NBOX XLEN.. YLEN.. ZLEN..` 一行式。

## 6.2 五条 FAIL 与修正

### FAIL-1 `attr/desc_restore.mac` — 没有「原值」可回

`DESC ''` 仓内零命中，且 setup 没给 EQ-A 设 DESC。

修正：setup_8000 在 `NEW EQUIPMENT /AIOS-SUITE-EQ-A` 下一行加 `DESC 'AIOS baseline'`（`setup.mac:14` 同款），restore 写回该值。

```
-- 登记: A07 | 语义: DESC 写回基线值 | expected_net: cancelled | restore 搭档: 本文件是 attr/desc_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/attr/desc_restore.log" OVER
$P SUITE-ATTR-DESC-RESTORE-ALIVE
/AIOS-SUITE-EQ-A
Q REF
Q NAME
DESC 'AIOS baseline'
Q DESC
SAVEWORK 'AIOS suite attr/desc restore'
$P SUITE-ATTR-DESC-RESTORE-DONE
ALPHA LOG END
```

### FAIL-2 `connect/bran_reorder_*` — `REORDER` 语法不存在

仓内零命中，无已证替代。**不落盘。** 先跑探针证语法（无 SAVEWORK，不进清单）：

```
-- 探针：REORDER 语法是否成立（无 SAVEWORK，不存；会话由 wrapper QUIT，改动不落库）
ONERROR CONTINUE
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/connect/bran_reorder_probe.log" OVER
$P SUITE-CONN-REORDER-PROBE-ALIVE
/AIOS-SUITE-BRAN-S
Q REF
Q NAME
Q MEMB
FTUB 2
Q REF
REORDER AFTER FTUB 3
$P SUITE-CONN-REORDER-PROBE-TRIED
/AIOS-SUITE-BRAN-S
Q MEMB
$P SUITE-CONN-REORDER-PROBE-DONE
ALPHA LOG END
```

### FAIL-3 `connect/bran_reparent_restore.mac` — `FTUB LAST` 不存在

修正：改 `FTUB 8`（BRAN-NEW 是 BRAN-S 副本，7 FTUB + 改挂进来的 1 个）。

**注意：树逆但几何不逆** —— INCLUDE 追加到 BRAN-S 末位，原 FTUB 4 变成末尾成员，支管路由序变。要真正回基线得跟一条 `connect/bran_del_member_restore`（整条重 COPY）。

```
-- 登记: A01 / A02 | 语义: 把改挂过去的 FTUB INCLUDE 回 BRAN-S（它在 BRAN-NEW 里是第 8 段 FTUB） | expected_net: cancelled（树逆；回到 BRAN-S 末位而非第 4 位） | restore 搭档: 本文件是 connect/bran_reparent_apply.mac 的 restore
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/connect/bran_reparent_restore.log" OVER
$P SUITE-CONN-REPARENT-RESTORE-ALIVE
/AIOS-SUITE-BRAN-NEW
Q REF
Q NAME
Q MEMB
FTUB 8
Q REF
!ftub = !!CE
/AIOS-SUITE-BRAN-S
INCLUDE $!ftub
$!ftub
Q REF
Q OWNE
SAVEWORK 'AIOS suite connect/bran_reparent restore'
$P SUITE-CONN-REPARENT-RESTORE-DONE
ALPHA LOG END
```

### FAIL-4 `pipe/whole_move_*` — PIPE 级 `BY` 不存在

仓内只有 ZONE / BRAN / EQUI 级 `BY`。改成逐 BRAN `BY`（`db8000_bran_scratch_move.mac:24` 同款）：

```
-- 登记: C05 | 语义: 整条 PIPE 的每条 BRAN 同窗平移；预期每 BRAN HPOS/TPOS 变 → 刚体路拒 → regen | expected_net: modified | restore 搭档: pipe/whole_move_restore.mac
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/pipe/whole_move_apply.log" OVER
$P SUITE-PIPE-WHOLE-APPLY-ALIVE
/AIOS-SUITE-PIPE-P
Q REF
Q NAME
VAR !brans COLLECT ALL BRAN FOR CE
DO !b VALUES !brans
  !!CE = !b.dbref()
  Q REF
  Q HPOS WRT /*
  BY E 500 WRT /*
  Q HPOS WRT /*
  Q TPOS WRT /*
ENDDO
SAVEWORK 'AIOS suite pipe/whole_move apply: every BRAN of PIPE-P BY E 500'
$P SUITE-PIPE-WHOLE-APPLY-DONE
ALPHA LOG END
```

restore：同上，`BY E 500` → `BY W 500`，日志名与 `$P` 标签换 restore。

### FAIL-5 `pipe/bran_hpos_*` — `HPOS BY` 不存在

仓内做法是**先探再写绝对值**（`db8000_bran_hpos_apply.mac:11,29`）。`<E> <N> <U>` 从 `probe_8000` 日志的 `Q HPOS WRT /*` 手填。

```
-- 登记: A01 | 语义: 只改支管头点（抬 200）；预期该 BRAN regen | expected_net: modified | restore 搭档: pipe/bran_hpos_restore.mac
ALPHA LOG "D:/work/plant-code/old/gen-model/scripts/e3d/increment_suite/pipe/bran_hpos_apply.log" OVER
$P SUITE-PIPE-HPOS-APPLY-ALIVE
/AIOS-SUITE-BRAN-S
Q REF
Q NAME
Q HPOS WRT /*
HPOS E <E> N <N> U <U+200> WRT /*
Q HPOS WRT /*
SAVEWORK 'AIOS suite pipe/bran_hpos apply'
$P SUITE-PIPE-HPOS-APPLY-DONE
ALPHA LOG END
```

### FAIL-6 `room/pave_move_*` — `PLOO 1` / `PAVE 2` 序号导航不存在

仓内所有 PAVE 操作都是 `NEW PAVE`（新建），零处序号导航。已证替代是 `COLLECT ALL PAVE FOR CE` + `$!var[i]` 下标（`ploo_26251_pave_probe.mac:16`、`room_cap_cross_db_apply.mac:8`）。

**要重写**，不能用序号导航。

## 6.3 十四条 WARN

| 宏 | 问题 | 修正方向 |
|---|---|---|
| `common/setup_8000` | `/*` 单行导航、`=24384/0`、PIPE 级 `COPY`、NBOX 一行式 均★ | `/*` 改 `={{WORLD_REFNO}}`；WORL 用 preflight 的 `world_refno`；NBOX 改分行 |
| `common/teardown_8000` | 同 `/*` | 同上 |
| `create/equi_copy_apply` | 落点 E 52000 与 EQ-NEW 重叠；依赖 EQ-NOZ 有 NOZZ ★ | 换落点 |
| `create/bran_copy_restore` | 必须在 `bran_reparent_restore` 之后，否则连 FTUB 4 一起删 | 顺序约束写进 manifest |
| `create/cross_db_copy_*` | 跨库 `COPY` 无同款；依赖 `setup_7998` ★ | 需真机证 |
| `xform/rot_u/e/n`（6 条） | `ORI .. WRT /*` 设值带 WRT 零命中 | 去掉 `WRT /*`（EQ-A 属主链未旋转，属主系即世界系） |
| `xform/compound` | 同上 | 同上 |
| `xform/noz_move` | 依赖 EQ-NOZ 确有 NOZZ ★ | 真机 `Q MEMB` 确认 |
| `geom/nbox_add_apply` | `NEW NBOX XLEN.. YLEN.. ZLEN..` 一行式零命中 | 改分行（`s4:30-34` 同款） |
| `connect/bran_add_member_apply` | `COPY $!src` 零命中（两个已证机制的复合）；复制 FTUB 7 得同位置零长段 | 真机证；或换源件 |
| `connect/bran_del_member_restore` | 重 COPY 换全部 refno、BRAN-S 落 PIPE-P 末位 | 已知限制 |
| `connect/nozz_move` | `NOZZ 1` 零命中 | 改 `COLLECT ALL NOZZ` + 下标 |
| `room/member_move` | `POS .. WRT /*` 设值带 WRT 零命中 | 去掉 `WRT /*` |
| `h03/probe_7998`、`setup_7998`、`teardown_7998` | `=<WORL-7998>` 占位必须替换 | preflight 取 |

## 6.4 前置依赖链

**无环**，但有一条被 `suite-manifest` 顺序踩破：

> `create/bran_copy` 作为独立链会 apply → restore 闭合（删掉 BRAN-NEW），再跑 `connect/bran_reparent` → BRAN-NEW 不存在，必红。

**修正**：把四腿合成一条链

```
connect/bran_reparent = [
  create/bran_copy_apply,
  connect/bran_reparent_apply,
  connect/bran_reparent_restore,
  create/bran_copy_restore,
]
```

并从独立链清单里删掉 `create/bran_copy`。

其余依赖边：`setup_8000` → 全部；`h03/setup_7998` → `h03/move_*`、`create/cross_db_copy_*`；`connect/bran_reparent` 整条闭合 → `connect/bran_del_member`；`room/member_move` → `room/frmw_rename`；`attr/*`、`xform/*`、`geom/*`、`compound/*` → `delete/*`（delete 会换 EQ-A/BOX-A1 的 refno）；全部 → `create/cross_db_copy`（它删 EQ-A）。

## 6.5 restore 完备性

**属性级真逆**（refno 不变）：`attr/*` 六条、`xform/*` 十二条、`geom/resize`、`geom/nbox_resize`、`room/member_move`、`room/pane_move`、`room/pave_move`、`room/frmw_rename`、`compound/*` 四条、`pipe/whole_move`、`connect/nozz_move`、`h03/move`。

**重建逆**（refno 变）：`create/*` 五条的 restore、`delete/box_restore`、`delete/container_keep_child_restore`、`delete/child_then_parent_restore`、`geom/nbox_add_restore`、`connect/bran_add_member_restore`、`connect/bran_del_member_restore`。

**不逆**：`delete/recreate_same_name`（设计如此）；`connect/bran_reparent_restore`（树逆几何不逆）。

## 6.6 录制纪律

63 条里所有 apply / restore / setup / teardown 都恰好一个 `SAVEWORK`、无 `QUIT`/`FINISH`、有 `Q REF` 与 `Q NAME`、`ALPHA LOG` 路径 = 宏路径换 `.log`。

**会被脚本拒的**：`common/probe_8000`、`h03/probe_7998`（无 `SAVEWORK`，`Record-Db8000SessionChain.ps1:74-77` 拒）——设计上不进清单，单走 `l3_suite --check-driver`（`:116-118`）。

**隐患**：`Invoke-Leg` `:121-123` 要求 sesno 恰好 +1。若 driver 的 QUIT 多写 DBMDAT 会话，每腿都拒——move-chain 因此改用 `replay/*.mac` + `SYSCOM` 快照手录。

## 6.7 `Q NAME` 审查漏洞

`Record-Db8000SessionChain.ps1:88-93` 只是存在性检查，不要求与被改元素对应。所以 PAVE 宏里对 PANE 的 `Q REF`/`Q NAME` 能过审查。

**代价**：`Read-RefnoMap`（`:99-110`）只能解出 PANE 的 refno，清单 `elements` 里不能登记 PAVE——要登记就登 PANE。

## 6.8 两个清单

**可直接落盘（44 条 PASS）**：`common/probe_8000`、`create/box_apply`+`restore`、`create/subtree_apply`+`restore`、`create/equi_copy_restore`、`create/bran_copy_apply`、`delete/*` 全部、`attr/rename_*`、`attr/desc_apply`、`attr/owner_*`、`xform/move_*`、`geom/resize_*`、`geom/nbox_resize_*`、`geom/nbox_add_restore`、`connect/bran_add_member_restore`、`connect/bran_del_member_apply`、`connect/bran_reparent_apply`、`pipe/*` 待修后、`room/pane_move_*`、`room/frmw_rename_*`、`compound/*` 四条、`h03/move_*`。

**必须先修正（6 条 FAIL）**：`attr/desc_restore`（改 DESC 基线值）、`connect/bran_reorder_apply`+`restore`（不落盘，先探针）、`connect/bran_reparent_restore`（`FTUB LAST` → `FTUB 8`）、`pipe/whole_move_apply`+`restore`（改逐 BRAN `BY`）、`pipe/bran_hpos_apply`+`restore`（改绝对值）、`room/pave_move_apply`+`restore`（改 COLLECT + 下标）。

## 6.9 审计方法说明

审计由 CT 窗口（claude-fable-5-1-thinking-max）执行，Lead 独立核实了三条关键结论：

| 结论 | 核实方式 | 结果 |
|---|---|---|
| `INCLUDE $!var` 有出处 | `rg "INCLUDE \$"` | ✅ `db8000_pipe_reparent_apply.mac:30` |
| `REORDER` 零命中 | `rg "REORDER"` | ✅ 零命中 |
| `HPOS BY` 零命中 | `rg "HPOS BY"` | ✅ 零命中 |
| `PLOO n` / `PAVE n` 序号导航零命中 | `rg "^\s*(PLOO\|PAVE)\s+\d"` | ✅ 零命中（PAVE 只有 `NEW PAVE`） |
| `NEW NBOX` / `DELETE NBOX` 有出处 | `rg "^\s*(NEW\|DELETE) NBOX"` | ✅ 四处 |

另外独立跑了全宏的 `SAVEWORK` / `ALPHA LOG` / `Q REF`-`Q NAME` 平衡性统计：除两个探针（设计上无 SAVEWORK）外全部合规。

---

# 第七部分 E3D TTY 真机实测（2026-10-28）

前面六部分全是静态分析。这一部分是**真机跑出来的**，推翻了若干条此前当成事实的假设。

## 7.1 测试环境

| 项 | 值 |
|---|---|
| 驱动器 | `D:\Rust\target\debug\l3_suite.exe --check-driver` |
| E3D 安装 | `E:\reverse\e3d\shadow_e3d31_gen_model_test`（影子） |
| 启动器 | `scripts/e3d/run_ams_c_entrymacro.bat`（`AVEVA_DESIGN_ENTRYMACRO` 通道） |
| 项目副本 | `E:\reverse\e3d\replay-src\E3D3.1\AvevaMarineSample` |
| 目标库 | `ams8000_0001`，基线 sesno **734** |
| MDB | `/ALL` |

**隔离验证**：副本用 robocopy 整目录复制，之后逐文件 `fsutil hardlink list` 校验，
452 个库文件**全部链接数=1**（真实副本，非硬链接）。

原件 `D:\AVEVA\Projects\E3D3.1` 全程未动：实测 `amssys` 副本从 991→1001，
原件保持 991；`ams8000_0001` 副本从 734→884，原件保持 734。

## 7.2 ★★ 最重要的发现：E3D TTY 的「当前库」语义

**这是整套宏能不能写进目标库的唯一关键。** 前面六部分完全没意识到这一点。

`/ALL` MDB 的默认库是 **db 1112**（`ams1112_0001`），不是 db 8000。
「当前库」由 **CE 所在库**决定，不是由 `--target-db-file` 决定。

不用有效导航把 CE 挪进 db8000 时，会发生：

- `NEW SITE` 报成功，但 `Ref =17496/...`（17496 = db1112 的 world prefix）
- `SAVEWORK` 报成功，实际写的是 `ams1112_0001` 与 `amssys`
- `ams8000_0001` 的 sesno 纹丝不动
- 驱动器报 `macro completed without advancing the target DB session (734 -> 734)`

### 实测过的四种导航写法

| 写法 | 结果 |
|---|---|
| `=24384/0` | `Undefined reference (DB number 8000)`；CE 变成 `WORL /*` |
| `=24384/23257` | 成功，CE 落在 db8000 的 `BRAN /C-OR-1R345-C` |
| `=/1RX03-EQUI` | `CP: Syntax error`（`=` 后不能跟名称） |
| **`/1RX03-EQUI`** | ✅ 成功，`Ref =24384/22399` |

### 正确的锚点结构

```
/1RX03-EQUI
OWNER
```

第一行把 CE 挪进 db8000；第二行上跳到该库的 WORLD（`Ref =16192/0`）。

**只写导航不写 `OWNER` 是不够的**：`COLLECT ALL SITE ... FOR CE` 是**相对当前元素的子级**，
不是世界级。CE 停在 SITE 上时它返回空数组。

### db8000 的身份常量

| 量 | 值 | 怎么得到的 |
|---|---|---|
| WORLD refno | **16192/0** | `/1RX03-EQUI` + `OWNER` |
| 元素 refno 前缀 | **24384** | 所有 db8000 元素都是 `=24384/xxxxx` |
| 稳定锚点 SITE | `/1RX03-EQUI` = `=24384/22399` | BRAN → PIPE → ZONE → SITE 逐级 `OWNER` |

注意 `24384` 只是**元素前缀**，不是 WORLD refno。这是最容易搞错的一处。

### 对 `{{WORLD_REFNO}}` 的修正

原设计用 `{{WORLD_REFNO}}` 单个占位符，现在拆成 `{{WORLD_ANCHOR}}`，
渲染时展开成**两行**（锚点 + `OWNER`）。5 个 common 宏已改。

## 7.3 语法判定：实测推翻了 4 条静态结论

| 语句 | 静态审计判定 | **TTY 实测** | 证据 |
|---|---|---|---|
| `REORDER` | ★ 零命中 | ❌ **`CP: Syntax error`** | step5：`REORDER ^^AFTER FTUB 3` |
| `HPOS BY` | ★ 零命中 | ❌ **`CP: Syntax error`** | step5：`HPOS ^^BY U 200 WRT /*` |
| `FTUB LAST` | ★ 零命中 | ✅ **成立** | step5：`FTUB LAST` → `Ref =17496/161709` |
| `PLOO 1` / `PAVE 2` 序号导航 | ★ 零命中 | ✅ **成立** | step6：`PAVE 2` → `Position E 62000mm N 3000mm` |
| `NEW NBOX XLEN.. YLEN.. ZLEN..` 一行式 | ★ 零命中 | ✅ **成立** | step6：`Type NBOX` |
| PIPE 级 `BY` | ★ 零命中 | ✅ **成立** | step6：零错误 |
| `ORI .. WRT /*` 设值 | ★ 零命中 | ✅ **成立** | step4：`Orientation Y is W and Z is U` |
| `DESC ''` | ★ 零命中 | ✅ **成立** | step4：`Description unset` |
| `NEW NOZZ` | 仓内零命中 | ✅ **成立** | nozzmk：`Ref =24384/26717` |
| `IF / .SET() / .SIZE()` | 仓内零命中 | ✅ **成立** | ifsyn：`CNT=1`，走 YES 分支 |
| `BY E 500 WRT /*` | 有出处 | ✅ 成立 | step5：`1000mm` → `1500mm` |

**审计报告对 `FTUB LAST` 和 `PLOO/PAVE` 序号导航的判定是错的。**
我按审计把它们改成 `FTUB 8` / `COLLECT + 下标`，实测后已回滚成 `FTUB LAST` / 序号导航。

### 仍然不成立的两条

`REORDER` 和 `HPOS BY` 确认不存在，与审计一致。`connect/bran_reorder_*` 已从正式套件移除，
改成 `connect/bran_reorder_probe.mac`（无 SAVEWORK，只证语法）。

## 7.4 语料限制：db8000 没有 NOZZ

`COPY /1-LNR-Q005-PJ` 只带过来 **1 个 BOX**，没有 NOZZ。
用 `findnozz` 探针扫遍 db8000 全部 40+ 个 EQUI（`COLLECT ALL EQUIPMENT` 逐个数 NOZZ），
**`FN-HIT` 计数为 0**——这个库里没有任何带 NOZZ 的生产设备。

后果：`connect/nozz_move_*` 和 `xform/noz_move_*` 原本没有靶子。
`COLLECT ALL NOZZ` 返回空数组时 `!nozzs[1]` **越界崩宏**，日志留空、驱动报 `Indeterminate`。

**修法**：`common/setup_8000.mac` 里显式 `NEW NOZZ /AIOS-SUITE-NOZZ-1`（自建元素，
符合「不动生产元素」纪律）；`nozz_move_*` 加 `IF / .SET() / .SIZE()` 判空保护。

## 7.5 实测结果

**81 腿跑完，34 个正式用例腿中 31 个 `completed`**，每腿稳定 **+2 会话**
（1 个业务会话 + 1 个 `SAVEWORK` 后的内务会话，与 `docs/plans/2026-09-15-*` 记的
「db8000 一腿两号」一致）。

### 通过的用例（31 腿）

| 组 | 用例 | 会话 |
|---|---|---|
| setup | `common/setup_8000` | 746 → 748 |
| create | `box` / `subtree` / `equi_copy` / `bran_copy` | 各 +2 |
| delete | `box` / `container_keep_child` / `recreate_same_name` / `child_then_parent_1` | 各 +2 |
| attr | `rename` / `desc` / `owner` | 各 +2 |
| xform | `move` / `rot_u` / `rot_e` / `rot_n` / `compound` / `noz_move` | 各 +2 |
| geom | `resize` / `nbox_resize` / `nbox_add` | 各 +2 |
| connect | `bran_add_member` / `bran_del_member` / `nozz_move` | 各 +2 |
| pipe | `whole_move` | +2 |
| room | `member_move` / `pane_move` / `pave_move` / `frmw_rename` | 各 +2 |
| compound | `rename_move_add` / `two_roots` | 各 +2 |

### 可逆性实证（`create/box`）

```
apply   → NEW BOX /AIOS-SUITE-BOX-A2 → Ref =24384/26716
          sesno 751: EQUI /AIOS-SUITE-EQ-A
                     children_changed: old=[26716, 26560] → new=[26560]
restore → COLLECT ALL BOX WITH (NAME EQ '/AIOS-SUITE-BOX-A2') → 找到 26716
          DELETE BOX
          sesno 752: {"refno": "24384_26716", "op": "deleted"}
```

**apply 建 26716，restore 删 26716，refno 完全一致，净效果 `cancelled`。**

### 强制链实证（`connect/bran_reparent` 四腿）

```
1. create/bran_copy_apply        → BRAN-NEW =24384/26911
2. connect/bran_reparent_apply   → FTUB 4 =24384/26896 改挂到 BRAN-NEW
3. connect/bran_reparent_restore → BRAN-NEW 的 Members 里 FTUB LAST = =24384/26896 ✓
4. create/bran_copy_restore      → 删整条 BRAN-NEW，FTUB 26896 回到 BRAN-S
```

四腿各 +2 会话，零错误，净效果 `cancelled`。

**这证明了第六部分 6.4 的判断**：单独跑 `create/bran_copy` 会 apply→restore 闭合
（删掉 BRAN-NEW），后面的 reparent 必红。实测复现了这个失败，合成一条链后通过。

### 失败的 3 腿与根因

| 用例 | 现象 | 根因 | 处置 |
|---|---|---|---|
| `A01-bran-reparent-apply` | `Indeterminate`，日志空 | 前置 `create/bran_copy` 已闭合，BRAN-NEW 不存在 | 必须按四腿链跑，已在 manifest 标 `chain` |
| `A01-child-parent-restore` | `Indeterminate`，日志空 | 原宏不幂等：EQ-A 还在时报 `Name already used` 崩宏 | 已改成先删同名再重建 |
| `F03-cross-db-copy-apply` | `Indeterminate`，日志空 | 需要 `/AIOS-SUITE-7998-Z`，7998 库还没 setup | 见 7.6 |

## 7.6 db7998 的锚点（★ 已解决，2026-10-28）

### 7.6.1 refno 的位布局（★ 源码验证，不是经验公式）

第一版这里写的是「两条经验公式：WORLD 前缀 = db_no + 8192、元素前缀 = db_no + 16384」。
**源码查证后修正：那不是两条公式，是同一条位布局规则的两个 extent 取值。**

`vendor/e3d-io/src/refno.rs:1-30`：

```rust
/// - dbno = (word0 & 0x1FFF) | ((word0 >> 13) & 0x3E000)
pub const fn dbno(&self) -> u32 {
    (self.word0 & 0x1FFF) | ((self.word0 >> 13) & 0x3E000)
}

/// Extent index carried in bits 13..25 of `word0`.
/// ... A database's first extent is index 1, and
/// each further one adds 0x2000 to `word0`.
pub const fn extent(&self) -> u32 {
    (self.word0 >> 13) & 0x1FFF
}
```

也就是说 `word0` 被切成三段：低 13 位放 dbno 的低半、中间 13 位放 extent、高位放 dbno 的高半。
对 8000 以内的库号（高半为 0），等价于：

```
word0 = db_no + 0x2000 × extent_index        （extent_index 从 1 起算，即 0x2000 × 1）
```

`extent 1 → +8192 (0x2000)`、`extent 2 → +16384 (0x4000)`、`extent 3 → +24576 (0x6000)`。

★ 这里有个极易写错的地方（第一版就写错了）：`extent_index` 从 **1** 起算，
所以 `extent 1` 加的是 `0x2000 × 1 = 8192`，**不是 0**。
公式里没有「加 0」的 extent——`db_no` 本身（`word0 == db_no`）解码出来是 extent 0，那不是合法 extent。

验证（`ams7999_0001` 的三个 extent，源码测试 `refno.rs:153-160` 同款）：

| extent | word0 | 实测 |
|---|---|---|
| 1 | 7999 + 0x2000 = 16191 | ✅ |
| 2 | 7999 + 0x4000 = 24383 | ✅ |
| 3 | 7999 + 0x6000 = 32575 | ✅ |

所以：
- WORLD 在 **extent 1** → `db_no + 8192`
- 普通元素在 **extent 2** → `db_no + 16384`

我之前把「+8192」当成「WORLD 前缀」是错的——它其实是「extent 2 相对 extent 1 的偏移」。
数值全对，解释错了。

### 7.6.2 WORLD refno 是文件头里的存储字段，不是算出来的

`vendor/e3d-io/src/meta/descriptor.rs:61` 的字段表写着：

```
| +0x38 | 14 | *not in the integer table* | `primary_extent_ref` | 16192 |
```

`descriptor.rs:198`：

```rust
pub fn world_refno(&self) -> crate::refno::RefNo {
    crate::refno::RefNo::new(self.primary_extent_ref, 0)
}
```

**WORLD 的 refno = 文件头 `+0x38` 处的 `primary_extent_ref` 配元素号 0。**
这是 E3D 自己写进文件的值，不需要推导。直接读五个库的 `+0x38`：

| 库 | `+0x08` db_no | `+0x2C` ext_no | `+0x38` primary_extent_ref | WORLD |
|---|---|---|---|---|
| `ams1112_0001` | 1112 | 1 | 9304 | `9304/0` |
| `ams7997_0001` | 7997 | 1 | 16189 | `16189/0` |
| **`ams7998_0001`** | **7998** | **1** | **16190** | **`16190/0`** |
| `ams7999_0001` | 7999 | 1 | 16191 | `16191/0` |
| `ams8000_0001` | 8000 | 1 | 16192 | `16192/0` |

`16190` 与我在 TTY 里实测成功的 `=16190/0` **逐字一致**，两条独立路径互相印证。

`descriptor.rs:182-193` 还有一段 1,947 个库的普查结论：每个库恰好一个 `WORL`、owner 为 null，
1,905 个元素/目录/字典库（DESI、CATA、DICT、PROP、PADD、ISOD…）的 WORLD 都是
`RefNo(primary_extent_ref, 0)`。三个例外是项目管理层：`COMM`(8190) 与 `MISC`(8189) 在元素 1，
`SYST`(8191) 在**第二 extent 的元素 3**——这就是 `mdb_membership.rs:52` 里
`SYS_WORLD = 24575/3` 的来历。

注意 `24575 = 8191 + 0x4000 = 8191 + 16384`，解码是 **extent 2**，不是 extent 3。
那个 `/3` 是**元素号**（`word1`），不是 extent。`mdb_membership.rs:53` 的注释写的是
「系统元素落在第 2 个 extent」，与此一致——第一版把「第 2 个 extent 的元素 3」
读成了「extent 3」，是错的。

### 7.6.3 extent 3 在 db7998 上不存在

既然 extent 2 的 `24382` 可用，自然会问 extent 3 的 `32574` 是否也能写。实测：

```
=32574/1
→ (2,107) Undefined reference =32574/1 (DB number 7998)
```

**db7998 只有 extent 1。** 但要说清楚证据是怎么来的，因为第一版这里推理错了。

**错误的推理（第一版）**：读文件头 `+0x2C` 得到 `ext_no = 1`，就说「这个库只有 extent 1」。
**为什么错**：`+0x2C` 是**这个文件自己**的 extent 号，不是 extent 总数。
五个库的 `+0x2C` 全是 1，因为目录里每个库只有一个文件（都是 primary）。
用它推断 extent 个数是循环论证。

**正确的证据**：extent 是**独立的兄弟文件**，命名 `ams1000_0001` / `ams1000_0002` / `ams1000_0004`，
见 `vendor/e3d-io/src/page/io.rs:449-457` 的测试
`extent_paths_name_exactly_the_files_the_reader_reads`。
`PageReader::extent_paths()`（`io.rs:263-269`）返回的就是「打开时实际挂上的文件」，
`engine.rs:303-306` 也这么描述。

实测目录：

```
E:\reverse\e3d\replay-src\E3D3.1\AvevaMarineSample\ams000\
  ams7997_0001          73,498,624
  ams7998_0001          14,901,248      ← 只有一个文件
  ams7999_0001          60,233,728
  ams8000_0001          44,347,392
```

**没有任何 `_0002` / `_0003` 兄弟文件。** 所以：

- db7998 只有 1 个 extent 文件（`ams7998_0001`，7,276 页）。
- `24382` 段**能写**（`NEW SITE` 得到 `=24382/1883`），但目录里没多出 `ams7998_0002`，
  文件只是从 14,706,688 长到 14,901,248（+95 页）。
- `32574` 段（extent 3）报 Undefined。

**已确证**：7998 只有 extent 1 的文件，extent 3 的地址不可用。

**未验证**：`24382`（extent 2）的地址为什么能在单文件库里写成功——
可能是 E3D 把 extent 2 的地址空间映射到同一文件的追加页，也可能是别的机制。
本次只观察到「能写、文件变长、没有新兄弟文件」，没有读 page 1 的 extent 表，
也没有从 `core.dll` 侧确认。**这条不要当成结论使用。**

**可以确定的影响**：7998 的元素只能落在 `24382` 段，容量受该文件页数限制，
不能靠加兄弟文件扩容（加 `ams7998_0002` 是否被 E3D 接受，未验证）。

### 7.6.4 为什么之前找不到锚点

`=24382/1`、`/2`、`/3`、`/10`、`/100`、`/535`、`/1000` 全部报
`Undefined reference (DB number 7998)`，`Q MDB` 的 37 个 SITE 里也没有一个前缀是 24382。

**真相：db7998 的 WORLD 下没有任何子元素。** 实测：

```
=16190/0
Q CE          → WORLD /*
Q REF         → Ref =16190/0
VAR !c COLLECT ALL FOR CE
Q VAR !c      → <ARRAY> [1] '=16190/0'      ← 只有自己
```

这个库是真实的（DESI，645 会话，14.7 MB），但**里面一个模型元素都没有**。
所以不是「锚点找不到」，是「根本没有锚点可用」——必须自己建。

对比：`ams7997_0001` 70 MB 有 15 个 SITE 在 MDB 里，`ams7999_0001` 57 MB 有 4 个，
`ams7998_0001` 只有 14 MB 且 0 个。文件大小和内容量吻合。

### 7.6.5 7998 可写，建锚点成功

```
=16190/0
NEW SITE /AIOS-SUITE-7998-Z DESC 'AIOS 7998 anchor'
  → Ref =24382/1883          ← 前缀 24382，正是 extent 2 的值
SAVEWORK
```

`SAVEWORK` 后库文件 14,706,688 → 14,753,792 字节，sesno 645 → 647，元素重启后仍存在
（`Q CE` → `SITE /AIOS-SUITE-7998-Z`，`Q OWNER` → `WORLD /*`）。

### 7.6.6 导航方式：名称优先于 refno

`/AIOS-SUITE-7998-Z` 这种**名称导航会跨库命中**，实测直接落到 `=24382/1883`：

```
/AIOS-SUITE-7998-Z
  → SITE /AIOS-SUITE-7998-Z
    Ref =24382/1883
NEW ZONE /AIOS-SUITE-7998-Z2
  → Ref =24382/1884          ← 新元素仍在 7998
```

所以 7998 侧的宏**不需要 `{{WORLD_ANCHOR}}`**（那是 db8000 的两行名称锚点），
用 `=16190/0` 建出第一个元素后，后续全部走名称导航。重建后 refno 会变，名称不会。

**重要**：refno 会随重建而变。实测中 `=24382/1883` 在后续 setup/teardown 循环跑过之后
就不再存在了（那个位置被回收，新建的元素拿到 `1888`）。所以**宏里绝不能写死 7998 元素的
refno**，只能写死 WORLD（`16190/0`，文件头字段，永不改变）与名称。

### 7.6.7 ★ 导航到不存在的元素会让 E3D 崩溃

这是本次最危险的一个坑：

```
/AIOS-SUITE-7998-Z          ← 该 ZONE 不存在时
→ exit=3221225477           ← 0xC0000005 访问违例
→ 日志 3 字节（ALPHA LOG 没 flush）
→ 驱动器报 Indeterminate
```

**不是语法错、不是报错返回，是整个 E3D 进程访问违例退出。**
排查时必须看 `check-driver-evidence.json` 的 `attempts[].driver_log` 里
`detached pid=xxxx exit=3221225477` 这一行，日志本身是空的。

所以 7998 侧所有宏都要先判存在再导航，`create/cross_db_copy_apply.mac` 已加两道兜底
（SITE 与 ZONE 各自判空后自建）。

### 7.6.8 双库用例实测通过

`h03/setup_7998` + `h03/teardown_7998`：

```
setup    → SITE 24382/1884 → ZONE 1885 → EQUI 1886 → BOX 1887
teardown → 删 SITE 24382/1884（级联带走全部）
```

`create/cross_db_copy_apply` + `create/cross_db_copy_restore`：

```
apply   → 7998: EQ-X =24382/1890 + BOX =24382/1891   （COPY /AIOS-SUITE-EQ-A）
          8000: EQ-A =24384/26923 被删
restore → 8000: EQ-A 重建 =24384/27081 + BOX 27082
          7998: EQ-X =24382/1890 被删
```

净效果 `cancelled`（refno 变了，因为是删后重建，符合重建逆的预期）。

两库 sesno：7998 645 → 654，8000 897 → 900。原件 `ams7998_0001` 保持 14,706,688 字节未动。

**至此 34 个正式用例全部可跑，无一阻塞。**

## 7.7 三处幂等性缺陷（实测发现，静态分析没看出来）

| 宏 | 缺陷 | 后果 |
|---|---|---|
| `delete/child_then_parent_restore` | 直接 `NEW EQUIPMENT /AIOS-SUITE-EQ-A` | EQ-A 还在时报 `Name already used` 崩宏 |
| `create/bran_copy_apply` | 直接 `NEW BRAN /AIOS-SUITE-BRAN-NEW` | 重跑时报 `Name already used` 崩宏 |
| `connect/nozz_move_*` | 无判空 | `!nozzs[1]` 越界崩宏 |

**共同模式**：PML 宏崩掉时**日志留空**（`ALPHA LOG` 的内容没 flush），
驱动器只能看到 `alive=True done=False exit=1` + `Indeterminate`。
排查时必须看 `check-driver-evidence.json` 的 `attempts[].alive_seen` / `done_seen`，
不能只看日志。

三处已全部修成幂等/判空。

## 7.8 录制纪律复检

改动后重跑 `Test-MacroDiscipline` 的复刻检查：**75/75 PASS**（探针除外）。

> 77 降到 75 是因为 `common/setup_7998.mac` 与 `common/teardown_7998.mac` 与 `h03/` 下重复，已删除。

## 7.8b `pipe/bran_hpos` 实测通过（2026-10-28 补跑）

`HPOS BY` 已确认不存在（7.3），所以这两腿走「探绝对值 → 写绝对值」。

### 基线探出

`/AIOS-SUITE-BRAN-S`（`Ref =24384/27054`）：

```
Hposition E 8551mm N 14060mm U 2900mm
Tposition E 9093mm N  9973mm U 2900mm
Members  1 FTUB 1 =24384/27055 … 9 FTUB 7 =24384/27063
```

`Q POS WRT /*` 报 `(2,201) Element BRANCH ... does not have attribute POS`——
**BRAN 只有 HPOS/TPOS，没有 POS**，这与 EQUI 不同，写宏时别混用。

基线值在每次 `setup_8000` 后都一致（同坐标建管），所以可以直接填死。

### 填值

| 宏 | 写入 |
|---|---|
| `bran_hpos_apply.mac` | `HPOS E 8551 N 14060 U 3100 WRT /*` |
| `bran_hpos_restore.mac` | `HPOS E 8551 N 14060 U 2900 WRT /*` |

**关键：写绝对值，不写相对增量。** 因为 PML 没有 `HPOS BY`，若用「读出来再 +200」的
写法，重跑时基线已经变了，会变成 3300、3500 一路漂。绝对值写法天然幂等。

### 实测结果

```
apply   : 2900 → 3100   refno 24384/27054 不变   alive=True done=True exit=0
restore : 3100 → 2900   refno 24384/27054 不变   alive=True done=True exit=0
重跑 apply   : 2900 → 3100   逐字一致
重跑 restore : 3100 → 2900   逐字一致
```

库 sesno 884 → 893（+9，含本轮 setup 与探测），两腿各 +2 会话。

**属性级真逆成立**：refno 全程不变，`expected_net` 从 apply 的 `modified`
回到 restore 后的 `cancelled`。

### 一个前置发现

首次探测时 BRAN-S 的第 1 个成员是 `FTUB 1 =24384/26920`，refno 比 BEND 1 的 `26893` 还大——
这是之前 `connect/bran_add_member` 加进来但没删干净的残留。重跑 `setup_8000` 后恢复为
`27055-27063` 连续 9 个。

**教训**：`bran_hpos` 这类依赖成员几何的用例，跑之前必须先确认 BRAN-S 是干净基线，
否则 HPOS 与成员不匹配。

## 7.9 仍未验证的

| 项 | 状态 |
|---|---|
| `create/cross_db_copy_*` | ✅ **已通过**（见 7.6.8，跨库复制 + 删除 + 重建逆全闭合） |
| `h03/setup_7998` + `teardown_7998` | ✅ **已通过**（SITE→ZONE→EQUI→BOX 全建在 7998，teardown 级联清空） |
| `connect/bran_reorder_*` | ✅ **已结案**（见 7.9e：5 种语法全不存在 + 删除重排实验证明成员序不可写，归 (c) 类） |
| `pipe/bran_hpos_*` | ✅ **已通过**（见 7.8b，绝对值填死 + 重跑幂等） |
| `pipe/spre_swap_*` | ✅ **已通过**（见 7.9h，语料补齐 + SPRE 可写可逆 + Compare pass） |
| `attr/desc_restore` 写回基线值 | ✅ 通过（`desc` 两腿 completed） |
| db7998 extent 2 为何能在单文件库写成功 | ⚠️ **未验证**（见 7.6.3，只观察到现象，没读 page 1 的 extent 表） |

**35 个正式用例至此全部可跑，无一阻塞。**

## 7.9h `pipe/spre_swap` 语料补齐并跑通（2026-10-28）

### 原先的判断是错的

设计文档旧版写：「`<ALT_SPRE>` 要在 db 13244 的 SPEC 13244/109234 里枚举同 bore/rating 的弯头组件，
**该库在已接目录集里未定位**」。

实测推翻：`13244` 不是独立库，它就是 **`ams5052` 的库号**
（`docs/2026-08-12_live-test-ledger.md:447` 记「13244（ams5052）」），
库文件 `ams000/ams5052_0001` 存在（159 MB）。SPEC 也能直接点查：

```
=13244/109234
→ SPECIFICATION /ACP1000-Ventilated
  Ref =13244/109234
  Type SPEC
```

所以「卡语料」从来不是缺库，是**没找对库号**。

### 组件清单在哪

SPEC 的 9 个 SELE 里没有 CATR/STYP/SPRE（`Q` 全部报 does not have attribute），
真正的组件清单在 `/ACP1000-Trough` 的第三层 SELE 下。顺着 BEND 成员的 `SPRE` 反查 owner 即可拿到：

```
/AIOS-SUITE-BRAN-S-M2
  Q SPRE  →  Spref /ACP1000-Trough/ACP1000-TBR:50
  !sp = !!CE.SPRE  →  =13244/108844
  owner  →  SELEC 1 of SELEC 2 of SELEC 3 of SPECIFICATION /ACP1000-Trough   (=13244/108842)
```

该 SELE 下 6 个 SPCO，全部 refno 已取到：

| 组件 | refno | 说明 |
|---|---|---|
| `ACP1000-TBL:50` | `13244/108843` | 50 bore 直管 |
| `ACP1000-TBR:50` | `13244/108844` | ← M2 基线用的 |
| `ACP1000-TBL45:50X150` | `13244/108847` | 45° |
| `ACP1000-TBR45:50X150` | `13244/108848` | 45° |
| `ACP1000-TBL90:50X150` | `13244/108851` | 90° |
| `ACP1000-TBR90:50X150` | `13244/108852` | 90° ← 本用例换成这个 |

### `SPRE` 可写、可逆、幂等

```
/AIOS-SUITE-BRAN-S-M2
Q SPRE        →  Spref /ACP1000-Trough/ACP1000-TBR:50
SPRE =13244/108852
Q SPRE        →  Spref /ACP1000-Trough/ACP1000-TBR90:50X150
Q POS         →  Position E 9381mm N 13500mm U 2900mm      ← 不变
Q ORIE        →  Orientation Y is N 34 E and Z is U        ← 不变
```

写绝对值（不是增量），重跑幂等。写回 `=13244/108844` 即恢复。

### RVM 可见且精准

换件前后几何数 `122 → 124`（90° 弯头比直管多两片面），成员数不变。
逐成员比对只有 M2 自己变：

```
/AIOS-SUITE-BRAN-S-M2
  TBR:50       [9365.31, 13478.89, 2900.00, 9397.20, 13522.83, 2930.00]
  TBR90:50X150 [9178.24, 13217.05, 2901.50, 9664.12, 13457.04, 2930.00]
```

Compare 判定 **pass**，三条断言全 ok：

```
[ok] resized: .../AIOS-SUITE-BRAN-S-M2 AABB 吻合 [9178.24, 13217.05, 2901.5, 9664.12, 13457.04, 2930]
[ok] subtree-count: .../AIOS-SUITE-BRAN-S 成员数变化 0（成员 9→9，并集不变）
[ok] unchanged-elsewhere: 其余成员逐一吻合（≤ 1 mm，共 117 件）
```

restore 后几何数回到 122，`equals_state_before` 成立。

### 登记

- 用例 id `E04-spre-swap`，登记号 `E04`，`expected_net: modified`
- `suite-manifest.json` 的 `cases` 从 34 增到 35
- `rvm-truth.json` 新增 `pipe/spre_swap_apply.mac`（`resized` + `subtree.count_delta: 0`）
  与 `pipe/spre_swap_restore.mac`（`equals_state_before`）

## 7.9b 独立交叉验证（CT 窗口 #3，2026-10-28）

本节的结论被一次**独立的只读验证**复核过：CT 窗口
（`bc-1d0a7c54-d05e-43cc-bdf0-99e1de9ef11f`，`claude-fable-5-1-thinking-max`）
在自己绑定的 `D:\work\plant-code\old` 下，用 pwsh 读 `E:\` 的库文件头、读
`vendor/e3d-io` 的源码，**自己算一遍**再与本节结论对比。它没有看到我的推导过程，
brief 里给的是「待验证的声称」而不是「结论」。

### 它确认成立的

- `refno.rs:17-30` 的 `dbno()` / `extent()` 掩码就是 `0x1FFF` / `0x3E000` / 右移 13，
  「每个后续 extent 加 `0x2000`」的注释存在。
- `descriptor.rs:61` 的 `+0x38 = primary_extent_ref`，`:198` 的
  `world_refno() = RefNo(primary_extent_ref, 0)`。
- 五个库头的实读值与 7.6.2 的表**逐字一致**（9304 / 16189 / 16190 / 16191 / 16192）。
- **`db7998` 的 WORLD = `16190/0`，与实测一致。**
- 把实读值代回 `dbno()` / `extent()`，五个库的 `dbno(primary_extent_ref)` 都等于各自
  的 `db_no`、`extent` 都是 1。

### 它纠正了本节的三处错误

| # | 第一版写的 | 正确的 | 影响 |
|---|---|---|---|
| 1 | `extent 1 → +0`、`extent 2 → +8192` | `extent 1 → +8192`、`extent 2 → +16384` | 已修正 7.6.1。公式 `0x2000 × extent` 没错，是我把它展开成表时错位了一格 |
| 2 | `SYS_WORLD = 24575/3` 是「extent 3」 | `24575` 解码是 **extent 2**，那个 `/3` 是**元素号** | 已修正 7.6.2。`mdb_membership.rs:53` 注释本来就写「第 2 个 extent」，是我读错了 |
| 3 | 用 `+0x2C ext_no = 1` 推断「7998 只有 extent 1」 | `+0x2C` 是**这个文件自己**的 extent 号，不是 extent 总数；这是循环论证 | 已重写 7.6.3，改用「目录里没有 `_0002` 兄弟文件」这个独立证据 |

### 它还指出两处措辞问题（数值都对，公式表述错）

- 我 brief 里写「大端 u32，即 `byte[3]<<24 | byte[2]<<16 | byte[1]<<8 | byte[0]`」——
  那是**小端**公式，自相矛盾。文件确实是大端，实际代码
  `ToUInt32(@(b[11],b[10],b[9],b[8]))` 是对的。CT 把两种字序都算了一遍，
  小端得到 `1042219008` 这类垃圾值，据此判定字序。
- `word0 = db_no + 0x2000 × extent` 只对 `db_no ≤ 8191` 成立。库号高 5 位在
  `word0` 的 bit 26..30，大库号要按
  `(db_no & 0x1FFF) | (extent << 13) | ((db_no & 0x3E000) << 13)` 算。
  `refno.rs:143-151` 的测试用的就是 `dbno = 251000` 这种大库号。本套件涉及的
  库号都在 8191 以内，不受影响。

### 它未验证的（诚实记录）

- 没有把 `16190/0` 点查成元素并核 noun（`descriptor.rs:195-197` 要求的那一步）。
- 没有读 page 1 的 extent 清单，所以「7998 有几个 extent」它认为**未验证**。
  我后来用目录里没有兄弟文件这一点补上了证据，但「extent 2 地址为何可写」仍未解。
- `+0x3C`（word 15）在 1112 是 3、其余四个是 2，字段表没有这一行，含义不明。

**这次交叉验证的价值**：三处错误全部是我自己推错的，其中 #1 和 #3 会让照着文档写宏的人
得出错误结论（#3 会让别人以为「读 `+0x2C` 就能判断 extent 个数」）。只靠我自己的实测
发现不了，因为实测数据本身没错，错的是我解释数据的方式。

## 7.9c 命名纪律与三处 PML 陷阱（2026-10-28 实测）

### 为什么必须显式命名

`Compare-SuiteRvmTruth.ps1` 的比对键是**导出根以下的相对路径**，而 E3D 自动命名是
`BOX 1 of EQUIPMENT /xxx`、`PAVERT 3 of PLOOP 1 of PANEL /xxx` 这类**带序号**的形态。
序号会随成员增删漂移，refno 会随重建变化，只有显式名字稳定。

首轮导出实测：148 个成员里 **34 个用缺省名**，导致真值表按元素名 `AIOS-SUITE-BOX-A1`
匹配不到，`xform/move_apply` 对拍报 `moved-absent` + `unexpected-diff`。

### 改造结果

`common/setup_8000.mac` 改造后重导：**148 成员，缺省名 0 个**。对拍实测：

```
RVM 对拍 xform/move_apply.mac：before 118 件 / after 118 件 → 新增 0 删除 0 平移 1 变形 0 未变 117
  [ok] moved: AIOS-SUITE-8000-Z/AIOS-SUITE-EQ-A/AIOS-SUITE-BOX-A1 平移 [500, 0, 0]
  [ok] unchanged-elsewhere: 其余成员逐一吻合（≤ 1 mm，共 117 件）
判定：pass
```

### 三处 PML 陷阱（都是实测踩出来的，静态审计看不见）

**陷阱 1：无名元素的 `NAME` 属性不是空串，是 refno 字符串**

```
Q CE                      → BOX 1 of EQUIPMENT /AIOS-SUITE-EQ-NOZ
!cn = !!CE.NAME
Q VAR !cn                 → <STRING> '=24384/27392'
```

所以 `IF (!!CE.NAME EQ '')` **永远为假**，整段补名会被静默跳过——不报错、不崩溃，
只是名字没加上。判据必须用 `IF (NOT MATCHWILD(!cn,'/*'))`。

**陷阱 2：PML 不支持 `STRING + REAL`**

```
!nm = '/AIOS-SUITE-PIPE-P-BRAN-' + !i
→ (2,779) Method <STRING>.+(<REAL>) not found
→ 随后 NAME $!nm 报 (46,28) ERROR - Variable NM not defined
```

`!i` 是 `<REAL>`，拼不上。改用单引号内插：

```
!nm = '/AIOS-SUITE-PIPE-P-BRAN-$!i'     → <STRING> '/X-TEST-7'（实测成立）
```

注意 `'P1' + 'P2'` 是成立的（STRING + STRING），所以这个坑只在拼数字时出现。

**陷阱 3：`COLLECT ALL FOR CE` 会抓到隐含元素，`MEMBERS` 不会**

```
/AIOS-SUITE-BRAN-S
VAR !mb COLLECT ALL FOR CE   → 12 个，含 3 个 'IL TUB OF =xxx'
!mb = !!CE.MEMBERS           → 9 个，纯 DBREF
```

隐含元素（`TUBI` 型，如 `ileave tube of BEND ...`）不能命名。批量补名必须走 `MEMBERS`。

### 补充实测结论

| 语句 | 结论 |
|---|---|
| `NAME /xxx` 对 PAVE | ✅ 成立（`Type PAVE`、`Ref` 不变） |
| `NAME /xxx` 对 PLOO | ✅ 成立 |
| `NAME /xxx` 对 FTUB / BEND | ✅ 成立 |
| `NEW PAVE` 直接建在 PANE 下 | ❌ `Cannot create a PAVE at this level`，必须先 `NEW PLOO` |
| `DELETE PANE` | ⚠️ 会级联删掉其下 PLOO/PAVE，重建房间时要整段重来 |
| `DOWN` | ❌ `CP: Syntax error`，不是合法导航命令 |
| `!!STR(!i)` | ❌ `PML: Function not found` |

### 命名方案（`setup_8000` 里的落地形态）

| 层 | 名字 |
|---|---|
| 房间 | `/AIOS-SUITE-RM-R201-PLOO`、`-V1`..`-V4`（R202 同） |
| COPY 来的设备 BOX | `/AIOS-SUITE-EQ-NOZ-BOX` |
| COPY 来的 19 条 BRAN | `/AIOS-SUITE-PIPE-P-BRAN-1`..`-19` |
| 每条 BRAN 的管件 | `/AIOS-SUITE-PIPE-P-BRAN-<b>-P<m>` |
| COPY 来的 BRAN-S 成员 | `/AIOS-SUITE-BRAN-S-M1`..`-M9` |

## 7.9d calibrate 项订正（2026-10-28 实测）

`rvm-truth.json` 里原先标 ★ 的 calibrate 项，本轮用实测逐条判定。**三条里一条判错又回滚**。

### 订正 1：PANE 确实进 RVM（成立）

`room/pane_move_apply.mac` 原来登记 `calibrate: true` + 「PANE 是否出几何待首轮观测」。
实测 `PANE BY D 200` 前后：

```
before: 58000.003, 3000.000,    0.000, 62000.003, 7000.000, 2000.000
after : 58000.003, 3000.000, -200.000, 62000.003, 7000.000, 1800.000
```

U 上下界同时下移 **-200.000**，与登记的 `union_shift [0,0,-200]` 逐字吻合。
PANE 的 AABB 就是四个 PAVE 顶点的并集、高度取 PLOO 的 `HEIG 2000`。
→ `calibrate` 已去掉，`pane_move_restore` 同步去掉。

### 订正 2：PAVE 改顶点是变形不是平移（成立）

`room/pave_move_apply.mac` 原来只记 `count_delta: 0` + calibrate。
东边两顶点 `E 62000→64000` 后，PANE 并集应从 `[58000,3000,0,62000,7000,2000]`
变 `[58000,3000,0,64000,7000,2000]`。真值已改为显式 `union_aabb`。
→ `calibrate` 已去掉。

### 订正 3：NBOX 不进 RVM（成立，但原因和原先写的不一样）

原先写「负实体（NBOX）是否进窄口径 RVM 未验证 ★」。
实测：NBOX-N1 在库里**确实存在**（`Ref =24384/27584`、`Q XLEN 500 / YLEN 500 / ZLEN 2500`、
`POS 0,0,0`），但它**挂在 BOX-N 之下**，不出现在 `EQ-NBX` 的 `MEMBERS` 里
（`Q MEMB` 只有 `1 BOX 1 /AIOS-SUITE-BOX-N`），RVM 导出的 148 个成员里也完全没有它。

→ 结论不是「负实体被过滤」，而是「NBOX 是 BOX 的子件，不在设备级成员树上」。
所以 `geom/nbox_*` 两条腿在 RVM 里**天然不可见**，`union_shift` 恒 `[0,0,0]`、
`count_delta` 恒 `0`，只能靠 PML 侧 `Q XLEN` 断言。这两条保留 `calibrate`（因为确实无痕）。

### ★ 一次判错又回滚：BRAN HPOS 在 RVM 里不可见

我先把 `pipe/bran_hpos_apply.mac` 从 calibrate 改成 `moved: {BRAN-S-M1: [0,0,200]}`，
理由是「HPOS U 2900→3100，头段 M1 在 U 2900–2930，应该整体上移 200」。

实测 `bran_hpos_apply` 后再导快照，逐件算 AABB 差值：

```
/AIOS-SUITE-BRAN-S-M1   dUlo=0.00   dUhi=0.00
/AIOS-SUITE-BRAN-S-M2   dUlo=0.00   dUhi=0.00
...（M3..M9 同样全 0）
```

**9 个管件一个都没动。** HPOS 是分支的拓扑属性，改它不重算管件几何。

已回滚为 `calibrate: true` + `aabb_blind: true`，并写进 `limits`。
这条腿只能靠 PML 侧 `Q Hposition` 断言（2900→3100→2900）。

**教训**：AABB 差值必须实测，不能从「属性变了，几何该跟着变」推。
这已经是本轮第二次在 RVM 可见性上推错（上一次是 34 个缺省名）。

## 7.9e 支管成员换序归入 (c) 类（2026-10-28 实测结案）

`connect/bran_reorder` 长期挂在待办上，原因是 `REORDER` 语法在仓内零命中。
本轮把候选语法逐个实测，全部不成立：

| 语句 | 结果 |
|---|---|
| `REORDER AFTER FTUB 3` | ❌ `CP: Syntax error` |
| `MOVE FTUB 1 BEFORE FTUB 2` | ❌ `CP: Syntax error` |
| `ORDER FTUB 1 AFTER FTUB 2` | ❌ `CP: Syntax error` |
| `SEQUENCE 1 2 3` | ❌ `CP: Syntax error` |
| `UNGROUP` | ❌ `CP: Syntax error` |

还试了唯一有希望的路子——**删成员看序是否重排**：

```
删 /AIOS-SUITE-BRAN-S-M3（原 FTUB 2）前：
 1 FTUB 1 /AIOS-SUITE-BRAN-S-M1
 2 BEND 1 /AIOS-SUITE-BRAN-S-M2
 3 FTUB 2 /AIOS-SUITE-BRAN-S-M3
 4 FTUB 3 /AIOS-SUITE-BRAN-S-M4
 5 FTUB 4 /AIOS-SUITE-BRAN-S-M5
 6 BEND 2 /AIOS-SUITE-BRAN-S-M6
 7 FTUB 5 /AIOS-SUITE-BRAN-S-M7
 8 FTUB 6 /AIOS-SUITE-BRAN-S-M8
 9 FTUB 7 /AIOS-SUITE-BRAN-S-M9

删 M3 后：
 1 FTUB 1 /AIOS-SUITE-BRAN-S-M1
 2 BEND 1 /AIOS-SUITE-BRAN-S-M2
 3 FTUB 2 /AIOS-SUITE-BRAN-S-M4     ← M4 顶上来，序号重编
 4 FTUB 3 /AIOS-SUITE-BRAN-S-M5
 5 BEND 2 /AIOS-SUITE-BRAN-S-M6
 6 FTUB 4 /AIOS-SUITE-BRAN-S-M7
 7 FTUB 5 /AIOS-SUITE-BRAN-S-M8
 8 FTUB 6 /AIOS-SUITE-BRAN-S-M9
```

**剩余成员的相对顺序完全不变**，只是 E3D 把序号重编了一遍。即：

- 序号（`FTUB 2`、`BEND 1`）是「当前位置」的显示名，不是可写属性；
- 成员序由拓扑连接（`Arrive` / `Leave`）决定，PML 层没有改写入口。

→ 「支管成员换序」在 PML 里触发不了，该用例从覆盖矩阵**移到 (c) 类**。
`connect/bran_reorder_probe.mac` 保留作回归证据，结论已写进文件头，换 E3D 版本可重跑一次复核。

## 7.9f 从 PML 侧生成 ATT，身份解析率 1/147 → 147/1（2026-10-28）

### 问题

窄口径 RVM 只写几何，不写元素身份。`rvm_verify import` 不带 ATT 时，
148 个成员里只有导出根那 1 个是 `cli_root`，其余 147 个是 `stable_hash` 未解析。

这不影响按名字比对（Compare 用相对路径当键），但会：
- 让 `resolved/unresolved` 元数据永远是 1/147，掩盖真实覆盖率；
- 让任何按 refno 的交叉引用（如与 PML 侧 `Q REF` 对账）无法进行。

### 官方路径为何不可用

官方是 `GenModelRvmExport` CAF addin 的 pair 模式，靠环境变量
`GENMODEL_ATT_OUT` 驱动，产出 `.rvm` + `.att`。本机两个障碍：

- `launch_e3d_sample_repaired.ps1:100,119` 硬编码
  `C:\Program Files (x86)\AVEVA\Everything3D3.1\Startup.dll`，该路径**已不存在**
  （两个影子安装 `shadow_e3d31_aps_all` / `shadow_e3d31_gen_model_test` 里都有这个 dll，
  但启动器不读它们）；
- `run_ams_c_entrymacro.bat` **根本不加载 addin**（全文无 `addin` / `DesignAddins`）。

注意 `GenModelRvmExport.dll` 本身是存在的，`DesignAddins.xml` 里也注册了，
`DesignAddins.xml:31` 有 `<string>GenModelRvmExport</string>`。卡的是启动链路，不是 dll。

### 解法：`New-SuiteAttDump.ps1`

从 PML 侧把同样的信息 dump 出来自己拼 ATT。ATT 格式取自
`vendor/../src/rvm_baseline/att.rs:34-35`（分隔符常量）与 `:73-113`（merge 解析器）：

```
AVEVA_Attributes_File v1.0 , start: NEW , end: END , name_end: := , sep: &end&
NEW Header Information
Element:= /AIOS-SUITE-8000
END
NEW /AIOS-SUITE-8000-Z
NAME:= =24384/27738
TYPE:= ZONE
END
```

三个约束：

1. **键值分隔符是 `:=`，不是 `=`**（`att.rs:35` 的 `KEY_VALUE_SEPARATOR`）。
   写成 `NAME = =ref` 会被 `split_once(":=")` 判为无键值对而**整行丢弃**——
   section 建起来了（`identity_source` 变成 `att_name`）、但 `NAME` 取不到，
   `resolved` 仍是 `false`。第一版就踩了这个坑，现象极隐蔽。
2. **section key 必须与 RVM 组名逐字相同**（`import.rs:199` 的 `self.att.get(&name)`）。
3. **`NAME` 写 `=ref0/ref1`**（`import.rs:211` 的 `refno_from_att_name`，
   `att.rs:124-134` 要求两段都非 0），命中即 `identity_source = "att_direct"`、`resolved = true`。

### 结果

```
身份 已解析/未解析: 147 / 1
```

147 个成员全部 `att_direct` 并带真实 refno：

```
/AIOS-SUITE-8000      refno=24384/27737  src=cli_root
/AIOS-SUITE-8000-Z    refno=24384/27738  src=att_direct
/AIOS-SUITE-EQ-RM     refno=24384/27895  src=att_direct
/AIOS-SUITE-BOX-RM    refno=24384/27896  src=att_direct
...
```

剩 1 个未解析是隐含 TUBI（`TUBE 1 of BRANCH /AIOS-SUITE-PIPE-P-BRAN-19`），
PML 的 `COLLECT` 不遍历它——这是 PML 侧的边界，不是 ATT 的缺陷。

已接入 `Export-SuiteRvmTruth.ps1` 的 `Export-State`：每次导出自动 dump ATT 并带 `--att`；
dump 失败只告警不中断（ATT 只影响身份解析率，不影响比对）。
`Import-RvmSnapshot` 新增 `-AttFile`，不传时命令行与以前逐字相同。

## 7.9g BOX-A1 改长方体，三条旋转腿从 aabb_blind 变为真判（2026-10-28）

### 问题

`BOX-A1` 原是 `1000×1000×1000` 正方体且居设备原点，绕自身中心转 90° 后
AABB 完全不变，`xform/rot_u|e|n` 三条腿在 RVM 里看不出任何差异。

### 改造

基线改成 `2000×1000×500` 长方体。三条腿现在都能真判：

| 腿 | 旋转 | 尺寸变化 | 实测 AABB | 判定 |
|---|---|---|---|---|
| `rot_u` | 绕 U 90° | X/Y 互换 → 1000×2000×500 | `[39500,4000,-250,40500,6000,250]` | pass |
| `rot_e` | 绕 E 90° | Y/Z 互换 → 2000×500×1000 | `[39000,4750,-500,41000,5250,500]` | pass |
| `rot_n` | 绕 N 90° | X/Z 互换 → 500×1000×2000 | `[39750,4500,-1000,40250,5500,1000]` | pass |

`geom/resize` 腿同步验证（1600×1200×800）仍 pass。

### 连带同步五处尺寸字面值

不改成一致，「终态与基线按名相等」的前提就不成立：

- `common/setup_8000.mac`
- `geom/resize_restore.mac`（写回 2000×1000×500）
- `delete/box_restore.mac`
- `delete/child_then_parent_restore.mac`
- `delete/recreate_same_name_apply.mac`

### ★ 两处期望值第一次算错

- **rot_e**：我把 N 区间算成 `5000±500`，实际是 `5000±250`。
  Y/Z 互换后 N 方向是**原来的 Z 跨度 500**，不是 Y 跨度 1000。
- **rot_n**：我把 U 区间算成 `0±750`，实际是 `0±1000`。
  X/Z 互换后 U 方向是**原来的 X 跨度 2000 的一半**。

两次都是「尺寸互换就推区间」的错。教训与 7.9d 一致：
AABB 期望值必须按「哪个轴换了哪个轴、中心在哪」逐个算。

### 仍不可见

`BOX-N`（2000³）与 `BOX-RM`（500³）仍是正方体，旋转在 AABB 上不可见。
它们的用例（`geom/nbox_*`、`room/member_move`）不涉及旋转，未改。

## 7.10 复现命令

```powershell
# 1. 建隔离副本（一次性，4.15 GB）
pwsh -NoProfile -File .scratch\make-isolated-copy.ps1

# 2. 跑 setup（必须先跑，否则后续用例找不到基线元素）
python .scratch\tty_batch.py setup

# 3. 按组跑用例
python .scratch\tty_batch.py A07     # attr
python .scratch\tty_batch.py C01     # xform
python .scratch\tty_batch.py B01     # geom
python .scratch\tty_batch.py G01     # room
python .scratch\tty_batch.py A01     # delete/connect/pipe
python .scratch\tty_batch.py A02     # create

# 4. 强制链必须四腿连着跑
python .scratch\tty_batch.py A01-bran-copy
python .scratch\tty_batch.py A01-bran-reparent
```

`tty_batch.py` 的 `render()` 负责把 `{{WORLD_ANCHOR}}` 展开成
`/1RX03-EQUI` + `OWNER` 两行，`{{LOG_PATH}}` 展开成证据目录里的 `.log`。



## 7.11 三项历史待决的归属（2026-10-28 核清）

这三项长期挂在本套件待办里，但它们**都不属于 PML 测试套件的改动范围**，属于代码审计域。
核清后从本套件待办移除，避免下一轮继续误挂。

| 项 | 真实归属 | 现状 | 本套件要不要动 |
|---|---|---|---|
| T-D-6 越级 | 代码修复：`src/fast_model/model_initial_persist.rs`、`src/data_interface/manual_update.rs`、`src/data_interface/dbnum_initialize.rs` | 已有完整盘点 `t-d-6-initialize-vs-legacy-pointer-rows.md`（97 行），结论「红」，给了三个修复方案 | **不动**。PML 侧没有能造出「`pe` 为 0 但模型面还有 legacy 行」这个状态的手段，测不出来 |
| G1 两处文档矛盾 | 文档：`docs/specs/incr-gen-audit/` 下的门定义 | 记录里的「`task_plan.md:208` vs `:323`」已过时——该文件不存在，实际是 `plan.md`（222 行）、`spec.md`（355 行）、`tasks.md`（137 行）、`review.md`（162 行） | **不动**。门定义是 spec 的事，与本套件无关 |
| 是否派 CT 做只读范围门报告 | 审计：`src/data_interface/update_scope.rs:266` 的 `admits` | 未做 | **不做**。与本套件无关；本轮已用 CT 做过一次交叉验证（7.9b），价值在那处已兑现 |

### 为什么核清这三项

原先的记录里出现过 `docs/specs/incr-gen-audit/task_plan.md:208` 与 `:323` 这样的引用，
但仓库里**没有 `task_plan.md`**。这类过时引用会让下一轮花时间找一个不存在的文件。
核清一次，把真实文件名与行数写下来，后续直接查表。

`t-d-6-initialize-vs-legacy-pointer-rows.md` 本身质量很高（代码路径、夹具草案、修复方案、
未确认项都齐），不需要重做；它只是不该挂在 PML 套件的待办里。

## 7.12 本套件的收口状态（2026-10-28）

| 维度 | 状态 |
|---|---|
| 正式用例 | **35 个**（34 + `E04-spre-swap`），全部可跑，无一阻塞 |
| 宏文件 | **77 个** `.mac`（75 + `pipe/spre_swap_apply/restore`） |
| RVM 成员缺省名 | **0 / 148** |
| 身份解析率 | **147 / 148**（剩 1 个是 PML `COLLECT` 不遍历的隐含 TUBI） |
| 已实测 pass 的 RVM 腿 | `xform/move`、`xform/rot_u/e/n`、`geom/resize`、`room/pane_move`、`pipe/spre_swap` |
| aabb_blind 但已有 PML 侧断言的腿 | `pipe/bran_hpos`（`Q Hposition`）、`geom/nbox_*`（`Q XLEN`）、`connect/nozz_move`（无 CATREF） |
| 归 (c) 类（PML 触发不了） | `connect/bran_reorder`（成员序不可写） |
| 仍未验证 | db7998 extent 2 为何能在单文件库写成功（7.6.3） |
