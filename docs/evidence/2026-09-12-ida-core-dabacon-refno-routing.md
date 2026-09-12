# core.dll 的跨库引用路由（只读取证）——refno → dbno → 已开库 → 目标元素

> 切片：`docs/plans/2026-09-12-cross-db-attribute-refno-routing-dev-plan.md` §0.1 / W9；母计划 `docs/plans/2026-09-12-refno-native-routing-and-lazy-mdb-plan.md` §2。
> 取证工具：ida-bridge 只读 SQL（`names` / `decompile()` / `pseudocode`），实例 `idalib-33060`（E3D 3.1 `D:\ida_scratch\plant3\core.dll.i64`）
> 与 `idalib-19528`（E3D 2.1 `D:\ida_scratch\plant210\core.dll.i64`）。**未写任何 IDB**。
> 首次取证 2026-09-12 10:23–10:28（fable-5-1-5），本文件 13:40 按同一批地址重新拉了一遍反编译片段核对。

---

## 0. 一句话结论

**core 从引用属性读到目标元素的整条路上没有任何「ref0 → dbnum」的全局表。** dbno 是从 refno 的 `word0` 自身按位解出来的
（`w0 & 0x1FFF | (w0 >> 13) & 0x3E000`，2.1 / 3.1 同式）；解出的 dbno 在进程内「已开库有序表」上二分找到打开态；目标元素再在
那个库自己的索引 B+tree 上点查。MDB 只决定「哪些库打开」（`DB_MDB::internalOpenDB` 按 MDB 定义逐库开），不提供反查表。
成本与 MDB 元素总数无关。

我们仓内与之同构的那条路是 `vendor/e3d-io` 的 `RefNo::dbno()` + `DbSet::db_slot()`；gen-model 那条「全库扫描建 ref0 表当定位器」的路
（`InMemoryCataLocator` / `MdbMemoryStore.affiliations` / `model_source::DBNUM_BY_REF0`）是为一个 core 里并不存在的歧义建的，切片
W1–W7 已把属性 / 根定库 / 树 / 探针四处全部换到位解码 + `MdbRouteDirectory`。

---

## 1. 链路总表（E3D 3.1 `core.dll`）

| 环节 | 函数 | 地址 | 反编译要点 |
|---|---|---|---|
| 引用属性取值 | `DB_Element::getElement(DB_Attribute const*, int) const` | `0x5939c60` | 34 行：`DB_Ref::DB_Ref(v9)` → `DB_Element::getAtt(this, attr, qualifier, v9)` → 成功则 `DB_Ref::DB_Ref(a2, v9)` 拷贝构造返回。**不开库、不查表**，返回一个惰性引用句柄 |
| DABACON 取引用 | `DB_Element::dabGetAtt(int, DB_Element&) const` | `0x592c7f0` | 44 行：`elGotoCPP(this, 1)` 先把游标定到本元素，再 `sub_5AABFE0(a2, v7)`（opcode 486，取属性值到两个 word） |
| refno → dbno（C++） | `DB_Element::dbno() const` | `0x5931280` | `db_get_dbno_from_reference(this_ref, &out)`，返回 `out[0]` |
| refno → dbno（DABACON 入口） | `db_get_dbno_from_reference` | `0x5aade70` | `sub_5AC4F40(58, a1, a2)`，opcode **58** |
| opcode 58 派发 | `sub_5AC4F40` | `0x5AC4F40` | 第 27 行 `result = sub_5B05260(a2)`；第 28 行 `*a3 = result` |
| **位解码** | `sub_5B05260` | `0x5B05260` | 全文一行：`return *a1 & 0x1FFF \| (*a1 >> 13) & 0x3E000;` |
| dbno → DB 对象（C++） | `DB_DB::findDB(int)` | `0x5902e80` | 在 `DB_DB::DBLookup_` 红黑树上按 dbno 键查（`v3[4] >= a1` 走左、否则走右），命中取 `v2[5]`；没有则 0 |
| 定位到元素（C++） | `DB_Element::elGotoCPP(bool) const` | `0x59316d0` | → `sub_5AAC300` → opcode 474 `sub_5ACF400` → `sub_5AEB6B0` |
| dbno → 打开态（DABACON） | `sub_5AEB6B0` | `0x5AEB6B0` | 第 54 行 `if ( a1[1] < 0 ) goto LABEL_101`（word1 为负 = 非法引用）；第 56 行 **`v3 = *a1 & 0x1FFF \| (v2 >> 13) & 0x3E000`**（同一个式子再解一次）；第 58 行 `if ( !v3 ) goto LABEL_101`（dbno 0 非法）；第 60 行 `sub_5AF6600(v3, 0, …)` / `sub_5AF6600(v3, 1, …)` 取该库槽位字段 |
| 已开库表 | `sub_5AF6600` → `sub_5AF5820` → `sub_5AF58E0` | `0x5AF6600` / `0x5AF5820` / `0x5AF58E0` | `sub_5AF58E0`：在 `dword_6A54190`（`[slot, dbno]` 对，长度 `dword_6A54180`）上**二分**找 dbno；`sub_5AF5820`：找不到 → `"Dbno is %d"` 错 **41**；找到但 5 个 40 字节子槽全无效 → 错 **635**；`sub_5AF6600`：按字段号从槽里取（0/1/2/9/10/11/12/13/14/15/17/18），未知字段号 → 错 507 `"Attribute name is %s"` |
| 目标元素点查 | `sub_5AEB6B0` | 第 181–183 行 | `v36 = v17 & 0x1FFF; v19 = (v17 >> 13) & 0xFFF;` 再 `sub_5AEDDA0(dbslot, &v35, …)`——word1 拆成页内 / 页号，在**该库**索引上点查 |
| MDB 只管开哪些库 | `DB_MDB::getDB(int) const` | `0x59f4e10` | 按 MDB 向量**位置**取 `DB_DB*`（`this+26..27` 是 `std::vector<DB_DB*>`），越界回 0；不是按 dbno 查 |
| 同上 | `DB_MDB::dbs(int, std::vector<DB_DB*>&) const` | `0x59f33a0` | 按类型列出已开库 |
| 反向引用（对照） | `DB_MDB::findElements(int, DB_Attribute const*, DB_Element const&) const` | `0x59f4720` | 逐库在引用方库的反向表点查再并集——反向方向 core 也没有跨库表（本切片非目标，见 `reverse_reference.rs` / `cable_endpoint.rs` 在案缺口） |

## 2. E3D 2.1 `core.dll` 核对

| 函数 | 地址 | 要点 |
|---|---|---|
| 位解码 | `sub_1064EF60` 第 22 行 | `v4 = *(_DWORD *)a1 & 0x1FFF \| (*(int *)a1 >> 13) & 0x3E000;` —— 与 3.1 `sub_5B05260` 逐位相同 |
| 库怎么进表 | `DB_MDB::internalOpenDB(DB_Element const&, DB_Element const&, bool, int, bool)` | `0x10559680` | 读 MDB 定义（`DBNO` / `ISTYP`）逐库打开；打开的库才进 `sub_5AF58E0` 那张有序表 |

两个版本的位格式一致，所以 `vendor/e3d-io` 的 `RefNo::dbno()`（`refno.rs:17`）一份实现覆盖 2.1 与 3.1；阶段 0 corpus 门在本机全部
1,947 个库 / 38,891,013 个元素上 `refno.dbno() == descriptor.db_mark` 零分歧（切片 §10.1）。

## 3. 反编译片段（3.1，原文）

### 3.1 位解码 `sub_5B05260`

```c
int __cdecl sub_5B05260(int *a1)
{
  return *a1 & 0x1FFF | (*a1 >> 13) & 0x3E000;
}
```

`0x1FFF` 取低 13 位，`(w0 >> 13) & 0x3E000` 把 bits 26..30 落到 bits 13..17；中间 bits 13..25 是 extent 号——所以**同一库每个 extent
一个不同的 word0**，「word0 ≠ dbnum」是对的，但按位解码就够了，不需要表。

### 3.2 `db_get_dbno_from_reference` / `DB_Element::dbno` / `DB_Element::getDB`

```c
BOOL __cdecl db_get_dbno_from_reference(int a1, int a2)
{
  sub_5AC4F40(58, a1, a2);          // opcode 58 → sub_5B05260
  return dword_6453B98 == 0;        // DABACON 错误码为 0 即成功
}

int __thiscall DB_Element::dbno(DB_Element *this)
{
  ...
  v2 = DLL_GlobalInterface::returnError(this);   // 取 this 的引用字段
  db_get_dbno_from_reference(v2, v5);
  v3 = v5[0];
  ...
  return v3;
}

struct DB_DB *__thiscall DB_Element::getDB(DB_Element *this)
{
  ...
  v2 = DB_Element::dbno(this);
  DB = DB_DB::findDB(v2);
  ...
  return DB;
}
```

### 3.3 `DB_DB::findDB(int)` —— 按 dbno 键的红黑树

```c
struct DB_DB *__cdecl DB_DB::findDB(int a1)
{
  ...
  v2 = DB_DB::DBLookup_;
  v3 = (int *)*((_DWORD *)DB_DB::DBLookup_ + 1);
  if ( *((_BYTE *)v3 + 13) )
    goto LABEL_10;
  do
  {
    if ( v3[4] >= a1 ) { v2 = v3; v3 = (int *)*v3; }   // key >= dbno → 左
    else               { v3 = (int *)v3[2]; }          // 否则 → 右
  }
  while ( !*((_BYTE *)v3 + 13) );
  if ( v2 == DB_DB::DBLookup_ || a1 < v2[4] )
LABEL_10:
    v2 = DB_DB::DBLookup_;
  v4 = (v2 == DB_DB::DBLookup_) ? 0 : v2[5];
  ...
  return (struct DB_DB *)v4;
}
```

### 3.4 DABACON 层：`sub_5AEB6B0` 第 53–60 行与 181–183 行

```c
  v2 = *a1;
  if ( a1[1] < 0 )
    goto LABEL_101;                                   // word1 为负：非法引用
  v3 = *a1 & 0x1FFF | (v2 >> 13) & 0x3E000;           // 再解一次 dbno
  *(_DWORD *)ArgList = v3;
  if ( !v3 )
    goto LABEL_101;                                   // dbno 0：非法
  if ( !sub_5AF6600(v3, 0, (int)&v50) || !sub_5AF6600(v3, 1, (int)&v51) )
  ...
        v35 = v16;
        v36 = v17 & 0x1FFF;                           // word1 低 13 位
        v19 = (v17 >> 13) & 0xFFF;                    // word1 bits 13..24
        v20 = sub_5AEDDA0(*(int *)ArgList, (int)&v35, v18, 1, (int)&v45);   // 在该库索引上点查
```

### 3.5 已开库表：`sub_5AF58E0` / `sub_5AF5820` / `sub_5AF6600`

```c
int __cdecl sub_5AF58E0(int a1, int *a2, _DWORD *a3)     // 有序表二分
{
  v3 = dword_6A54180;                                    // 表长
  ...
  while ( 1 )
  {
    v6 = (v5 + v4) / 2;
    v7 = *((_DWORD *)dword_6A54190 + 2 * v6 + 1);        // [slot, dbno] 对的 dbno
    if ( v7 >= a1 )
    {
      if ( v7 <= a1 ) { *a2 = v6; *a3 = *((_DWORD *)dword_6A54190 + 2 * v6); return 1; }   // 命中：slot
      v5 = (v5 + v4) / 2;
    }
    else
      v4 = v6 + 1;
    if ( v4 >= v5 ) goto LABEL_10;
  }
  ...                                                    // 未命中：*a3 = 0
}

int __cdecl sub_5AF5820(int ArgList, _DWORD *a2, _DWORD *a3)
{
  if ( !sub_5AF58E0(ArgList, v8, &v9) ) return 0;
  if ( v9 )
  {
    ... while ( *(v6 - 2) == -1 || !*v6 ) { ++v5; v6 += 10; if ( v5 >= 5 ) {
        LicFeatureCheckOut_FormatSafe(&byte_6453BA0, 0x200u, "Dbno is %d", ArgList);
        dword_6453B98 = 635;                             // 槽全无效
        return 0; } }
    *a3 = v9 + 40 * v7;                                  // 40 字节子槽
    *a2 = v3;
    return 1;
  }
  LicFeatureCheckOut_FormatSafe(&byte_6453BA0, 0x200u, "Dbno is %d", ArgList);
  dword_6453B98 = 41;                                    // 库没开
  return 0;
}

int __cdecl sub_5AF6600(char ArgList, int a2, _DWORD *a3)  // 按字段号取槽内字段
{
  if ( !sub_5AF5820(ArgList, (int)v6, (int)&v5) ) return 0;
  switch ( a2 ) { case 0: ... case 18: ...; default:
    v4 = (const char *)sub_5AEDEF0(a2, v6);
    LicFeatureCheckOut_FormatSafe(&byte_6453BA0, 0x200u, "Attribute name is %s", v4);
    dword_6453B98 = 507; return 0; }
  return result;
}
```

### 3.6 `DB_MDB::getDB(int)` —— 按位置，不按 dbno

```c
struct DB_DB *__thiscall DB_MDB::getDB(DB_MDB *this, int a2)
{
  v2 = *((_DWORD *)this + 26);                                            // vector begin
  if ( a2 >= (unsigned int)((*((_DWORD *)this + 27) - v2) >> 2) || a2 < 0 ) // vector end
    return 0;
  return *(struct DB_DB **)(v2 + 4 * a2);
}
```

## 4. 与仓内实现的对照

| core | `vendor/e3d-io` / gen-model（切片落地后） |
|---|---|
| `sub_5B05260` 位解码 | `RefNo::dbno()`（`refno.rs:17`）；gen-model 只经 `data_interface::refno_route::refno_dbnum()` 取 dbnum（护栏 `tests/refno_native_guards.rs`） |
| `a1[1] < 0` / `dbno == 0` 非法 | `RefNo::is_valid()`；`refno_dbnum()` 回 `None` → `DirectStoreError::InvalidRefNo` |
| `sub_5AF58E0` 已开库有序表 + 错 41 | `DbSet::db_slot()` 池 + `DbFileResolver::resolve()`；`MdbRouteDirectory::route_dbno()` 未命中 → `DbNotInMdb` / `NoFileForDbnum`，不回退扫描 |
| `DB_MDB::internalOpenDB` 决定开哪些库 | `MdbRouteDirectory::from_membership()`（SYS/CURD 成员清单 + 文件头，零元素 I/O）+ `add_dependency_dir()`（依赖项目 CATA / PROP） |
| 打开后自报的库号 | `DirectStore::open_session` 核 `descriptor().db_mark == pin.dbnum` → `DbNumberMismatch`（不变量 3）；探针 `DbnumLedger` 在 ams8000 上 1313/1313 相等 |
| `sub_5AEB6B0:181–183` 目标库 B+tree 点查 | `ReadOnlyEngine::search_index()` / `DbSet::element_exists()`；`model_source::dbnum_of_root` 只在解出的那一个库点查一次（`element_exists(pin, root)`：开库一次 + 核 `db_mark` + 索引点查一次，`db49238e3`） |
| core 没有 ref0 → dbnum 表 | 已退役：`model_source::DBNUM_BY_REF0`（`425ca98d8`）、属性 / 树服务的 `locator_parts()` 骨架定位器（同一提交）、探针的 `InMemoryCataLocator::build_for_project`（`7f501ab0d`）。`MdbMemoryStore.affiliations` 本体留给母计划阶段 6 删 |

## 5. 重跑

```powershell
ida-bridge list                                     # 需要 idalib-33060（3.1）与 idalib-19528（2.1）在线
ida-bridge exec idalib-33060 --sql "SELECT decompile(0x5B05260) AS dbno_worker, decompile(0x5aade70) AS db_get_dbno_from_reference"
ida-bridge exec idalib-33060 --sql "SELECT n, line FROM pseudocode WHERE func_ea = 0x5AEB6B0 AND ((n BETWEEN 52 AND 60) OR (n BETWEEN 176 AND 186)) AND line != '' ORDER BY n"
ida-bridge exec idalib-33060 --sql "SELECT decompile(0x5AF58E0) AS table_lookup, decompile(0x5AF5820) AS find_open_db, decompile(0x5902e80) AS findDB"
ida-bridge exec idalib-19528 --sql "SELECT n, line FROM pseudocode WHERE func_ea = 0x1064EF60 AND n BETWEEN 20 AND 24 ORDER BY n"
```

全部只读；`decompile()` 有缓存，重跑不重算。
