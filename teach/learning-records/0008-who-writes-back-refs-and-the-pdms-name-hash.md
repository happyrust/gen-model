# 0008 — 谁真正写 back-ref？兼：PDMS 名字 hash 函数与连接配对表（实测）

- **日期**：2026-07-26
- **背景**：`0007` 留下的未决第二条——「哪些 handler 订阅了 `DB_ElementChangesPlugger::PostSetRefListAttribute`」。
- **工具**：`ida-pro-mcp`。会话 `core31-retrace`（core.dll，imagebase `0x5170000`）、
  `core3d-retrace`（Core3D.dll，imagebase `0x10000000`）、`afimodel-retrace`（AfiModeling.dll，本次新开）。
- **课件**：`teach/lessons/0002-ref-rev-reverse-reference-index.html` §9

## 1. 订阅者全部找齐了，一共只有三个模块

`?SubscribePostSetRefListAttribute@DB_ElementChangesPlugger@@QAEXPAV...@Z` @ `0x581F7E0`
在 **core.dll 内部没有任何调用方**——它唯一的 xref 是 `0x5E14028`，即导出表数据项
（IDA 标为 Exported entry 12076）。所以订阅动作全部来自外部模块。

对 `D:\AVEVA\Everything3D3.1` 下 621 个顶层 `*.dll` / `*.exe` 做导入名扫描
（`findstr /M /C:"SubscribePostSetRefListAttribute"`），命中除 core.dll 自身及其副本外只有三个：

| 模块 | 类型 | 订阅者 | 干什么 |
|---|---|---|---|
| `Core3D.dll` | native x86，15 MB | `DESDRA_SCPlugs`（单例） | **维护连接型 back-ref**（见 §2） |
| `AfiModeling.dll` | native x86，3 MB | `ElementEvents` + `AfiDabLegalityCheck*` | 把 DB 变更转发成应用层事件 + 合法性校验，**不写 back-ref** |
| `Aveva.Core.Database.Implementation.dll` | 混合模式 C++/CLI，1.2 MB | 未分析 | 推测是 .NET 事件桥 |

- `DESDRA_SCPlugs`：ctor `0x10407740`、dtor `0x10407A80`、`Init` `0x10409160`（订阅点）、
  `Instance` `0x10409440`。它把 Pre/Post Create/Delete/Include/Reorder/SetAttribute/SetName/
  SetRef/SetRefList 以及全套 `*Allowed` 校验接口一次实现完。
- `AfiModeling.dll` 的 `sub_100C4AB0` 一次性 new 出 `ElementEvents`（0xA0 字节、14 个 handler 基类子对象）
  并逐个 Subscribe；ref-list 那一路挂在 `dword_11307450 + 152`。它的 vftable 全是 `ElementEvents`，
  与 back-ref 无关。

## 2. Core3D 这条链的全貌（都是实测）

```
DB_ElementChangesPlugger::PostSetRefListAttribute      core.dll  @0x591E780   广播
  └─ DESDRA_SCPlugs::PostSetRefListAttribute           Core3D    @0x10409C50  打包
       把 (元素 refno, noun hash, 属性 hash, 新目标 refno 数组, 个数) 交给 —
     └─ VDESFA（trace 串 "descases/VDESFA"）           Core3D    @0x101F2D4B  差分
          DGETFA 读该属性的旧值 → 与新列表比对
            ├ 新增的目标 → BAKREF（"structures/BAKREF"） @0x102D4724   建立反向引用
            └ 移除的目标 → BREAKF（"structures/BREAKF"） @0x102D448F   断开反向引用
```

两处细节值得记：

1. **BAKREF 内部会先调 BREAKF** 再写新边（`sub_102D448F(_3AC, a2, a3, a5)`），
   与 gen-model `maintain_reverse_index` 的「先整体清、再整体重写」是同一条纪律。
2. BAKREF 用 `sub_10382A60`（trace 串 **`desdblib/LOOKUP`**）按 (noun, 属性) 查出**配对属性**——
   见 §3。`JOIS` / `JOIE` / `TREF`（hash `912394` / `636832` / `653690`）在 BAKREF 里另有特判分支。

## 3. `desdblib/LOOKUP`：646 行的连接配对表

静态表 `dword_10DA5EA0`，**646 行 × 4 列**（列间距 646 dword，共 10 336 字节），
`sub_10382A60` 线性扫描：`col0 == noun_a && (col1 == noun_b || noun_b == 0) && col2 == attr` → 产出 `col3`。

- `col0` / `col1`：**noun 对**，92 / 95 个不同值（BRAN、NOZZ、ELBO、TEE、FLAN、GENSEC、HANG、AHU、DAMP…）
- `col2` / `col3`：**属性对**，只有 11 个不同值 ——
  `CREF` `HREF` `TREF` `CRFA` `JOIS` `JOIE` `PFRE` `CFRA` `LNFA` `HPREF` `TPREF`

表是**对称**的（既有 `TREF→CRFA` 也有 `CRFA→TREF`）。整表已解码落盘：
`.ida_scratch/out_conn_pairing_table.md`（646 行）。例：`BRAN | TEE | HREF | CREF`
＝「BRAN 的 HREF 指向 TEE 时，配对的反向属性是 TEE 的 CREF」。

**所以这条链维护的是「连接型 back-ref」，不是目录/规格的 `SPBREF` 家族。** 这一点更正了 `0007` §3
配图里「handler A：维护 back-ref（写 SPBREF / SCBREF）」的猜测性标注。

## 4. 顺手反解出 PDMS 的名字 hash 函数（可离线复用）

从本仓 `all_attr_info.json` 的 701 个 (name, hash) 对拟合：先用「只差一个字符的名字对」
求出每个位置的权重恒为 `27^i`（**首字符权重最低**），再按名字长度解常数项，得到闭式：

```
hash(name) = 27^4 + Σ_{i=0..n-1} val(c_i) · 27^i        val('A')=1 … val('Z')=26
```

- 在 701 个属性名上 **100% 命中**（长度 2/3/4/5/6 全对）。
- 对 noun 同样成立：`TABITE` → `83 083 448`，与 `noun_flags.json` 一致。
- 用途：gen-model 以后**不需要字典就能算任意 noun / 属性的 hash**，也能把逆向里遇到的裸
  hash 立刻反解成名字（本次 646 行表里 8 个字典查不到的 noun 就是这么解出来的）。
- 脚本：`.ida_scratch/attr_hash_solve.py`（拟合+验证+反解）、`.ida_scratch/decode_lookup_table.py`（解表）。

## 5. 对 gen-model 的启示

- **A2 兜底被这次逆向验证了**。E3D 认定为双向连接的 11 个属性里，gen-model 的
  `DEPENDENCY_CASCADE_ATTR_NAMES` 只显式列了 `CREF`/`HREF`/`TREF`；另外 8 个
  （`CRFA` `JOIS` `JOIE` `PFRE` `CFRA` `LNFA` `HPREF` `TPREF`）都不在任何静态清单里、
  也不被 `attribute_affects_model` 命中 → 落 `Unknown` → 被
  `classify_attribute_effect_with_meta` 的 A2（`att_type == ELEMENT`）升级为 `DependencyCascade`。
  **`ref_rev` 的成员资格没有缺口**，但这 8 个名字是靠兜底进来的，不是靠 curated 清单。
- 这 11 个属性**全都在离线 `all_attr_info.json` 里**（646 行表的 col2/col3 全部由字典解出），
  与 `SPBREF` 家族的「离线零命中」形成对照：**连接型 back-ref 离线可读，目录型 back-ref 离线不可读**。
- `model_impact.rs` 注释里已有的「CTYP/JFRE 系 Core3D `VDESPT` (noun,attr) 特例」是同一族发现，
  `VDESFA` / `desdblib/LOOKUP` 是它的邻居，可一并对照。

## 6. 仍未决

- **谁写 `SPBREF`/`SCBREF`/`TABREF`/`GOBREF`/`HDBREF`/`DBREF`**：不是本次找到的两个订阅者。
  尚未分析 `Aveva.Core.Database.Implementation.dll`；也可能根本不走 plugger，而是由 dabacon 内核
  在写引用属性时自己维护（这与 ADR-003「PDMS 里 back-ref 是独立引用表 / 系统维护结构」的判断一致）。
- **VDESFA 顶部跳过的三个属性 hash**：`18424546` / `18432565` / `18427258`
  （`cmp eax, 11922E2h / 1194235h / 1192D7Ah` @ `0x101F2D9B` / `0x101F2DAB` / 第三处）。
  全binary 搜索显示它们**只在 VDESFA 里出现**；按 §4 的函数反解为 `PSARFA` / `PSLRFA` / `ALERFA`，
  三者都不在本仓字典里。考虑到表里确实存在 `CRFA` / `CFRA` / `LNFA` 这类「…FA」后缀族，
  这三个名字可能是真实但未收录的属性，**目前不下结论**。
- **back-ref 属性的 `wnoevt` 是否为 true**：仍是推断（`0007` 未决第一条），值在字典里。
  但注意 VDESFA 用的是**显式 hash 排除**来防递归，说明 E3D 并不只依赖 `wnoevt` 这一道闸。
