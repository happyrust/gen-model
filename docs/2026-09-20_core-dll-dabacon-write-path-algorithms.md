# 2026-09-20 core.dll（DABACON）把数据库数据写到文件的算法清单 —— ida-bridge 取证

- 问题（用户原话）：「结合 ida-bridge 分析 core.dll 是如何写数据库数据到文件的，整理成一个算法清单。」
- 手段：ida-bridge 活桥，`core.dll` = `idalib-33060`（`D:\ida_scratch\plant3\core.dll.i64`，E3D 3.1，34 308 个函数，MSVC 符号 + FORTRAN 例程名）。
  本文所有地址、常量、字符串均本轮实读（`decompile()` / `xrefs` / `string_refs` / `ida_bytes.get_dword`），
  未读到的地方明确标「推断」或「未展开」。名字常量用 `ENHAS2` 逆函数解回词（`531441 + Σ code(cᵢ)·27ⁱ`）。
- 承接（只引用，不重复取证）：`vendor/e3d-io`（读侧：页 0 描述符字表、会话页、`(5,13387743)` / `(5,7618377)` 两棵 B+ 树、类型 7 元素记录页）、
  `上下文/会话-2026-09-04-模型联动更新梳理度核对-3263d0f3.md` §三之九（`sub_5AF6840` 的根偏移）。
- 第一轮纯只读：未改 IDB，未改生产代码，未跑构建。第二轮（同日，补 A3a/A3b/A8、复核 §5 位移）只改了 `vendor/e3d-io` 里的**注释**（见 §5），未改代码与测试；IDB 仍未改。

---

## 0. 一句话总览

core.dll 的数据库写入是**追加式影子分页（shadow paging）+ 单点提交**：

1. 会话进行中，任何修改都**不改盘上旧页**——元素记录被整条重写到「当前输出数据页」（类型 7，bump 分配），
   索引（`INDEX` B+ 树）沿路径做 copy-on-write，改出来的页都是 **ext=0 的临时页**，只住内存（页帧不够时才溢出到临时文件）。
2. `SAVEWORK` 时开一个「刷盘上下文」，把 `old_end+1` 起的连续页号**顺序**分给所有临时页（记录先搬、树页自底向上重编号、根写回控制块），
   新建一张**会话页**（类型 3：sesno、上一会话页、末页、时间戳、用户、注释、全部表根），把这些**新页顺序追加**写到文件尾；
   3.1 的普通写（`DOPENW`）在这之前还多一步**三方合并**（A3a）：把「基线会话 → 工作会话」的差异套到「文件最新会话」的克隆上，再落盘——
   别人在你打开之后存过的会话不会被覆盖；有冲突整次存盘失败（576），一页都不写。
3. 最后**只改写页 0 的第 10/11 字**（`LSPGNO` → 新会话页）。页 0 落盘即提交；崩溃在此之前，文件从页 0 看仍是上一会话。
4. 文件层用 `WriteFile` 按 1-based 块号定位（`(blk-1) × 页字节`），**大端**落盘（x86 上逐字 `bswap`），
   独占/共享由 `LockFileEx` 锁 `0x7FFFFFF6FFFFFFFF` 处 1 字节的「哨兵字节锁」实现，离开写模式时 `FlushFileBuffers`。

---

## 1. 分层、命名与入口

### 1.1 五层 trace 前缀（错误文本第一段）

| 前缀 | 层 | 例 |
|---|---|---|
| `1xx:` | DB1 页缓存层（页帧、锁、脏位、读写页） | `1GP` get_page、`1WP` write_page、`1UP` db1_update_page、`1ULP` unlock_page、`1WDBP` write_db_page、`1SM` switch mode |
| `2xx:` | DB2 文件/页 0/页 1 层 | `2RP` read page、`2WP` write page、`2CSP` check session pages、`2GNE` get new (page-1) entry、`2FUB` find unused bucket、`2UP1UOR` update page 1 user-or-… |
| `3xx:` | DB3 表页（B+ 树，带 level）层 | `3CTE` change table entry、`3SN` split node、`3GNTE` get next table entry、`3STS` start table search |
| `4xx:` | DB4 元素层 | `4SCEA` set current element attribute、`4GCEA` get…、`4CEUOK` current element update ok、`4SCEFER` set … ext ref |
| `5xx:` | DB5 会话层 | `5SW` save work、`5WCEP` write claimed element pages、`5WCTP` write claimed table pages、`5C` compare |

### 1.2 C API 包装与「例程编号表」

所有 `db_*` / `dbx_*` 外部 API 都是薄包装：`db_write_page(tok,pg,buf)` → `sub_5ADCDD0(290, …)`，随后 `return dword_6453B98 == 0`（全局错误码）。
第一参是**例程编号**，`off_6003900[编号]` 是名字表（奇数下标 = FORTRAN 6 字名，偶数 = C 名，本轮整表读出 663 项）。与写路径相关的：

| 编号 | FORTRAN | C 名 | 包装 → 实现 |
|---|---|---|---|
| 197/198 | `DSVWRK` | `db_save_work` | `0x5AAF340` → `sub_5AD9880` → **`sub_5AE68D0`**（5SW 主体） |
| 459/460 | `DPSWRK` | `db_partial_save_work` | `sub_5AAEB10` → `sub_5AD0ED0` |
| 465/466 | `DPSWAB` | `db_partial_save_work_all_but` | `sub_5AAEB40` → `sub_5AD11D0` |
| 417/418, 603/604 | `DXSWRK` / `DXSWK2` | `dbx_save_work_extra` / `…_incl_table_changes` | 未展开 |
| 289/290 | `DWRTPG` | `db_write_page` | `0x5AAF6C0` → `sub_5ADCDD0` → **`sub_5AFD5A0`** |
| 287/288 | `DRDPG` | `db_read_page` | `0x5AAEED0` → `sub_5AD5EE0` |
| 405/406 | `DWRTP0` | `db_write_page0` | 未展开 |
| 333/334 | `DADSP0` | `db_add_session_to_page0` | 未展开 |
| 331/332 | `DCHKSP` | `db_check_session_pages` | 未展开（实现体应即 `sub_5AF3000`「2CSP」） |
| 291/292 | `DUPSES` | `db_check_and_update_session` | `0x5AAAB50` → `sub_5ABB660` |
| 357/358, 359/360 | `DADBKT` / `DGTBKT` | `db_add_bucket` / `db_get_bucket` | `sub_5ABAD10` / `sub_5AC3810` |
| 381/382 | `DUPDBL` | `db_update_bucket_list` | （C++ `DB_DB::updateBucketList` 0x59144E0 调用） |
| 295–298, 379/380, 391–402 | `DFLSH` `DFLDB` `DFLAB` `DEXFL` `DEOFL` … | `db_flush*`（extract 间刷写） | 见 §3.12 |
| 567/568, 407/408 | `DREGFF` / `DUNDFF` | `db_register_failed_flush` / `db_undo_failed_flush` | `sub_5AD6FB0` / `sub_5ADCA10` |
| 169/170 | `DQMOD` | `db_db_modified` | `sub_5AAD850` → `sub_5ABDC40` |
| 137/138, 253/254, 133/134 | `DOPENW` / `DOPENS` / `DOPENR` | `db_open_write_db` / `db_open_shared_db` / `db_open_read_db` | → `sub_5AF9EF0(…, openmode)`，openmode 1 = write、2 = shared、0 = read |
| 167/168, 335/336 | `DQLEFT` / `DINCIP` | `db_get_incore_pages_left` / `db_increase_incore_pages` | 页帧池容量 |

### 1.3 C++ 门面（`DB_DB` / `DB_MDB`）

```
DB_MDB::saveWork(comment, moduleEnding)  0x59F8CE0
  ├ db_get_reference(535968) 记住 CE
  ├ DB_DB::saveWork(dbs, msg, comment)   0x5911420 → 0x59114B0（静态，遍历写模式 DB，DBSaveWorkAllowed 0x58F2AB0 门）
  │    └ 每库 DB_DB::dosaveWork(comment)  0x58FF0C0
  │         ├ LicFeatureCheckOut_StrEq(… "WRITE" / "SHARE")   许可门
  │         ├ db_db_modified(dbno)（DQMOD）或 this+116==2 → 否则 " has not been changed"
  │         ├ this+128 != 3 → DB_DB::checkUserStillValid
  │         ├ this+116==1 且 DB_TemporaryObject::getTemporaryElements 非空
  │         │     → db_partial_save_work_all_but(dbno, comment, len, 1, tempElems, n)   （临时元素不落盘）
  │         │   否则 → db_save_work(dbno, comment, len, 1, 0)                              （DSVWRK）
  │         ├ 失败：MR_Message::Reset(43,360) + addErrors
  │         └ 成功：DB_DBPlugger::ClearCaches；DB_DB::specialCaseSavework 0x5913A20
  │               → DB_RawChanges::createListBetweenSessions(oldSes → newSes)（增量差分表，供联动更新）
  ├ db_go_to_element(CE) 回到原 CE（失败 GOWORL）
  └ 写时间戳
DB_DB::internalSaveWork(elems, comment) 0x59094F0 → db_partial_save_work（DPSWRK，只存给定元素）
```

---

## 2. 盘上格式要点（写侧视角，只列写路径用到的字）

- **页大小**：页 0 第 13 字 `PGSIZE`（4 字节字数，样本 512 字 = 2048 B）；全局 `dword_6453DC4` = 页字数；块 I/O 按 **1-based 块号**：`offset = (blk-1) × recl × 4`（`sub_5B9B9D0`）。DB1 层页号 0-based，交给 DB2 层时统一 `+1`。
- **字节序**：文件大端。`sub_5B9B9D0` 在 `DB_MDBSetUp::subtype(entry)==1 && a5` 时先经 `sub_5B9C270`（SSE2 逐 32 位 `bswap`）再 `WriteFile`；读侧对称。
- **页类型（第 0 字）**：0 描述符（页 0）；1 文件信息/桶页（页 1，`2GNE`/`2FUB`/`2COUBE` 操作它，multi-user 桶表）；3 会话页；5 表页（B+ 树结点，第 1 字 = 表名 hash，带 level）；7 元素记录页。`DWRTPG` 拒绝 >7（"Unknown page type is %d"）。
- **表名 hash**（本轮 `ENHAS2` 逆解）：`13387743 = "INDEX"`（refno → 记录位置 B+ 树）、`7618377 = "CLAIM"`（e3d-io 读侧命名一致）、`276202698 = "OWNSES"`（会话页第 4 字来源）。
- **INDEX 叶值打包**（`sub_5AE9960` / `sub_5AE9DF0` 实读）：两字 `(pageno, packed)`，`ext = packed & 0x1FFF`（记录所在页的 extent 号，13 位），`word_offset = (packed >> 13) & 0xFFF`（页内偏移，**4 字节字**，12 位）；写侧 `packed = ext | (word_offset << 13)`。与 e3d-io `>>12` 读法的关系见 §5。
- **页 0（写路径改的字）**：第 2 字 `DBNO`；第 9 字 `TMPLVN`（首次存盘后取负）；**第 10/11 字 `LSPGNO`（最新会话页 pageno/ext）= 提交指针**；第 14/15 字 = 属性 7（两字，写模式相关）；第 16/17.. `CRINFO`；第 94/96 字存盘时清零。
- **会话页（类型 3，`sub_5AE38B0` 逐字写）**：

| 字 | 内容 | 来源 |
|---|---|---|
| 0 | 3 | 页类型 |
| 1–2 | 上一会话页 `(pageno, ext)` | 旧页 0 第 10/11 字 → 会话链 |
| 3 | sesno | 显式给定，否则 DB 属性 1（当前会话号） |
| 4 | `OWNSES` 值 | `sub_5AF8BF0(dbno, ext, sesno, 276202698)` |
| 5–6 | 末页 `(end_pageno, ext)` | `sub_5AF0C10()` = 本次刷盘最后分到的页号；属性 0 |
| 7–8 | INDEX 根 | 属性 3（两字，cb+12/+16；推断） |
| 9–10 | CLAIM 根 | 属性 5（两字，cb+20/+24；e3d-io 读侧 0x24 = claim 根，一致） |
| 11 | 会话类型标志 | 调用方 a12（普通 SAVEWORK 传 1） |
| 12–16 | 时间戳 A：4, 年(+1900), 月, 日×24+时, 分×60+秒 | `sub_5B8F4F0` + `sub_5B8F5E0` |
| 25–29 | 时间戳 B（同格式）或调用方给定 | `sub_5B8F560` |
| 30, 31.. | 用户名长度 + 打包字符 | `aImdatsinotar` 全局（运行时填用户名）/`dword_6453DEC` |
| 40, 41.. | 注释长度 + 打包字符 | 调用方注释；属性 18≠1 时先经字符集转换（`sub_5471FC0`/`sub_54B2CE8`/`sub_5471E50`） |
| 20–24 | SessionPackage 附加 blob + 长度 | 仅 package 路（`sub_5B1BE20`/`sub_5B1BE10`） |
| 199, 200.. | 表目录条数 + 三元组 `(表名 hash, 根 pageno, 根 ext)`（用户表… 然后 CLAIM、INDEX） | `sub_5AF7870` 枚举 |
| 503–504 | 8 字节程序/模块标识 | `sub_54719F0(&byte_6453B8F, 8, …)` |
| 505 | `*dword_6453DF0` = 当前写者的用户/桶号（页 1 `2UP1UOR` 与 CLAIM 值第 1 字用同一变量，见 A3a.6） | |
| 506–510 | a15 / a18 / 上一 sesno / a13 / a17 | |

---

## 3. 算法清单

### A1. 内存页缓存（DB1）

- 帧表 `dword_6A540EC`，每帧 28 字节 7 字：`[0]` 页缓冲指针、`[1]/[2]` 哈希链 next/prev、`[3]` pageno、`[4]` extno、`[5]` dbno、`[6]` 标志：低 14 位 = 锁计数、`0x4000` = 脏（modified）、`0x8000` = 最近引用（clock 第二次机会）、`0x10000` = 已触碰。帧 1 的 `[1]` 为空闲链头；帧号 2..N+1 参与替换；池大小 `dword_6A540FC`（`DINCIP` 可增）。
- 查找 `sub_5AF04A0(pageid, dbno)`：桶 = `(97·dbno + |pageno|) % dword_6A540E4`，链上比对 `(pageno, ext, dbno)`。插入 `sub_5AF03F0`，摘除 `sub_5AF0520`。
- 取页 `sub_5AEE4E0(dbno, &pageid, lock, _, &buf)`（`1GP`）：命中 → 锁计数 +1；未命中 → `sub_5AEF150` 取帧 → 填 `(dbno,pageno,ext)`、清脏、置 0x10000 → 找文件 token（`sub_5AEF2F0(dbno,ext)`：按 dbno 的红黑树 → ext 匹配）→ `sub_5AF0640(token, pageno+1, buf, 页字数×n)` 读盘；支持 **预读 n 页**（`DB_N_DBPAGES_TO_READ`，`dword_6A5415C`，读到暂存区 `dword_6A54158` 再分发到各帧）和**本地文件缓存**（`sub_5BCC210` 读 / `sub_5BCC240` 写；类型 >5 或 `(5, 非 CLAIM)` 的页才缓存）。ext=0 的页从临时文件 token `dword_6A540E8` 读。
- 取帧 `sub_5AEF150`（clock 替换）：指针 `dword_6A540E0` 循环扫；跳过锁住的帧；`0x8000` 置位者清位放过；**脏帧只有在「是临时页（ext=0）」或「正处于刷盘上下文」时才允许被驱逐**（先 `sub_5AF1C30(pfno, 0)` 写出）；扫两圈仍无 → 错 400（页帧耗尽 — 会话内改动多且不 SAVEWORK 的典型报错）。
- 置脏 `sub_5AF0F30(pfno)`；解锁 `sub_5AF11D0(pfno)`（"1ULP: Unlocking a page that isn't locked" → 656）。

### A2. 会话内修改：记录追加 + 索引 CoW，全部落在临时页

1. **元素记录不原地改**。`4SCEA` 设属性（`sub_5AB7B90`）→ `sub_5AECBE0`：先 `4CEUOK`（`sub_5AE8E40`：属性 10 = 写权限门、属性 12 = 打开模式、claim 检查经 `sub_5B01340(…, 7618377 CLAIM …)`），然后
   `sub_5AE9DF0(len, len, lock, &newloc, …)` 在**当前输出数据页**里 bump 分配记录空间（全局：`dword_6A540C0` 当前页帧、`dword_6A540B4` 已用字数、`dword_6A540B8/BC` 页 id；不够放 → 解锁旧页，`sub_5AEE230` 新开一页并写第 0 字 = 7），把旧记录头 11 字 + 各属性按模板（`sub_5AF6AB0` 取 noun 模板，类型 2/6 real→double 展宽等）拷到新位置，改写目标属性，hunk（数组/字符串块）经 `sub_5AEA500` 读、`sub_5AEC110` 重写；
   最后 `3CTE`（`sub_5AFDF10`）把 INDEX 中该 refno 的叶值改成新 `(pageno, packed)`。
2. **索引页 CoW**：`3CTE` 对路径上每页调 `sub_5AF1600`「db1_update_page」（`1UP`）：
   - 无刷盘上下文（会话进行中）且页未脏：摘哈希 → 改页 id 为 **`(dword_6A54140++, ext=0, dbno=0)` 临时页号** → 重挂哈希 → 置脏，返回 `moved=1` + 新 id → 父结点指针随之改写（父也如此递归到根，根变了 `sub_5AFC950` 写回控制块）；
   - 已是脏临时页且需保留旧版本：另取一帧复制内容，两帧交换 id（"should be dealing with 2 temp pages" 一致性检查 655）——供 `DSTMRK`/`DUNDO` 会话内撤销用；
   - 有刷盘上下文且页属当前 DB/ext 且 `pageno ≥ dword_6A54108`（本次新分配区）→ 就地置脏不搬。
   结点满时 `3SN`（`sub_5B01BF0`）`sub_5AEE230` 新页分裂。
3. **新页分配** `sub_5AEE230(lock, &id, &buf)`：取帧、清零，刷盘上下文中 → `id = (dword_6A54114++, dword_6A54118)` 归当前 dbno；否则 → 临时页 `(dword_6A54140++, 0)`；置脏。
4. **临时页溢出**：帧不够时脏临时页可被驱逐 → `sub_5AF1C30` 的 ext==0 分支写到临时文件 `dword_6A540E8`（按页号顺序补写中间「dummy」页保证连续，`temp_file_size` = `dword_6A54144`）。
5. 修改期间**盘上任何旧页都不被写**——写只发生在 A7 的刷盘上下文里。

### A3. SAVEWORK 主体 `sub_5AE68D0`（5SW）

入参 `(dbno|0=当前|-1=全部, sesno显式|0, comment, len, flag=1, elems, n)`。

1. 前置：`sub_5AFCFD0(dbno)` 非法态 → 530；`sub_5AEA070`/`sub_5AE9F90` 进入（失败即 `sub_5AEB000` 退出）。
2. 用 `sub_5AF77B0(2, &dbno, &ext, …)` 枚举**写模式打开的库**，读属性 12（打开模式），列入 `dword_6453F00[]`/`dword_6553F00[]`；空 → "5SW:No dbs open in write mode to save"（41）。多于一库或非 shared 时先 `sub_5AFBF60()` + `sub_5AFB8C0()`（claim/一致性预检）。
3. 逐库按模式分派：
   - **模式 2（`DOPENS` shared / multiwrite）且属性 10≠2**：不建新会话——取页 0（锁 1）→ 取其第 10/11 字指向的最新会话页 → 就地刷新：第 5 字 = 末页、第 7 字 = INDEX 根、第 9 字 = CLAIM 根、第 199+ 表目录重写、页 0 第 14/15 字 = 属性 7 → `sub_5AF2520(0)` 刷脏页 → 解锁。
   - **模式 1（`DOPENW` 普通写，3.1 主路）**：`sub_5B1BEE0(dbno,ext)`（该库是否走「中央服务」）选 `SessionPackageCentral`(0x94 B, `sub_5B1C2F0`) / `SessionPackageNative`(0x34 B, `sub_5B1BFE0`) → `pkg->begin()`（A8；失败 704，仍调 `commit()` 收尾）→ 属性 7 >0 时 `sub_5AFD040`（`2UP1UOR` 更新页 1 用户/桶条目）→ `sub_5B12760` → 校验页 0 → 会话页类型 3（否则 664）→ 属性 17（extract/claim 标志）非零时 `sub_5AE79F0` → **`sub_5B113E0(dbno, sesno, comment, len, a5, 0, a6, pkg)`**（多写合并 + 提交，见 A3a）→ `pkg->getSesno()` 记入 `(dbno,ext)` 红黑树（`sub_5AE8560`/`sub_5AE0000`/`sub_5AE0190`）→ `pkg->commit()`（失败 705）。
   - **其它模式（旧直写路）**：`sub_5AE4DD0(dbno,&changed)` 无改动且未指定 sesno → 跳过；否则 `sub_5AF0FB0(1,…)`（`FHSWIT "QXMO"` 独占改写模式）→ 取页 0 与最新会话页 → 校验 "Sesno pointed to by page0 is %d, but sesno being created is %d"（665）→ **`sub_5AF0E70(dbno, ext, end+1)` 开刷盘上下文** → A5 重编号 → `sub_5AF8BF0(…OWNSES…)` → **`sub_5AE38B0`** 建会话页并提交（A6/A7）→ `sub_5AFC760(dbno, 1, sesno+1)` 当前会话号 +1 → `sub_5AF1590(1)` 关上下文 → `sub_5AF0FB0(3,…)`（回 `MODIFY`）。
4. 收尾：`sub_5AF9EA0` / `sub_5AEFB10` / `sub_5AEB2F0`（统计与状态复位）。

### A3a. 多写合并 `sub_5B113E0`（package 路主体，537 行；`DOPENW` 普通写的 3.1 主路）

**为什么要合并**：`DOPENW` 打开的库不独占文件，别人可能在你加载之后又存了新会话。所以 3.1 不再像旧路那样把工作会话直接写成「文件最新会话 +1」，
而是先把「基线 → 工作」的差异套到「文件最新会话」的克隆上再落盘；有冲突整次存盘失败，一页都不写。

**会话槽（合并上下文全局 `dword_6A54234…6A5425C`；部分存盘 `sub_5AE5770` 复用同一组）**

| 槽 | (ext, sesno) 全局 | 内容 | 怎么来 |
|---|---|---|---|
| dbno | `dword_6A54244` → | 当前库 | |
| working | `(6A54248, 6A54234)` = (属性 0, 属性 1) | 内存中的工作会话（含全部未存改动） | package 路存盘后属性 1 = **−新会话号**（旧直写路里属性 1 是正的「将建会话号」） |
| base | `(6A5424C, 6A54238)` = (ext, −属性 1) | 工作副本所基于的盘上会话 | `dword_6453EC4 > 0` 时改用预置的 `(dword_6453EC0, dword_6453EC4)`（`sub_5AE4A90`/`sub_5AF3710` 设，未展开） |
| latest | `(6A54258, 6A54250)` | 文件最新会话：页 0 第 10/11 字 → 会话页第 3 字 | 与 base 相同（没人存过）就不单开视图 |
| new | `(6A54240, 6A5423C)` = (ext, latest + 1) | 本次要写的会话 | `sub_5AF3940(dbno, ext, latestSesno, 会话页号, 1, 0, 1, 0)`：以 latest 的会话页为模板克隆，a7=1 → sesno+1 |
| 模式 | `6A54254` | 1 = 全量（5SW）、2 = 只存列出元素（`DPSWRK`）、3 = 除列出外全存（`DPSWAB`） | |
| 开关 | `6A5425C` | 1 = 合并时复制记录 + 对 latest 去重（5SW 与部分存盘都置 1） | |
| 第 4 槽 | `6A54260/64/68` | 另一 extent 的会话（extract flush 路 `sub_5B17450`/`sub_5B18A20`/`sub_5B1AB20` 用） | 5SW 为 0 |

「db block」= 一个会话视图（表根快照，0x24 B）：`sub_5AF3940`（trace "Create db block for dbno %d, extno %d, sesno %d,orig flag %d"）从会话页抄 INDEX/CLAIM 根（第 7–10 字）与第 199/200+ 表目录，
每库最多 **5 个并存**（40 B 槽；满 → 550，重复开 → 540；a8 置位时还要求该 sesno 就是页 0 指向的最新会话，否则 556——`sub_5B113E0` 都传 a8=0）；`sub_5AF2A70`（"Change to db block"）切换当前视图（550/552）；`sub_5AF54B0`（"Delete db block"）释放。
合并全程就是在这几个视图之间来回切换，用同一套 3xx/4xx 例程读写。

**主流程（a6 = 0，普通 SAVEWORK；a6 = 1/2 是同函数的两条变体，未展开）**

0. `sub_5B14A80(&cm)` 造 claims manager：`sub_5B295B0()`（store id 与 transaction id 两段 16 字节都已配置）→ `DAB_CentralClaimsManager`（vtbl 0x5D8DCC8，4 B）否则 `DAB_NativeClaimsManager`（vtbl 0x5D8DC54，20 B）。
1. 开 base 视图（`sub_5AF8CA0` 沿会话链找该 sesno 的会话页）→ 读页 0 → LSPGNO → 最新会话页 `v36`；`newSesno = v36[3] + 1`；开 new 视图（克隆 latest）；切回 working；latest ≠ base 时再开 latest 视图。
   显式 sesno（a2）覆盖 newSesno 并 `sub_5AFC760(dbno, 1, a2)`。预置目标模式下先把 base 的全部非 CLAIM 表根登记进 new（`sub_5AFC950`，523 → `sub_5AFCB40` 新建）。
2. **冲突扫描 `sub_5B10410(0, 0, 0)`**（5C）。工具 = 3STC 表比较：`sub_5B029A0(dbno, rootA, rootB, flag)` 开（两棵 B+ 树各一条栈 `dword_6A541F8/FC`，"table compare stack2"）、
   `sub_5AFFCB0(&key, &klen, &valA, &lenA, &valB, &lenB, &st)` 按键序归并取下一条、`sub_5AFFA60` 收；`st` **1 = 两树都有（两边值都给回来，由调用方比值）、2 = 只在 B、3 = 只在 A、4 = 结束**（5 = 已结束再取）；`sub_5B02F00` 清比较栈 `dword_6A541F0`。
   - 先 `sub_5B07750`：**名字类用户表**（根页第 3 字 key 宽 = 0 的变长键表）base↔working 有差异的键，到 new 里查同名：我新起的名字在 latest 里已给了别的元素，或同一名字两边都改且指向不同 → **561**；
   - base(A) ↔ new(B)：只在 base（别人删了）且 working 里改过（`sub_5B1F890`：INDEX 值不同且属性表不同）→ **562**；
   - base(A) ↔ working(B)：只在 base（我删了）但 new 里对 base 改过 → **590**；元素 TYPE == `UNKNOW`（338811897）→ **648**；主元素（`PRMTYP`≠0）相关走 `sub_5B07F50`/`sub_5B081F0`/`sub_5B084D0`（换 owner 等，未展开）；
     部分存盘（模式 ≠1）里非主元素与其主元素（`sub_5B059C0(0)` 找 primary，失败 565）必须同进同出列表 → **582**。
   - 冲突经 `sub_5AE5E00(ref, code, extra)` 追加到 `dword_6453DBC`（16 B/条，计数 `dword_6453DB4`，`sub_5AE5770` 开头清为 −1）；**有任何一条 → 返回 576，存盘失败**，属性 1 回填 −base，什么都不写。
3. **元素合并 `sub_5B0C090(0, 0, 0)`**：base(A) ↔ working(B) 的 INDEX：
   - st 3（只在 base = 我删了）→ 切 new，`4GTE`（`sub_5AEB6B0`）到该元素 → `sub_5AB3620(0)` 删（与 `sub_5ABDE20` 同一入口；推断为删当前元素）；已不在（10）→ 略；
   - st 2（只在 working = 我新建）→ latest 里没有 → `sub_5B0EA00`：`sub_5AE9960` 把记录**整条复制**到当前输出页（`6A5425C` 置位时总是复制，否则直接共用 working 的记录位置）→ new 里 `3CTE(INDEX, ref, 2, 新位置, 2, 1, 1)` 插入 → 引用属性修正 `sub_5B1CD90`/`sub_5B0A110` → 挂进 owner 的成员表 `sub_5B1CC10`；
     latest 里已有同 refno（过了冲突检查）→ `sub_5B06C40`（比 LOCK/OWNREF/SOFTYP + 属性 + 成员表）→ 不同则 `sub_5B214D0`（整条记录替换 + 收尾修正，见 A3b）；
   - st 1（两边都有 = 我改过）→ new 里还在：`SOFTYP`（243798436）未变 → `sub_5B0FDC0`（LOCK → `sub_5B0ED40` 逐属性三方合并 → `sub_5B211C0` owner 变更 → `sub_5B224B0`/`sub_5B20CA0` 成员表；见 A3b）；变了 → `sub_5B214D0`；new 里已没有（别人删了，且我没改到触发 562）→ 丢弃；
   - 部分存盘用 `sub_5B10DD0` 按列表/模式过滤；`PRMLST`（297853135）变化走 `sub_5B222D0`。
   `3CTE`（`sub_5AFDF10`）第 7 参：1 = 插入（529 已存在可容忍）、2 = 改值、0 = 删（534 不存在可容忍）。
4. **用户表合并 `sub_5B0D080(0, 0, 0, a7)`**（除 INDEX/CLAIM 外的表，base↔working 根不同者，带值比较）：st 1 → latest 里已是 working 的值就跳过，否则 new 里 `sub_5AB91B0(table, …, 14, key, …, valB)` 改写；
   `a7`（5SW 第 6 参；推断 = `DXSWK2 dbx_save_work_incl_table_changes` 的开关）或模板类型 2 时：st 2 → `3CTE(table, key, valB, …, 0, 1)` 插、st 3 → `3CTE(…, 1, 1)` 删。
5. **INDEX 去重 `sub_5B07080()`**：latest(A) ↔ new(B)，两边都有且 `sub_5B06C40` 判定内容相同 → `3CTE(INDEX, key, 2, valA, 2, 2, 1)` 把 new 的叶值改回 **latest 在盘上的记录位置** → 这些记录不再写。
   （`6A5425C = 0` 时先对 working 做一遍；`6A54260` 时再对第 4 槽做一遍。）
6. **claim 修正 `sub_5B0C760(0)`**：latest(A) ↔ new(B)：st 2（我新建）且 `PRMTYP`（243803617）≠0 → cm+8 加 claim，值 `(*dword_6453DF0, newSesno)`（529 已有 → cm+48 改）；st 3（我删了）且 `PRMTYP`≠0 → cm+36 删；
   其它 → 找主元素 `sub_5B059C0(1)` → cm+44 读 claim（534 = 无）→ 元素对 latest 有变（`sub_5B06C40`）→ cm+48 把主元素 claim 第 2 字盖成 newSesno。→ **CLAIM 值 = 两字 (claim 者的用户/桶号 `*dword_6453DF0`, 最近改动它的会话号)**（部分存盘里第 1 字可为 −ext 或 0）。
7. `sub_5B02F00()`；属性 17 ≠ 0（claim 生效）时 **CLAIM 表搬运**：cm+24 从哨兵键 `0x80000001` 起逐个取 working 的 CLAIM 键 → cm+44 读值 `(w0, w1)` → `w1 ≠ 0` 时改成 `(*dword_6453DF0, newSesno)` → cm+48 写进 new（534 不存在 → cm+8 加）→ cm+48 回写 working；cm+24 报 6 = 走完。
   new 的 CLAIM 根不从 base 抄（`7618377` 在两处表根循环里都跳过），所以 new 起点是 latest 的 claim 表：别人的 claim 原样保留，我的再叠上去。
8. 与旧路相同的落盘尾巴（切到 new）：读页 0/最新会话页 → `sub_5AF6840(INDEX)` → `sub_5AF0E70(end+1)` → A5（5WCEP/5WCTP/`sub_5AFC950`，其余表同理）→
   `sub_5AE38B0(dbno, a2, 用户, 长度, 注释, len, 0,0,0,0, v32, a5, 0, **base sesno**, 0, 1, 0, 0, pkg)` 建会话页并提交（A6/A7）；失败 `sub_5AF1590(1)`。`v32` = latest 会话页第 4 字（或预置的 `dword_6453EC4`）。
   **会话页第 508 字 = base 会话号**（工作副本所基于的会话，不是 latest；e3d-io `SessionTail::base_sesno` 观察到「MERGE CHANGES 后是被折掉的号、回退后比相邻会话小」与此一致）。
9. 收尾：把 new 的全部非 CLAIM 表根登记回 working（`sub_5AFC950`，523 → `sub_5AFCB40`）→ 内存工作会话与刚写的会话一致；`sub_5AF1590(1)`；释放 latest/new 视图；切回 working；`sub_5AFC760(dbno, 1, −newSesno)`；释放 base；`sub_5AEA070`。

**claims manager vtbl**（Native = 直接操作本库 CLAIM B+ 树，每个槽先 `sub_5B14B90` 存当前元素、完了 `sub_5B14C80` 恢复；`err` = `DAB_ErrorMessage`，第 5 字是错误码）

| 槽 | Native（vtbl 0x5D8DC54） | 语义 | Central（vtbl 0x5D8DCC8，打 `Spectrum_ClaimsClient()` 服务） |
|---|---|---|---|
| +8 | `sub_5B242C0(dbno, vector<ref>, val[2], err)` | 逐个 `3CTE(CLAIM, ref, 2, val, 2, 1, 1)` 加 | `sub_5B25710` → 自身 +12（服务端加） |
| +24 | `sub_5B24B70(dbno, sesRef, &key, err)` | `sub_5B2A5B0` 开 CLAIM 表游标 + `sub_5B2A740` 取下一键；值不是两字 → 570 | `sub_5B27240`：+52 拉整表，`sub_5B25690` 二分找下一个；到头 err 6 |
| +36 | `sub_5B25030(dbno, vector<ref>, err)` | `3CTE(CLAIM, ref, 2, 0, 0, 0, 0)` 删 | `sub_5B284C0` → 自身 +32 |
| +44 | `sub_5B24600(dbno, ref, &val, err)` | `sub_5B01340(CLAIM, ref)` 查；长度 ≠2 → 532 | `sub_5B26730`：+52 拉整表线性找 |
| +48 | `sub_5B25160(dbno, ref, val, err)` | `3CTE(CLAIM, ref, 2, val, 2, 2, 1)` 改 | `sub_5B28550`：空操作返回 1（会话号由服务维护） |

同一 DLL 还导入 `Spectrum_AcquireBucket/ReleaseBucket/UpdateBucket`（页 1 桶表的服务版）与 `Spectrum_Transaction*`（A8）。

**部分存盘**：`DPSWRK`(459/460) → `sub_5AD0ED0`、`DPSWAB`(465/466) → `sub_5AD11D0`、`DPQUIT`(467/468 `db_partial_quit`) → `sub_5AD0CE0` 都进 `sub_5AE5770`：
同一组槽，`6A54254 = a6`（2/3），先 `FHSWIT "QXMO"` 直写（不走 SessionPackage；页服务库直接拒 699；属性 10 == 3 拒 639；非写模式 627），
列表 `a7/a8` 上每个元素先验 `PRMTYP`（非主元素 → 573），驱动换成 `sub_5B15E10(…)`（未展开；它调 `sub_5B0D7E0(mode, list, n, a4)` = A3a.3–7 的打包版），完了 `FHSWIT "READ"`。

### A3b. 元素级 → 属性级三方合并（A3a.3 的展开）

**「我改过」且 SOFTYP 未变的元素走 `sub_5B0FDC0(ref, list, n)`**：
1. LOCK（750558）：base ≠ working（我改了锁；第 4 槽时 base ≠ latest 则让别人的）→ new 里 `4GTE` 后 `sub_5AB8C20(working 的 LOCK)`。
2. **`sub_5B0ED40(ref)` 逐属性三方合并**（下文）。
3. `sub_5B211C0(ref, list, n, dbno, base, working, latest, new, flag)` **owner 变更**：读三方记录头第 4/5 字（OWNREF；`dword_6A54024` 当前元素块 +20 → 记录，+16/+20）；base ≠ working（我换了 owner）→ 与 latest 比：`flag`（= 第 4 槽开关）且 base ≠ latest（别人也换了）→ 保留别人的；否则 new 里：到 latest 的 owner 上 `sub_5AB4580(REF, 4, FIRST=11547609, ref, 2)`（4FA：在成员表里定位到 ref，错 6 = 不在）→ `sub_5AEB0B0()` + `sub_5AB3F30(0)` 删掉定位到的成员项（推断）→ 部分存盘时新 owner 不在列表则 `sub_5B1CC10` 先把 ref 挂进新 owner 的成员表 → 回到 ref，`sub_5AECBE0()`（A2 的整条重写）后把记录 +16/+20 改成 working 的 owner。
4. `sub_5B224B0(ref, dbno, base, working, 1, &changed, &kind)` 成员表比较 → 有变：`kind == 2` → `sub_5B1F660`（第 4 槽时先比 base↔latest，别人动过就不覆盖）= **整段覆盖**；否则 `sub_5B20CA0` **逐成员合并**。两者细节见下「成员表合并」。

**SOFTYP 变了 / 撞 refno 的「两边都改」走 `sub_5B214D0(ref, list, n, dbno, base, working, latest, new, delFlag, useLatestMembers)`**：
new 里已有 → `sub_5AB3620(delFlag)` 删 → working 里查 INDEX → `sub_5AE9960` 把 **working 的整条记录**复制到当前输出页 → new 里 `3CTE(INDEX, ref, 新位置, …, 1, 1)` 插入（= 用我的记录整体替换）→ 到 latest 取它的属性块指针 → new 里清记录 +32/+36（属性块地址两字）并 `sub_5AEC110(latestHunk, record, 2, 0, …, 7, 0)` 把 **latest 的属性块**重写挂上（推断：类型变了之后属性块按 latest 的重建，随后由收尾修正）→ 成员表：base↔working 没变 → 收尾；变了且 `kind ≠ 2` → `sub_5B20CA0`；`kind == 2` 且 `useLatestMembers` → 再比 working↔latest：不同 → 收尾，相同 → 用比较器留下的成员缓冲 `dword_6A542C0/C8` 整段重写成员表（`sub_5AECBE0` + `sub_5B21E00` 扩容）。
收尾 = `sub_5B1FB20()` → `4GTE` → `sub_5B1CD90(ref, 1, working)` + `sub_5B0A110`（引用属性修正）→ `sub_5B211C0`（owner）。

**`sub_5B0ED40(ref)`：属性表 + 三个缓冲**
- `sub_5B1CD90(ref, 0, dbno, base, working, &n)`：noun 模板（`sub_5AF6AB0` 取；条目 +0 hash / +8 类型 / +28 长度，`AILEN`=8079022 经 `sub_5B03470` 取长度）∪ 记录属性块条目（块内每项一字：`>>26` 类型、`&0x3FFFFFF` 字数——与 `sub_5AE9960` 搬 hunk 用的同一格式）→ 三元组表 `dword_6453DAC[(hash, type, size)]`（容量 `dword_6453DA8`，30 项步进扩容）。mode 1（`sub_5B0EA00`/`sub_5B214D0` 收尾用）跳过 14/15/16/17 类型；`dword_6453EF4`（只有部分存盘驱动 `sub_5B0D7E0` 置位）时块里出现模板没有的属性 → 冲突 **649**。
- 按最大长度 `malloc` 三个缓冲——"base att" / "working att" / "old to att"（`sub_5AB28A0` 扩容时的标签；第三个 = latest 里的旧值）；切到 new、`4GTE`、`sub_5AEB530` 存 CE。
- 逐三元组：类型 14（表属性）先 `sub_5AF6AB0` 取模板 + `4GAD`（`sub_5AB5920`）取描述子（bit 3 = 变体宽度）；类型 18 → `sub_5B0F1E0`（下）；其它按描述子定元素数：14 → `(size−1)/elemWidth`、15 → `size−1`、16 → 2（refno）、17 → `(size−1)/2`，类型 19/23（变长表）元素数 0；无描述子（未知属性）→ `sub_5B0D950(…, 1, n, 1000, 1, …)`。

**`sub_5B0D950(ref, idx, elemWidth, count, size, isDefault, &lenB, &lenW, &lenL, &bufB, &bufW, &bufL)`：每个属性（数组属性每个元素 k = 1..count）三读一写**
1. 切 base → `4GTE` → 读值：类型 14/15 用 `4GCETA`（`sub_5AB62C0(hash, k, type, elemWidth, buf, &len, aux[512], &auxLen)`），其它 `4GCEA`（`sub_5AB5BC0(hash, type, cap, buf, &len, 0)`）；错误 **534/18/523 = 未设 → len 0**、**17 → len −1**（未设标记；推断）、**23 = 缓冲太小 → `sub_5AB28A0` 扩容重读**（`size > 1` 的表属性遇 23 直接记冲突）。
2. 切 working → 同样读。**base == working（长度相等、按类型换算的字数逐字相等、aux 逐字相等）→ 我没改 → 跳过**，new 里保留 latest 的值。字数：类型 19/23 = Σ(每项长度+3)+1，其它 = `ceil(len / qword_6453DF8[type])`。
3. 切 latest → 同样读。**working == latest → 别人已改成一样 → 跳过。**
4. 第 4 槽（extract flush 变体）时再比 base ↔ latest：**base ≠ latest（别人改过）→ 跳过、不覆盖**。普通 SAVEWORK 没有这一步 → **同一属性两人都改时我的值覆盖别人的（属性级 last-writer-wins）**；A3a.2 的冲突扫描不看属性级并发。
5. 切 new → `4GTE` → 按类型写 working 的值：
   - 14/15：`len == 0` → `4RCETA`（`sub_5AB71B0(hash, k, type)`）删条目（534 可容忍）；否则 `sub_5AB91B0(hash, k, type, buf, &len, aux, auxLen, elemWidth)` 写（523/529 可容忍）；类型 15 撞 529（键已被别的元素占）→ `sub_5AFFC20(dbno, hash, key)` 找占用者 → 到它上 `4RCETA` 删 → 回来重写（**唯一键属性：我的抢占**）。
   - 17（值同时进以属性名命名的用户表）：`sub_5AB8DC0(hash, 17, extra, buf, &len)`；523（表不存在）→ `sub_5AFF6D0(dbno, hash)` 建表再 `sub_5AB91B0`；**23 → 记冲突 `(ref, 23, hash)` → 576**。
   - 16（refno 值）：`sub_5AB8D20(hash, 16, 2, buf, &2)`。
   - 其它：base 为「未设」(−1) → 先 `4RCEDA`（`sub_5AB6E90(hash, 0)`）；working 为空且（hash 不是合法 ENHAS2 六字名——`hash−531442 > 27⁶−2`，即 UDA/编号属性——或描述子可复位 `isDefault`）→ `4RCEDA(hash, type)` **复位成默认**（18 可容忍）；否则 `4SCEA`（`sub_5AB7B90(hash, type, n, buf, &len, 0, 0)`，类型 14 的 n 取三元组长度）→ **23 → 冲突 → 576**。

**`sub_5B0F1E0(ref, idx, max, …)`：类型 18（表/列表型属性）按条目做集合合并**——`sub_5B0BF60` 分别把 base / working / latest 的条目列表拉进临时缓冲（`dword_6A5426C/70`），`sub_5B06590` 求两组差异（base→working = 我的增删，working→latest = 别人的），`sub_5B0A920` 调和后逐条 `sub_5AB4BB0`/`sub_5AB4C60` 定位 → `sub_5AB91B0` 加 / `4RCETA` 删；键落在独立 B+ 树的经 `sub_5B01340` 查；23 → 冲突。**不是整体覆盖。**

**成员表合并（A3b.4 展开）**

- **内存里的成员表**（当前元素块 `dword_6A54024 + 60·idx` 的 +24，缺了由 `sub_5AEA430(0)` 装入）：`[0]` 改动计数、`[1]` 共享计数、`[2]` 载荷字数（= 2 × 成员数）、`[3]` 容量，第 4 字起每成员一对 refno 字。`sub_5B21E00(n)` 保证还能放 n 个（首分配 `2n+14` 字，否则 realloc 到 `+20`），并把 `dword_6A5401C` 链上其它 db block 里同一元素（+32/+36、+52/+56 相同）的指针改到新缓冲。
- **比较 `sub_5B224B0(ref, dbno, A 会话, B 会话, 1, &changed, &kind)`**：分别切到 A/B、`4GTE(ref)`（10 = 元素不在 → 空表），把成员表指针/长度放进全局 `dword_6A542BC/C4`（A）与 `dword_6A542C0/C8`（B），`sub_5AEB530` 存两次当前元素到 `dword_6A542CC/D0`；`kind` = 该 noun 模板字 3（推断；调用方只认 `kind == 2`）；长度或任一位置不同 → `changed = 1`。光标 `dword_6A542D4/D8` 归 4，上次状态 `dword_6A542DC` 清零。
- **`kind == 2` → `sub_5B1F660(ref, dbno, new)` 整段覆盖**：`sub_5AECBE0` 重写记录 → `sub_5B21E00(B[2]/2)` → 把 B（working）表连头带项原样拷进 new 的表 → `[0]++`。这一档 noun 的成员表被当成一个属性：别人在 latest 里的增删会丢（第 4 槽变体在此之前先比 base↔latest，别人动过就整段跳过）。
- **`sub_5B1FF60(ref, dbno, base, working, new, &st)` = 有序列表差异迭代器**（A = base 表，B = working 表；暂存 `dword_6A542A0`「已处理的移动项」/ `dword_6A542A4`「待落位的移动项」，容量 `dword_6A542A8`）：
  - 两光标指同一 refno → 都前进（位置没变，不产出）；
  - A 光标项在 B 里找不到 → **st 3 = 我删了它**；B 光标项在 A 里找不到 → **st 1 = 我加了它**；
  - 两边都有但位置不同 = **移动**：A 项已在「已处理」→ 跳过；B 项在「待落位」→ 转入已处理，**st 2**；否则比距离 `|A 项在 B 里的位置 − A 光标|` 与 `|B 项在 A 里的位置 − B 光标|`：前者大 → A 项进「待落位」、A 前进；否则 B 项进「已处理」、**st 2**（B 光标处的成员要落到这里）；
  - 一边耗尽：剩余 A 项 → st 3（已处理的跳过）；剩余 B 项 → st 1（在待落位表里的 → st 2）；都耗尽 → **st 4**。下一次调用先推进上一轮被消费那一侧的光标（st 3 推 A，其它推 B）。
- **`sub_5B20CA0(ref, list, n, dbno, base, working, latest, new, flag)` 把每个 st 落到 new 的成员表上**（每轮先 `4GTE(ref)` 切到 new 里的 owner；部分存盘且模板字 4（`*(*(CE+16)+16)`，推断）非零时成员还要过 `sub_5B10DD0` 列表过滤）：
  - **st 3（我删的）** → `sub_5B21F80(dbno, working, A 表, 光标)`：`sub_5AECBE0` 重写记录，在 new 的表里找到就摘掉（后项前移、`[2] −= 2`、`[0]++`）；不在表里 → 去 new/working 里 `4GTE` 该成员并比 owner 记录的 +32/+36，不一致才重扫一遍（推断：成员表块被别人换过的情形）。
  - **st 1（我加的）** → 先看这个成员元素本身：base 里有、latest 里没有（**别人把它删了**）→ 不加；`flag`（第 4 槽）时改比 base↔latest 的 OWNREF，别人换了它的 owner → 不加；否则 `sub_5B219B0(B 表, 光标)`：已在 new 表 → 幂等返回；否则 `sub_5AECBE0` → 扩容 → **从 B 表光标往前找最近一个「new 表里也有」的兄弟，插在它后面**（找不到 → 插最前，下标 4）→ `[2] += 2`、`[0]++`。
  - **st 2（我移的）** → `sub_5B21C10(B 表, 光标)`：`sub_5AECBE0` → 找该成员在 new 表的当前位置 `cur` 与「B 表里它前一个成员」在 new 表的位置（找不到再往前找最近的共有兄弟）`anchor` → `cur == anchor+2` 不动；否则把中间段整体挪 2 字，成员落到 `anchor` 之后（向前/向后两个方向都处理）→ `[0]++`。
  - 位置没变的成员不碰 → **别人在 latest 里的增删、挪动原样保留**。会被静默吞掉的只有：我把 X 挪到 Y 之后而别人删了 Y → 锚退到前一个共有兄弟或最前。
  - 收尾 `sub_5B1FB20()`：`sub_5AEB3A0` 恢复 `dword_6A542CC/D0` 存的两个当前元素。
- **换 owner 时挂成员** `sub_5B1CC10(ref, &newOwner, dbno, working, new)`：working 里 `4GTE(newOwner)` 找 ref 在其成员表中的位置 → new 里 `4GTE(newOwner)` → 找到了就 `sub_5B219B0(workingList, pos)`（同样的「前一个共有兄弟」锚），working 里也没有 → `sub_5B219B0(&ref, 0)` 插最前。
- `sub_5B214D0` 的 `kind == 2 && useLatestMembers` 分支复用同一套缓冲：working↔latest 相同时直接用 `dword_6A542C0/C8` 整段重写（与 `sub_5B1F660` 同一动作）。

**属性粒度语义小结**
- 只有「我改过」的属性会写进 new；没改的一律沿用 latest——别人的改动自然保留。
- 同一属性两人都改：普通 SAVEWORK 我的覆盖别人的；extract flush（第 4 槽）别人的优先。
- 我把属性清空 → new 里复位成默认（不是写空值）；唯一键类（类型 15）我的键抢占别人的；类型 18 列表按条目增删合并。
- 成员表按「增 / 删 / 移」三种差异逐成员套到 latest 的表上，锚点是「前一个共有兄弟」；只有 `kind == 2` 的 noun 整段覆盖。
- 会让存盘失败的属性级冲突只有：`4SCEA`/类型 17 写入返回 23（值放不进去），以及部分存盘里块中出现模板外属性（649）。

### A4. 刷盘上下文（全局单例）

`sub_5AF0E70(dbno, ext, &first_new, &ses2)`：要求 `dword_6A5411C==0`（否则 502 "Dbno is %d" 嵌套）；置
`dword_6A5411C=1`、`dword_6A54110=dbno`、`dword_6A54118=ext`、`dword_6A54114 = dword_6A54120 = dword_6A54108 = first_new`（= 旧会话页第 5 字 +1）。
`dword_6A54114` 是**下一个可分页号**（分配即 `++`），`dword_6A54120` 是**已写到盘的文件末页**（写页时推进）。
`sub_5AF1590(flushAll)`：`flushAll` → 先 `sub_5AF2520(0)`；然后全部置 -1、`dword_6A5411C=0`。

### A5. 提交前重编号（临时页 → 真实页号）

顺序固定（`sub_5AE68D0` 旧路与 `sub_5B113E0` 相同）：

1. **`sub_5AE7C30(dbno, dbno, &INDEX根, INT_MAX, INT_MAX)`（5WCEP）**：栈式遍历 INDEX 树（结点头：`[2]` 非零 = 内结点、`[3]` 键宽、`[4]` 值宽、`[6]` 空闲字数；条目从第 7 字起），对每个叶值 `(pageno, packed)`：若 `ext(packed)==0`（记录在临时页）→ 置脏树页，`sub_5AE9960` 把该元素记录**整条复制**到当前真实输出页（`sub_5AE9DF0` 在刷盘上下文里 `sub_5AEE230` 分到真实页号），连同 hunk（数组类型 20–23 按 `qword_6453DF8[type]` 换算字数）一并搬，叶值改成新 `(pageno, packed)`。`INT_MAX` 表示不做 extract 的 `DTGET_0`/`DTPUT_0` 引用换算。
2. **`sub_5AE8040(&INDEX根, dbno, INT_MAX, ext, 0, &newroot)`（5WCTP）**：自顶向下压栈、自底向上对每个树页调 `sub_5AF0860(pfno, &newid)`——摘哈希、`dbno=当前`、`pageno = dword_6A54114++`、`ext=当前`、重挂、置脏——并把新 id 写回父结点条目；根的新 id 经 **`sub_5AFC950(dbno, 13387743, newroot)`** 写入控制块 `cb+12/+16`。
3. **其余表**：`sub_5AF7870` 逐条枚举 `(hash, root)`（用户 int/name/ref 表… 然后 `CLAIM`、`INDEX`），非 INDEX 者同样 `sub_5AE8040` + `sub_5AFC950`（CLAIM → `cb+20/+24`，用户表 → `cb+8` 目录数组 `(hash,pageno,ext)` 三元组，`cb+32` 条数）。`sub_5AFC950` 在 `cb+28 != dword_6453DF4` 时先克隆控制块并链上（每会话/每 mark 一份根快照）。
4. 至此所有脏页都有 `≥ first_new` 的连续真实页号，且每张的父指针已指向新号。

### A6. 会话页构造与提交 `sub_5AE38B0`（字表见 §2）

1. `sub_5AEE230(1, &sesPageId, &buf)` 分一页（页号 = 当前 `dword_6A54114`，即紧跟数据/树页之后）→ 按 §2 表填字；`第 5 字 = sub_5AF0C10() = dword_6A54114 - 1`（会话页自身 = 末页）。
2. 取页 0（锁 1），token → 本地缓存登记 `sub_5BCC1D0(…, 9 字)`；把旧页 0 第 10/11 字抄进会话页第 1/2 字（链到上一会话）。
3. **`sub_5AF2520(pkg)`：刷全部脏页（含新会话页）** —— A7。
4. 改页 0：第 10/11 字 = 新会话页 id；第 9 字 `TMPLVN` >0 则取负；属性 10==1 且 12≠1 时第 14/15 字 = 属性 7；第 94/96 字清零；`sub_5AF1A00(page0_pfno, pkg)`（`1WDBP`：页号 ≤1 时校验第 0 字 == 页号，写块 1）→ **页 0 落盘 = 提交点**。
5. 解锁两帧。

### A7. 刷盘顺序 `sub_5AF2520` + 单页写 `sub_5AF1C30`（1WP）

1. `sub_5AF2520(pkg)` 两遍扫帧表（帧 2..N+1，`dbno==当前 && ext==当前 && 脏`）：
   **第一遍写 `pageno ≥ dword_6A54120`（本次新增页）**，`DB_MULTIPAGE_WRITE`（`dword_6A54160`）开着时只记最高页号、循环后一次 `sub_5AF1C30(maxpfno)` 触发批量；
   **第二遍写 `pageno < dword_6A54120`（改写既有位置的页）**。写完清脏位。
2. `sub_5AF1C30(pfno, pkg)`：要求上下文打开（503）且帧属当前 (dbno,ext)（506）；`pageno ≤ file_size` → 直接 `sub_5AF2260(token, pageno+1, buf, 页字数, pkg)`，写后 `dword_6A54120 = max(...)`；
   `pageno > file_size` → **先把 file_size..pageno-1 的全部中间页按序写出**（必须都在内存，否则 505；因为新页号是顺序分配的，中间页一定是本会话的脏页），批量模式下每 32 页拼进 `dword_6A54158` 一次 `FHDBWN` 多页写（"Writing %d buffered pages from block %d"），最后写目标页。**文件因此只增不洞、新页永远追加在旧末页之后。**
3. ext==0 分支：写临时文件（见 A2.4）。
4. 成功后计数 `dword_6A54154`（`DB1_n_pages_written`）。

### A8. SessionPackage（3.1 引入的会话写入器抽象）

vtbl（`??_7SessionPackageNative@Dabacon@Database@Core@Aveva@@6B@` 0x5D8D9E8 / `…Central…` 0x5D8DA74）：`+0` 析构、`+4 begin`、`+8 commit`、`+12 addPage(token, blk, buf, nwords)`、`+16 hasBlob`、`+20 getSesno`、`+24 getError(code,&text)`。

- **Native**（`sub_5B1BFE0(dbno, ext, flag)` 构造，0x34 B：+8 dbno、+12 ext、+20 sesno、+24 错误码、+28 错误文本；+4 已切独占、+5 自己开了上下文）：`begin` `sub_5B1C1C0` = 属性 12==2 拒绝；`sub_5AF0FB0(1)` → `FHSWIT "QXMO"` 独占改写（失败 700 "Failed to switch database file mode to exclusive modify"）；属性 12 == 0 时自己读页 0→会话页→`sub_5AF0E70(end+1)` 开上下文。`addPage` `sub_5B1C2B0` = 遇 512 字、类型 3 的页记下 sesno，然后 `sub_5AF2260(…, NULL)` 直接 `FHDBWN`。`commit` `sub_5B1C0F0` = 自己开的上下文才 `sub_5AF1590(1)` 关（顺带刷脏）→ `sub_5AF0FB0(0)` 回 `READ`（失败 706 "Failed to switch database file mode back to read"）。析构 `sub_5B1C040` 做同样的事——存盘半途失败时也会走到，但那时脏页都还是 ext=0 的临时页，`sub_5AF2520` 只写 `(dbno, ext)` 匹配当前库的帧，所以不会有半个会话落盘。
- **Central**（`sub_5B1C2F0(dbno, ext, addToTxn)` 构造，0x94 B：+8 内嵌 `DatabaseSyncCentral`（`sub_5B29760(dbno, ext, flag)`，vtbl 0x5D8DD44）、+100 dbno、+104 ext、+108 送给服务的 ext（注册表有强制值则取负）、+112 addToTxn、+116 sesno、+120 错误文本、+144 错误码；+4 已 begin、+5 已 commit）：
  - `begin` `sub_5B1C4C0`：属性 12 == 2 拒；`Spectrum_StoreClient()` 可用（结果缓存 `byte_6A54290/1`）否则 704 "Service client unavailable"；`sub_5B29E10` 取 16 字节 store id（`dword_6A55698..9C`，长度不是 16 就失败）；`sub_5B1BE30(dbno, ext)` 查注册表项 +28（>0 → `+108 = −值`）；
    **`DatabaseSyncCentral::begin` `sub_5B29930`**：`Spectrum_TransactionClient()` → `sub_5AF7020(dbno, ext, &localSesno, …)` → `Spectrum_TransactionLockDatabase(name, "%d~%d" id, &serviceSesno, &sesnoOut, &status, …)` 最多 20 次、退避 `SleepEx(1 << min(i,14))`（`sub_5B1C8F0`）→ 失败 700 "Failed to begin database synchronization" / status 2 → 709 "…Maintenance Mode"；
    锁到后**等本地视图追上服务端会话号**（`sub_5AF7020` ≥ serviceSesno，≤20 次）——别人刚提交的会话必须先在本地可见才允许写在它之上，否则解锁（≤10 次）并 708；服务端回 0 时 `sesnoOut = local + 1`；
    → `StartSaveSession(storeId, dbno, +108, sesno, &err)` → `+4 = 1`，失败 704。
  - `addPage` `sub_5B1C720`：未 begin → 0；512 字且类型 3 的页 → `+116 = 第 3 字`（记住实际会话号）；导出函数 `AddPage(dbno, blk−1, buf, &err)`（**0-based 块号**）失败 707。页由中央服务落盘，本进程不写本地文件。
  - **`commit` `sub_5B1C450` → `sub_5B1C790(1)`**：未 begin → 1；`EndSaveSession(dbno, +108, sesno, &status, &err)` 成功 → `+5 = 1`，且 `+112` 时 `sub_5B1BE00` 取事务名 → `Spectrum_AddToTransaction(txn, dbno, +108, sesno)`；失败：status 2 → 709 "Spectrum Store Service is currently in Maintenance mode"，否则 **705**（5SW 报的 "commit 失败 705" 就是它）；
    然后 **`DatabaseSyncCentral::end` `sub_5B29BA0(已提交 ? sesno : 0)`**：`Spectrum_TransactionUnlockDatabase(name, id, sesno, &status, …)` ≤10 次退避 → 失败 706 "Failed to end database synchronization" / 709；错误码/文本经 sync 对象 vtbl+0 取回；最后 `+4 = 0`。
  - 析构 `sub_5B1C3C0` → `sub_5B1C790(0)` = **放弃路**：不调 `EndSaveSession`，只解锁——begin 之后任何失败（合并冲突 576、`sub_5B113E0` 失败）都以 unlock 收场，服务端不会看到半个会话。
  - `getSesno` `sub_5B1C4A0` = `+116`（>0 才算有）；`getError` `sub_5B1C460` = `+144`/`+120`；两张 vtbl 的 +16 槽都指向被链接器折叠的常量函数（IDA 借名 `DB_DB::concurrency` / `DB_DsxMember::getMaxCardinality`），即 `hasBlob` 是常量。
  - 注册表：`sub_5B1BF70` 在 `std::map<(dbno, ext), entry>`（end 哨兵 `dword_6A54288`）里找；entry+24 = 「中央/页服务库」标志（`sub_5B1BEE0`；5SW 用它选 Central/Native，`sub_5AE5770` 用它拒部分存盘 699）、+28 强制会话号（`sub_5B1BE30`）、+36/+40 两个 id（`sub_5B1BE80`，格式化成 "%d~%d" 作事务锁的库标识）。
- 所有页写在 `sub_5AF2260(token, blk, buf, n, pkg)` 汇合：`pkg==NULL` → `FHDBWN`；否则 `pkg->addPage(...)`，失败文本 "failed to add page to session package (%d %s)"。

### A9. FH 文件层（FORTRAN 风格文件句柄，token 1..9999）

- 写：`FHDBWN(&tok,&blk,buf,&nwords)` 0x5B8F2A0 → `sub_5B9B9D0(tok, blk, buf, nwords×4, swap=1)`：校验 token（"TNV"）、文件须处于写模式 3（"FWT"）、模块 4/7（"NWA"）、blk>0（"BBN"）、长度整块（"BSE"）；`SetFilePointerEx((blk-1)×recl×4)`；若 `sub_5B933D0(entry)`（顺序写合并）开着，则把连续块攒进每 token 4 KB 缓冲 `dword_6A9549C[tok]`，不连续时先冲出；`subtype==1 && swap` → `sub_5B9C270` 大端化到 64 KB 栈缓冲；`sub_5B92E50`（**FHWRIT**）循环 `WriteFile` 直到写满并推进文件位置（`sub_5B93420`/`sub_5B935C0`），失败经 trace 开关 `dword_729D6C0 & 1` + `strstr(byte_729D6E0,"FHWRIT")` 打印 "FHWRIT: token %d: WriteFile on … returned MCD %d"。
- 读：`FHDBRN` → `sub_5B9B400`；DB2 层 `sub_5AF0640` 失败码 11 时 `SYWAIT(0.5s)` 重试，再失败则 `FHSWIT CLOSED` → 重开原模式再读，最后经回调 `dword_6453B7C` 问用户/`FLFINI("ABORT")`。
- 写失败恢复 `sub_5AF2260`：FH 错误 11（MCD 介质错）→ 关/重开 + 用户回调循环；36/37（DIF 磁盘满）→ 回调 `dword_6453B78` 让用户清空间；仍失败 → 37 "User has not solved problem after MCD error" / "…not cleared enough space after DIF error"。
- 模式切换 `FHSWIT(&tok, "QXMO"|"MODIFY"|"READ"|"CLOSED"|"MOD"|"QSRE"…)` 0x5468380 → `sub_5B90720`：`CreateFile(name, access, FILE_SHARE_READ|WRITE, OPEN_EXISTING, NORMAL)`；**锁 = `LockFileEx` 于 `Overlapped.Offset = 0x7FFFFFF6FFFFFFFF` 处 1 字节**（QXMO 独占 `LOCKFILE_EXCLUSIVE_LOCK`，其它共享；`FAIL_IMMEDIATELY` 重试）；**从写模式（access 4/7）切走时先 `FlushFileBuffers`**（第 280 行）；`SetEndOfFile` 用于截断/预留。`sub_5B8F880`「FHLOCK (BRIEF)」= 短暂独占锁+解锁作栅栏（读到 15 时等写者完成）。
- `sub_5AF0FB0(kind, dbno, ext)`：1→`QXMO`，3→`MODIFY`，其它→`READ`；对 "Page Service" 文件切模式会被拒（错 46）。

### A10. 外部整页写 `DWRTPG`（`sub_5AFD5A0`，工具/复制路径，不走页缓存）

1. 类型 >7 拒（578）；目标文件若正被本进程按 DB 打开（`sub_5AEDFA0`/`sub_5AF58E0`）→ 597。
2. 写页 0：读旧页 0 比对——第 10 字必须相同（629 "Old page0 points to sess pageno %d, new page0 to %d"）、`DBNO` 相同（512）、`CRINFO` 相同（615）。即 **DWRTPG 不允许借写页 0 改提交指针**。
3. 页号 ≤1 须 `FHSWIT "QXMO"` 成功；类型 >1 的页：读页 0 → 最新会话页 → 末页 `v36`；写既有位置时旧页类型必须等于新页类型（614），会话页要求独占。
4. **延迟提交表 `dword_6A5419C`**（16 B/项：token, sesno, 会话页号, 末页号）：写入类型 3 页时登记；当写到页号 == 登记的末页时，`sub_5AF3000`（`2CSP`：从末页倒查到会话页，核对 sesno/类型/链）通过后才把页 0 第 10 字改成该会话页号并写页 0，然后 `FHSWIT "MOD"`。→ 外部工具按页复制也保持「先数据页、后会话页、末页齐了才改页 0」的顺序。

### A11. 多用户/多写与页 1 桶表（本轮只定位，未逐指令展开）

- 页 1（类型 1）保存 per-user/per-bucket 条目；`2GNE`（`sub_5AF7480`）分配新条目（"There is already an entry on page 1 for user/bucket number %d"），`2FUB`（`sub_5AF59E0`）找空桶，`2COUBE`（`sub_5AF4FC0`）、`2UP1UOR`（`sub_5AFD040`，package 路存盘前调用）更新；对应 API `DADBKT`/`DGTBKT`/`DUPDBL`/`DREMBE`/`DFNDUB`/`DGETBL`，C++ `DB_DB::updateBucketList` 0x59144E0。
- shared 模式（属性 12==2）存盘不建新会话、就地改会话页（A3.3 第一条）；`DUPSES db_check_and_update_session`（`sub_5ABB660`）用于读端跟进他人新会话。

### A12. extract 间刷写 `DB_DB::flush*`（只列入口）

`DB_DB::flush / flushWithoutRefresh / internalFlush / internalGlobalFlush / getRequiredClaimsForVariantFlush / doClaimForVariantFlush`（0x5903B10…0x5908AB0）→ C API `db_flush_from_local_ext`(DEXFL) / `db_flush_to_owning_ext`(DEOFL) / `db_flush_db_*` / `db_flush_all_but_*` → 失败登记 `db_register_failed_flush`(DREGFF) / 回滚 `db_undo_failed_flush`(DUNDFF)、`DUDFUP`。这条路是把元素从子 extract 复制到父 extract 的 DB 文件，落盘仍走 A4–A7 的同一套上下文/会话页/页 0 提交；本轮未展开其合并规则。

---

## 4. 不变量与崩溃一致性（从上面各算法直接得出）

1. 盘上已有页在会话进行中**零写入**；SAVEWORK 前进程崩溃 = 丢本会话全部改动，文件完好。
2. 新页号严格顺序分配（`dword_6A54114++`），写入时补齐中间页，所以**文件只在尾部增长、无洞**；`file_size` 由会话页第 5 字（末页）定义而不是 OS 文件长度。
3. 提交顺序：数据页/树页（第一遍） → 改写位置页（第二遍） → **页 0**。页 0 第 10/11 字是唯一提交指针；旧会话页链完整保留（`DSWOLD`/`DCMELS` 会话比较、`specialCaseSavework` 差分靠它）。
4. 同一时刻只能有一个刷盘上下文（502），写者靠 `QXMO` 独占哨兵字节锁互斥；读者读页失败码 15 时用 BRIEF 锁等待写者。
5. 元素记录本身是**不可变快照**：改一次就整条搬新位置，INDEX 叶值换指针；所以 `(sesno, refno)` → 记录位置在任何历史会话里都可重放。

---

## 5. 对 gen-model / e3d-io 读侧的直接印证

- `7618377 = CLAIM`、`13387743 = INDEX`、`276202698 = OWNSES`（ENHAS2 逆解）——e3d-io 把 `(5,7618377)` 叫 claim 是对的；INDEX 是 refno→记录位置树。
- **INDEX 叶值 `(pageno, packed)` 的位移之争（`>>12` 还是 `>>13`）——两边都对，单位不同**：
  写侧 `sub_5AE9DF0` 打包 `packed = ext | (字偏移 << 13)`（`dword_6A540BC` = 当前输出页的 ext，`dword_6A540B4` = 页内已用字数），
  读侧 `sub_5AE9960` 解包 `ext = packed & 0x1FFF`（13 位 = 记录所在页的 extent 号：1 = 主文件、0 = 未存盘的临时页）、`记录 = 页缓冲 + 4 × ((packed >> 13) & 0xFFF)`（12 位，**单位 4 字节字**）。
  e3d-io 用 `offset_words = packed >> 12`（单位 2 字节）、`flag = packed & 0xFFF`：字节偏移 `2 × (packed >> 12)` 与 core 的 `4 × (packed >> 13)` 在 **bit 12 = 0（ext < 4096）时逐位相等**，`flag` 就是 ext 的低 12 位——
  语料 940 694 条叶值 `flag` 全为 1，正因为全部样本都是单 extent。e3d-io 用语料「否决 bit 13」的实验假设偏移单位是 2 字节，换成 core 的 4 字节单位后 bit 13 恰好成立。
  结论：**e3d-io 代码不用改（数值等价），注释要改**——本轮已改（只改注释，未改代码/测试）：`src/index/mod.rs`（模块头、`IndexEntry`、`to_page_address`）、`src/index/cursor.rs`（`RecordLoc`）、`src/record/block.rs`（`Address`）、`src/refno.rs`（`PageAddress` 两个字段的真实含义：extent 号 / 4 字节字偏移）、`src/record/mod.rs`（测试辅助）、`src/index/diff.rs`。
  遗留：多 extent 库里叶值 `flag ≠ 1` 时应按 `flag` 取页而不是继承索引页的 extent（e3d-io 目前是继承，语料无法区分），已在注释里标出，未改代码。
- 会话页第 508 字（e3d-io `SessionTail::base_sesno`）= `sub_5B113E0` 传给 `sub_5AE38B0` 的 **base 会话号**（工作副本所基于的会话，不是文件最新会话），与 e3d-io 观察到的「MERGE CHANGES 后是被折掉的号、回退后比相邻会话小」一致。
- 会话页第 505 字 / CLAIM 表值第 1 字 = `*dword_6453DF0` = 当前写者的用户/桶号（页 1 `2UP1UOR` 用同一变量）；CLAIM 值第 2 字 = 最近改动该 claim 下元素的会话号（A3a.6/7）。
- **存储类型号对表（A3b ↔ e3d-io `StorageShape`）**。core 的宽度表 `qword_6453DF8[type]`（元素/字）在 .bss，但初始化函数 `sub_5AE4A90`（`sub_5AE31E0` 调）把它从 `.rdata` 常量 `xmmword_5D8B9B0…5D8BA20` 和字面量 0.5 整段拷入——**可以直接从 IDB 读出**（e3d-io `descriptor.rs` 注释说"filled in at startup, so it is not readable from the binary"，这一句不对；它用 35 734 条描述子拟合出来的比值与实读全部一致）。模板里出现的类型只有 2–10、14–19（e3d-io `tools/probe_type_widths.py` 实跑）；11–13、20–23 只在运行时/记录 hunk 目录里出现。

| 类型 | core 元素/字 | 字/元素 | core 侧用法（本文实读） | e3d-io `StorageShape::of` | 结论 |
|---|---|---|---|---|---|
| 0, 1 | 0 | – | 未用 | None | 一致 |
| 2 | 0.5 | 2 | real；模板 flag bit 29 清零（compact，f32）时 `sub_5B1CD90` 把长度 ×2 展宽 | Real（wide 2 / compact 1） | 一致 |
| 3 | 1 | 1 | int / word | Word | 一致 |
| 4 | 0.5 | 2 | ref | Reference | 一致 |
| 5 | 32 | 1/32 | 位 | PackedBit（`location >> 20` 选位） | 一致 |
| 6 | 0.5 | 2 | real 数组（展宽规则同 2） | Real | 一致 |
| 7 | 1 | 1 | int 数组 | Word | 一致 |
| 8 | 0.5 | 2 | ref 数组 | Reference | 一致 |
| 9 | 32 | 1/32 | bool 位集 | BitSet | 一致 |
| 10 | 4 | 1/4 | 文本（4 字符/字） | Characters | 一致 |
| 11 | 0.25 | 4 | 只在运行时表；模板未出现 | None | e3d-io 无；不影响描述子解码 |
| 12, 13 | 0.5 | 2 | 同上 | None | 同上 |
| 14 | 1 | 描述子定（1/2/3/4） | **键控表属性**：`4GCETA`/`sub_5AB91B0(hash, k, …)` 按下标 k 逐条读写，重复键 529 = 硬错；`sub_5B0ED40` 元素数 = (size−1)/描述子宽度；UDATAB(2 字)/UDAFTB(4 字)/UUIDAR(4 字)/TYPEX/SPAMAP | Record（宽度 = (words−1)/count） | 一致；e3d-io 未写「按 k 键控」的写侧语义 |
| 15 | 4 | 1/4 | 文本 + **唯一索引**：值同时进以属性 hash 命名的 B+ 树（= 会话页表目录里的「用户表」，A3a.4），写入撞 529 → `sub_5AFFC20` 找占用者、删掉再写；**NAME** / DRTYPT / DRSTYT / RDFIRI | Characters | 一致；补充：NAME 的唯一性就来自这张表 |
| 16 | 0.5 | 2 | 单 ref，`sub_5AB8D20`；SPRE / ISPE / LSTU … | Reference | 一致 |
| 17 | 0.5 | 2 | ref 数组 + 以属性 hash 命名的索引表（523 → `sub_5AFF6D0` 建表再写；写入 23 → 冲突 576）；MDSYSF / SCAREF / DESREF | Reference；`descriptor_is_array` 含 17 | 一致；补充：带索引表 |
| 18 | 1 | 1 | **集合型**：`sub_5B0F1E0` 按条目求差、逐条增删合并；模板里只有 UDASTB(1000) | Word（"no decode path"） | 宽度一致；语义 e3d-io 未覆盖 |
| 19 | 4 | 1/4 | **变长项列表**（已定，见下「类型 19 布局」）：值 = `[项数]` + 每项 `[w0][len][w2][len 字载荷]`；`4SCEA` 原样写进 explicit 块、`sub_5AB4CF0` 原样读回，长度按**字数**记（19/23 不除比值）；`descriptor_is_array` 含 19 ✓；CLAURI / ROKEYS / ROVALS / DIURIS / GNOTES / ECMTDK | Characters（单串，计数字 = 字符数） | **不一致但休眠**：e3d-io 会把项数当字符数解成乱串；本机全部 E3D 3.1 库里没有一个类型 19 属性被设过值（下），所以从未触发 |
| 20 | 1 | 1 | hunk 数组（`sub_5AE9960`：20–23 的数据在 (页, packed) 外联块，目录项带长度） | None | e3d-io 无；模板不出现，只在记录 hunk 目录 |
| 21 | 0.5 | 2 | hunk double 数组（与 2/6 同样 ×2 展宽） | None | 同上 |
| 22 | 4 | 1/4 | hunk 文本 | None | 同上 |
| 23 | 4 | 1/4 | hunk 文本列表（`sub_5AE9960` 项数取目录第 6 字；`sub_5B0D950` 与 19 同分支） | None | 同上 |

  另：e3d-io `UNSET_SENTINELS` 注释点名的 `sub_5AB62C0`/`sub_5AB71B0`/`sub_5AB7B90`/`sub_5AB91B0` 与本文 4GCETA/4RCETA/4SCEA/表写入的命名一致；`descriptor_is_array`（14/17/19 恒为数组）与 A3b 的处理一致。

  **类型 19 布局（样本找不到，改从写侧实读）**。先在样本上找：用 e3d-io 的 `TemplateProvider` + `scan_elements(Full)` 写了个临时探针（%TEMP%，未入库）扫本机全部 E3D 3.1 库
  （510 DESI / 110 CATA / 26 DICT / 27 SCHE / 131 PADD / 14 ENGI / 3 CONF，AvevaMarineSample、AvevaPlantSample、AvevaCatalogue、SCB、ZDJ、TEST、YCYK 等）：模板里带类型 19 的元素全部 **未设值**——
  FIXING/PFIT/HRPANE/HRGATE/HRPOST/HRTERM 的 ROKEYS/ROVALS 9 421 个元素、TABITE.CLAURI 116 688、UDA.TADFLT 6 586、SC*.CLAURI ≈5 800、VIEW.VLOCDE 2 523、DSTMPL/PBSTPL.DIURIS 40、CFGVAL.CFGTAV 3，
  既无 implicit 槽（ROKEYS/ROVALS 描述子 word offset = 0，只能进 explicit 块）也无 explicit 项；E3D 2.1 的 20 个模板文件里根本没有类型 19。所以改读写侧：
  - `4SCEA`（`sub_5AB7B90`，`v114 == 19` 分支）把值写进 explicit 块：项头 `= 字数 | (19 << 26)`，然后 `block[] = buf[0]`（项数），逐项 `block[] = buf[i], buf[i+1] (= len), buf[i+2]`，再拷 `len` 个载荷字，下一项从 `i + 3 + len` 起——即 **`[项数] { [w0][len][w2][载荷×len] }*`**；
  - 读侧 `sub_5AB4CF0` `case 19: 字数 = 项头 & 0x3FFFFFF`，`memcpy(out, &entry[2], 4×字数)` 原样返回（别的类型从 `entry[3]` 起、`entry[2]` 是计数字；19 没有单独计数字）；`4GCEA`/`4FA` 对 19/23 的长度**不除 `qword_6453DF8`**，直接当字数；
  - 4SCEA 的类型升级表：6→21、7→20、10→22、**19→23**（值 ≥ 1024/0x2000 字或 `DB_Attribute::initialised` 时改存外联 hunk），`sub_5AB5BC0` 第 44 行把 19↔23 视为同类——23 就是 19 的外联形态；
  - 这与 A3b `sub_5B0D950` 对 19/23 的字数公式 `Σ(len+3)+1` 完全吻合。
  - **项头两字的含义（从 C++ 封装实读）**：类型 19 的 API 是 **`DPUTSVEC`/`db_cxx_put_str_vector`（例程 639/640，包装 `sub_5AAA8B0` → 实现 `sub_5AD5060`）**，入参就是一个 `std::vector<std::string>`（元素步长 24 = MSVC `std::string`）；读侧 `DGETSVEC`/`db_cxx_get_str_vector`（641/642，实现未定位）；另有 `DPUTSRTA`/`db_cxx_put_raw_string_array`（645/646，`sub_5AB15D0` → `sub_5AD3590`）与 `DGETSRTA`/`db_cxx_get_raw_string_array`（647/648）直接收发已打包的字数组。
    `sub_5AD5060` 用一个 `std::vector<u32>`（`sub_59FCED0` = `push_back`）攒值：先 `push(串数)`；对第 i 个串：按属性 18（UTF-8 开关）决定直接 `sub_54719F0` 打包（4 字符/字），还是先 `sub_5B8F3C0`+`sub_54B2CE8` 转成项目字符集（转换失败 → 错 23/638）再 `sub_5471E50` 打包；然后 **`push(i)`（0 起的项下标）、`push(字数)`、`push(转换后的字节数)`**，再把打包好的字整段追加；超过 0x10000 字节的串走堆缓冲同样格式。最后 `4SCEA(hash, 19, 总字数, buf, &len, 0, flags)`。
    → 项头 `w0` = 项下标、`len` = 该串的字数、`w2` = 该串转换后的字节长度（UTF-8 库里 = 原字节数）；载荷 = 4 字符/字打包文本。空 vector → `4SCEA(hash, 19, 0, …)` 写空值。
  结论：**类型 19 = 串（项）列表，不是单串**——它就是 `std::vector<std::string>` 的落盘形态，布局 `[n] { [i][words][bytes][packed text × words] }*`；e3d-io 的 `Characters` 解法对 19 是错的，但因为语料里没有设过值的 19，从未产生错误输出。已把这一段写进 e3d-io `src/record/descriptor.rs` 的 `Characters` 变体注释（只改注释，代码未动）。
- 会话页第 199 字条数 + 第 200 字起三元组表目录，与 e3d-io `+0x31C` 命名表目录一致；顺序是用户表在前、CLAIM、INDEX 在后。
- 会话页第 5/6 字 = 末页，第 1/2 字 = 上一会话页：e3d-io `end` / `previous` 命名一致。
- 写侧只追加、页 0 单点提交 → 读侧「从页 0 的 `LSPGNO` 进入、沿第 1/2 字回溯」即可拿到任意历史会话的一致快照；增量差分（`DB_RawChanges::createListBetweenSessions`）在 core 里就是两棵 INDEX 树的归并比较，与仓库现有 ADR-056 增量口径一致。

---

## 6. 未验证 / 待补

- 属性号 → 控制块字段的**语义命名**（0 ext、1 当前 sesno、3/5 两字根、7 两字、9 表数、10 写权限、12 打开模式、17 extract/claim 标志、18 UTF-8）多数由用法推断，未逐一从 `DOPENW` 写入处反证。
- A3a/A3b 里标「未展开」的：类型 18 集合合并 `sub_5B0F1E0`/`sub_5B06590`/`sub_5B0A920` 的调和细则、主元素相关 `sub_5B07F50`/`sub_5B081F0`/`sub_5B084D0`、部分存盘驱动 `sub_5B15E10`、`sub_5B113E0` 的 a6 = 1/2 变体、预置目标 `dword_6453EC0/EC4` 的设置者（`sub_5AE4A90` 只把它清零，真正的设置者是 `sub_5AF3710`，未读）、第 4 槽 `6A54260` 的 extract flush 变体、`sub_5B21F80` 成员不在表里时那段 +32/+36 核对的确切用意；
  `sub_5B0D080` 的 `a7` 与 `DXSWK2` 的对应、`sub_5AB3620(0)` = 删元素、`sub_5B059C0` = 找主元素、`4GCEA` 错误 17 = 「未设标记」、`sub_5AB4580(REF,4,FIRST,…)` + `sub_5AB3F30(0)` = 定位并删成员项、成员表 `kind`/过滤标志 = 模板字 3/4、`sub_5B214D0` 里挂 latest 属性块的用意 均为用法推断。
  存储类型号已与 e3d-io `StorageShape` 对表（§5），宽度全部一致；类型 19 已从写侧定为「串列表」且项头三字含义已读出（下标 / 字数 / 字节数），未定的只剩 `db_cxx_get_str_vector` 的实现位置与本机之外是否存在设了值的样本。冲突码 23/561/562/565/573/576/582/590/648/649 对应的用户可读文本本轮未定位。
- 页 1 桶表条目格式、`DWRTP0`/`DADSP0`、extract flush 合并规则未展开。
- CLAIM 树：键 = refno、值 = (用户/桶号, 最近改动会话号) 已从写侧读出（A3a.6/7），未在样本库上逐条核对。
- `DB_MULTIPAGE_WRITE` 环境变量取值语义（`sub_5AEF680` 只读到开关赋值）未细读。

---

## 7. 复现命令（本机）

> 注意：本机 `ida-bridge` 安装目录已由 `C:\Users\dpc\.agents\tools\ida-bridge` 改名为 `.agents-old`，用户 PATH 与 venv 里的 editable `.pth` 仍指旧路径，直接敲 `ida-bridge` 会静默退出（exit 1）。本轮用下面的包装绕过（不改任何系统配置）：

```powershell
# %TEMP%\ib.ps1
$env:PYTHONPATH = "C:\Users\dpc\.agents-old\tools\ida-bridge\src"
$env:IDA_BRIDGE_LOG_DIR = "$env:LOCALAPPDATA\ida-bridge\logs"
& "C:\Users\dpc\.agents-old\tools\ida-bridge\.venv\Scripts\python.exe" -c "import sys; from ida_bridge.cli import main_cli; sys.argv=['ida-bridge']+sys.argv[1:]; main_cli()" @args
```

```powershell
& $env:TEMP\ib.ps1 list                                                      # idalib-33060 = plant3\core.dll.i64
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5AE68D0)"    # 5SW save work 主体
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5AE38B0)"    # 会话页构造 + 提交
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5AF2520), decompile(0x5AF1C30), decompile(0x5AF1A00)"   # 刷盘顺序 / 单页写 / 页0写
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5AE7C30), decompile(0x5AE8040), decompile(0x5AFC950), decompile(0x5AF0860)"  # 重编号
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5AEE4E0), decompile(0x5AEF150), decompile(0x5AF1600), decompile(0x5AEE230), decompile(0x5AE9DF0)"  # 页缓存 / CoW / 记录分配
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5B9B9D0), decompile(0x5B92E50), decompile(0x5AF2260), decompile(0x5AF0640), decompile(0x5B8F880)"  # FH 层
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5AFD5A0), decompile(0x5AF3000)"   # DWRTPG / 2CSP
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5B1C1C0), decompile(0x5B1C0F0), decompile(0x5B1C2B0), decompile(0x5B1C4C0), decompile(0x5B1C720)"  # SessionPackage
& $env:TEMP\ib.ps1 exec idalib-33060 -c "import ida_bytes; _result_=[ida_bytes.get_dword(0x6003900+4*i) for i in (197,198,289,290,459,460)]"   # 例程编号表指针
# A3a 多写合并
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5B113E0)"                                        # 合并 + 提交主体
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5AF3940), decompile(0x5AF2A70), decompile(0x5AF54B0)"   # db block 建/切/删
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5B10410), decompile(0x5B07750), decompile(0x5B1F890), decompile(0x5AE5E00)"   # 冲突扫描
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5B0C090), decompile(0x5B0EA00), decompile(0x5B0FDC0), decompile(0x5B06C40)"   # 元素合并
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5B0D080), decompile(0x5B07080), decompile(0x5B0C760)"   # 用户表 / INDEX 去重 / claim 修正
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5B029A0), decompile(0x5AFFCB0), decompile(0x5AFFA60)"   # 3STC 表比较
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5B14A80), decompile(0x5B242C0), decompile(0x5B24B70), decompile(0x5B25030), decompile(0x5B24600), decompile(0x5B25160)"   # claims manager (Native)
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5AE5770)"                                        # 部分存盘主体 DPSWRK/DPSWAB/DPQUIT
# A3b 属性级三方合并
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5B0FDC0), decompile(0x5B214D0), decompile(0x5B211C0)"   # 我改过 / 两边都改 / owner 变更
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5B0ED40), decompile(0x5B0D950), decompile(0x5B0F1E0), decompile(0x5B1CD90)"   # 属性表 + 三读一写 + 类型 18 + 属性表构造
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5AB6020), decompile(0x5AB5BC0), decompile(0x5AB62C0), decompile(0x5AB7B90), decompile(0x5AB6E90)"   # 4GCES / 4GCEA / 4GCETA / 4SCEA / 4RCEDA
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5B224B0), decompile(0x5B1FF60), decompile(0x5B20CA0), decompile(0x5B1F660)"   # 成员表：比较 / 差异迭代器 / 逐成员落地 / 整段覆盖
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5B219B0), decompile(0x5B21C10), decompile(0x5B21F80), decompile(0x5B21E00), decompile(0x5B1CC10)"   # 成员表：插 / 移 / 删 / 扩容 / 换 owner 挂成员
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT func_name, string_value FROM string_refs WHERE func_name IN ('sub_5AB5BC0','sub_5AB62C0','sub_5AB6E90','sub_5AB71B0','sub_5AB7B90','sub_5AB5920') AND string_value LIKE '4%'"
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT name_at(from_func) AS f, group_concat(DISTINCT hex(to_ea)) FROM xrefs WHERE to_ea BETWEEN 0x6A54234 AND 0x6A5425C GROUP BY from_func"   # 合并上下文全局的使用者
# A8 Central
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5B1C4C0), decompile(0x5B1C720), decompile(0x5B1C450), decompile(0x5B1C790), decompile(0x5B1C3C0)"   # begin/addPage/commit/impl/dtor
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5B29760), decompile(0x5B29930), decompile(0x5B29BA0), decompile(0x5B1BF70), decompile(0x5B1BEE0)"   # DatabaseSyncCentral + 注册表
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT hex(address), name FROM names WHERE name LIKE '?Spectrum_%' OR name LIKE '%SaveSession%' OR name LIKE '?AddPage%'"
# INDEX 叶值打包/解包（§5）
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5AE9DF0), decompile(0x5AE9960)"
# 存储类型宽度表（§5 对表）：初始化函数 + .rdata 常量
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT n, line FROM pseudocode WHERE func_ea = 0x5AE4A90 AND line LIKE '%6453E%' ORDER BY n"
& $env:TEMP\ib.ps1 exec idalib-33060 -c "import ida_bytes, struct; d=lambda ea: struct.unpack('<d', struct.pack('<Q', ida_bytes.get_qword(ea)))[0]; _result_={hex(ea): (d(ea), d(ea+8)) for ea in (0x5D8B9B0,0x5D8B9C0,0x5D8B9D0,0x5D8B9E0,0x5D8B9F0,0x5D8BA00,0x5D8BA10,0x5D8BA20)}"
# 对照：e3d-io 侧的描述子几何拟合
python D:\work\plant-code\old\vendor\e3d-io\tools\probe_type_widths.py
# 类型 19 布局（§5）：4SCEA 写侧 / explicit 读侧 / 类型等价
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT n, line FROM pseudocode WHERE func_ea = 0x5AB7B90 AND n BETWEEN 785 AND 830 ORDER BY n"   # v114 == 19：[项数]{[w0][len][w2][载荷]}
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT n, line FROM pseudocode WHERE func_ea = 0x5AB4CF0 AND (line LIKE '%case 19%' OR line LIKE '%v20 == 19%') ORDER BY n"
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT n, line FROM pseudocode WHERE func_ea = 0x5AB5BC0 AND n = 44"   # 6↔21 10↔22 19↔23
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT decompile(0x5AAA8B0), decompile(0x5AD5060), decompile(0x5AB15D0), decompile(0x5AD3590)"   # db_cxx_put_str_vector(640) / put_raw_string_array(646) 包装与实现
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT hex(from_ea), name_at(from_func) FROM xrefs WHERE to_ea = 0x5AB7B90 AND is_code = 1"   # 4SCEA 的全部调用者（含 19 的两处）
# 样本核对用的探针不在库里：一个依赖 e3d-io 的临时 cargo 项目（TemplateProvider::open + scan_elements(Full)，按描述子 word offset 读 implicit、按 hash 读 explicit_attributes），扫 D:\AVEVA\Projects\E3D3.1 下全部库
& $env:TEMP\ib.ps1 exec idalib-33060 --sql "SELECT n, line FROM pseudocode WHERE func_ea = 0x5B90720 AND (line LIKE '%LockFileEx%' OR line LIKE '%FlushFileBuffers%') ORDER BY n"
```

ENHAS2 逆解（本地 Python）：

```python
def dec(h):
    n, s = h - 531441, ''
    while n > 0:
        n, r = divmod(n, 27); s += ' ' if r == 0 else chr(64 + r)
    return s
# dec(7618377)='CLAIM'  dec(13387743)='INDEX'  dec(276202698)='OWNSES'  dec(722578)='DESI'
```
