# ADR-008：从活 E3D 导出 noun 有序属性表，并确定隐含块寻址模型

状态：已接受（寻址模型）／进行中（占槽判据）
日期：2026-07-26
关联：ADR-004（db noun 分类数据）；`vendor/aios-parse-pdms/src/parse.rs`；`scripts/e3d/`；`.ida_scratch/probes/`

## 背景

gen-model 解析元素二进制依赖 `aios_core` 内嵌的 `all_attr_info.json` 快照，只覆盖 **339 个 noun / 700 个属性**，且只存 offset 不存长度——解析时靠「相邻 offset 相减」推步长，再叠 `is_f32` 启发式。上一轮已确认：属性的类型与字长能离线从 `attlib.dat` 读出，但**每个 noun 的有序属性表不在 attlib.dat 里**，core.dll 自己也是向 dabacon 运行期按 `(dabType, nounHash)` 要（opcode 536）。

两代会话尝试从活 E3D 导出这张表均告失败，且无任何错误输出。

## 决策

### 一、导出链路（`scripts/e3d/`）

写一个同时实现 PML.NET 可调用类与 CAF `IAddin` 的 x86 插件 `GenModelNounLayout.dll`，用 `DbElementType.GetAllElementTypes()` + `SystemAttributes()` 导出全部 noun 的有序属性表。**能跑通需要两个条件同时成立**，缺一不可：

1. **用 `DETACHED_PROCESS` 启动 `des.exe`**。它是 GUI 子系统程序，从 cmd 启动会继承父控制台，core.dll 因此走不完整初始化路径，会话里既没有 CAF、也没有命令循环。
2. **用 C: 那套安装**。两套安装所有程序集逐字节同大小，**唯独 `Startup.dll` 不同**（C=774656 / D=770048），D: 那份被历史会话改过，appware 起不来。

前两代会话一直在设的 `AVEVA_DESIGN_ENTRYMACRO` 与 `AVEVA_DESIGN_CONSOLE_WINDOW`，**字面量在 `des.exe` / `mon.exe` / `core.dll` 里一次都没出现**（前者只在 `Startup.dll`，而它从不被加载）。core.dll 真正读的是 `PDMS_SHOWCONSOLE` / `PDMS_HIDECONSOLE` / `PDMS_GRAPHICS` / `PDMS_NOGRAPHICS` / `PDMS_ACTIVATE` / `PDMS_CONSOLE_IDENTIFIER`。这是两代会话「静默失败」的根因。

一条命令 `run_export_ams_c.bat`，14 秒无人值守跑完。

### 二、隐含块寻址模型

```
offset = (bitIndex << 20) | wordOffset        <- 正是解析器现在吃的编码
  wordOffset : 从第 11 个字起，按 SystemAttributes() 顺序累加；
               该 noun 的所有 BOOL 塌缩进同一个位打包字，
               字的位置由第一个 BOOL 出现处决定；
               数组 1 + maxSize * unit，标量 unit
               (INTEGER/WORD=1，DOUBLE/ELEMENT/DIRECTION/POSITION/ORIENTATION=2)
  bitIndex   : 非 BOOL 恒 0；BOOL 取其在 BOOL 游内的位次
```

BOOL 塌缩是本轮关键发现。它同时解释了为什么「导出顺序与快照 offset 顺序不一致」是个伪问题——同一个打包字内部本就没有顺序。

## 验证

- **字偏移**：从第 11 字逐个累加重建（非相邻相减，错一个后面全崩），**212 / 250 个 noun 每个 offset 全对**，单属性 90.3%。
- **bit 下标**：`offset` 高位只在 BOOL 上非零（取值 0–6，与实测「最多 7 个 BOOL 共一个字」精确吻合）；BOOL 的 bit 下标等于其在导出顺序中的位次，**128 对 / 1 错 = 99.2%**（唯一失败是 `BOOL[]`）。
- **独立二进制佐证**：numpy 扫 `ams000` 下 441 个库文件（0.5 GB），得 615 个 noun 的实测 `impl_len`，384 个有高置信众数。与快照对拍 13/23 精确、其余恰好差 +2——差值来自快照漏了末尾属性，`CYLI` 可精确解释（算到 `HEIG` 结束是 33，导出列表里其后正是 `ORRF`(ELEMENT, 2 字)，33+2=35 = 实测值）。

## 结果 / 约束

产物：`output/noun_layout.json`（1935 noun / 22095 属性）、`output/noun_attr_fields.json`（4271 属性 x 57 个 `DbAttributeField`）、`output/impl_len_observed.json`（615 noun 实测长度）。

**未解**：哪些属性占隐含块的槽位。已系统排除全部字典侧信息——57 个属性级字段（最佳 `DCHC` 80.7%；三字段组合 96.6% 但那是 530 个格子记忆 5472 个样本的过拟合）、`HiddenByType` / `NoClaim`（53% / 55%）、`TrueSizE`（实测等于 `maxSize/4`，标量恒 0，无信息量）、`GetDefault`（58.2%）。**占槽是按 (noun, attr) 对存在 dabacon 元素记录 schema 里的，属性字典不携带它。**

已实现的离线替代：用实测 `impl_len = 11 + 占槽字长之和` 作约束反解占槽子集，209 个 noun 有唯一解，但与快照交叉验证仅 **66% 完全一致**（另 15% 是快照漏标的超集，15 个真实分歧，`PURP` / `GRADE` 这类反复背叛的属性是主因）。**因此该反解结果尚不可直接用于生产解析**——布局错一个字，后面整块全错。

寻址模型与占槽判据成熟度差异很大，建议分开推进：前者可先在已知占槽的 339 个 noun 上接入并加回归测试；后者需继续打磨反解，或去逆向 core.dll 的隐含块构造逻辑取权威答案。

## 附：占槽判据已试过的四条路与实测数字

写在这里是为了下一轮不要重做。四种方法**都卡在 ⅓ 左右的残差上**：

| 方法 | 结果 |
|---|---|
| 属性级字典字段（57 个）| 最佳 `DCHC` 80.7%；三字段组合 96.6% 但是 530 格记 5472 样本的过拟合 |
| 前缀 − 学习黑名单（116 名）| 66.8% |
| 实测 `impl_len` 子集和反解 | 209 noun 有唯一解，与快照交叉验证 66% |
| 双重约束修复（`impl_len` + 每个已知 offset 必须对上）| 161 中只有 56 自洽 |

两个值得记住的观察：

1. **偏差很小但不在末尾**。拿快照占槽集算总长与实测 `impl_len` 对拍，161 个 noun 里 158 个偏差 ≤ +3 且几乎无负值（分布 `{0:63, +1:27, +2:21, +3:47}`）——说明**字长规则是对的**，否则偏差会又大又乱。但进一步要求中间每个 offset 也对得上时只剩 56/161，说明**快照漏标的属性不只在末尾，中间也有洞**。
2. **90.3% 是地板不是天花板**。把 38 个不一致定位到第一个分歧属性，它们没有一致的类型特征（DOUBLE 12 / ELEMENT 7 / WORD 6 / INTEGER 3…）且偏差双向——“少算”是跳过了实际占位的属性，“多算”多半是 f32。也就是说模型真实准确率可能高于 90.3%，只是被一把不准的尺子（快照）量着。

结论：再换启发式意义不大，信息真的不在手里。要拿权威答案得去逆向 core.dll 的隐含块构造逻辑（入口线索：`DB_Noun::isAttValid` 里的 `sub_5AADD30(dabType, noun, attr, fieldId, out)` 是按 (noun, attr) 对取字段的通用入口，`isAttValid` 用的 fieldId 是 3522340）。

本轮验证与分析探针全在 `.ida_scratch/probes/`，命名自述，均可重跑。

### 补：f32 不是布局因素，残差只剩占槽一个

曾怀疑部分 noun 把 DOUBLE 存成 f32（解析器里 `is_f32` 启发式就是在猜这个）。拿实测 `impl_len` 做二元判定：每个 noun 分别按 f64（DOUBLE=2 字、POSITION/ORIENTATION/DIRECTION=7 字）和 f32（=1 字 / 4 字）各算一遍总长。结果：**63 个只合 f64，0 个只合 f32**，剩下 98 个两者都不合（即占槽集本身就不对）。

所以**隐含块里 DOUBLE 始终按 f64 占位**，`is_f32` 处理的是固定槽内的值编码而非槽宽。之前看到的“SCTN 的 BANG 只占 1 字”是拿快照相邻 offset 相减得来的，而那个差值本身已被排序/占槽问题污染，不能当证据。

结论：**残差不是“f32 + 占槽”两个问题，就是占槽一个。**

### 补：真库值级对拍的结果

`src/bin/noun_layout_parse_probe.rs`：用重建的偏移表解析 `ams251181_0001` 的 5888 个元素，与快照解析结果逐属性比值：**77709 / 97238 = 79.92% 一致**。

offset 算对 90.3% ≠ 值读对 79.9%——中间隔着表达式属性、BOOL 取位等分支。典型失败形态：`SPRE` 快照读到 `(15202, 1722)` 而重建表读到 `(1722, 1)`，**恰好偏一个字**；POS/ORI 随之出 NaN。

**判定：重建的偏移表现阶段不能替掉 `all_attr_info.json`。** 导出数据与寻址模型可以归档备用，但接入生产必须先解决占槽判据。

### 补：core.dll 根本不算字偏移，它按属性 key 问 dabacon

把读属性的链路逆完了（IDA session `core31-retrace`，即 `D:\AVEVA\Everything3D3.1\core.dll`）。入口是 `DB_Element::dabGetAtt` 一家（`0x592c920` / `0x592cab0` / `0x592cc40` / `0x592ce90` / `0x592d060` / `0x592d790` / `0x592dbc0`，按返回类型重载），核心就三行：

```c
v9 = DB_Noun::hashValue(该元素的 noun);
if ( sub_5AAB330(v9, attrKey, 810973, &size) )      // opcode 484：按 (noun, attr) 对要长度
    goto READ;
db_clear_error();
if ( !DB_Attribute::findAttribute(attrKey, &a) ) return false;
size = DB_Attribute::size(a);                       // 兑不到就退回字典声明长度
READ:
sub_5AAB4D0(attrKey, outBuf, &size);                // 真正取值
```

而 `sub_5AAB4D0` 只是个 trace 包装，里子是 **`sub_5AC78A0(492, attrKey, outBuf, sizePtr)`——dabacon opcode 492**。

**关键结论：core.dll 全程没有计算任何字偏移，它只传属性 key 和一个长度。** 隐含块的定位发生在 dabacon 引擎内部。这反过来解释了为什么属性字典里找不到占槽判据（本 ADR 前面排除了五条途径）：**那个信息不在字典里，在引擎对元素记录的处理里**。

三个已知 opcode：

| opcode | 包装函数 | 作用 |
|---|---|---|
| 484 | `sub_5AAB330(noun, attr, fieldId, out)` | 按 (noun, attr) 对取字段；`trueLength` 用 fieldId=810973 |
| **492** | `sub_5AAB4D0(attrKey, outBuf, sizePtr)` | **从当前元素按 key 读属性值——下一个逆向目标** |
| 536 | `sub_5AADCD0(dabType, nounHash, keys, …)` | 取有序属性表（`getSystemAttributes` 用）|

另外两个值得看的函数：`DB_Element::isDabArrayType`（`0x59435f0`）和 `DB_Element::getActualUdaLength`（`0x5932af0`），都走 opcode 484。

> 提醒：IDA MCP 同时开着多个 worker（`idalib_list` 可查）。core.dll 在 `core31-retrace` 里，**每个调用要显式带 `"database": "core31-retrace"`**，否则会打到当前上下文绑定的那个库上（本轮就因此白跑了几次查询）。

### 补：找到占槽判据了——它在 dabacon 属性描述符的低 20 位

顺着 opcode 492 一路挖到底：

```
DB_Element::dabGetAtt
  → sub_5AAB4D0(attrKey, buf, &len)          // trace 包装
  → sub_5AC78A0(492, attrKey, buf, &len)     // trace + opcode 名
  → sub_5AB5BC0(attrKey, 7, len, buf, &len, 0)   ← 真正干活的
```

`sub_5AB5BC0`（`0x5AB5BC0`）里的关键分支：

```c
sub_5AB5920(0, attrKey, &desc);   // 取 dabacon 属性描述符，v7 = desc
...
v16 = (unsigned)(attrKey - 531442) > 0x17179147
   || (*(_DWORD *)(((flag & 0x20000000) != 0 ? 32 : 20) + v7) & 0xFFFFF) == 0;
v17 = sub_5AB5600;                // 隐含块读取
if (v16) v17 = sub_5AB4CF0;       // 显式块读取
```

**描述符偏移 20（或 32，由一个来自每-DB-类型结构的 `0x20000000` 标志位选择）处那个 DWORD，它的低 20 位就是隐含块字偏移；为 0 则该属性不在隐含块里，走显式路径。**

两件事值得注意：

1. `& 0xFFFFF` 这个掩码与 gen-model 的 `AttrInfo.offset` 编码**完全一致**——也就是说快照里那个字段本来就是从这个描述符拉出来的，“offset==0 即显式属性”这个惯例在这里得到了二进制层面的确认。
2. 所以占槽判据**不是推不出来，而是存在 dabacon 的属性描述符里**。要离线拿到它，下一步是逆 `sub_5AB5920`（`0x5AB5920`），看描述符从哪个表/文件建起来。

旁证：`DB_Element::isDabArrayType`（`0x59435f0`）与 `getActualUdaLength`（`0x5932af0`）都用 fieldId **810973** 取长度，且后者对 type 7/8/9 会 `return v16 / 3`——说明 810973 返回的是**元素个数**（POSITION 计 3）而非字数。`isDabArrayType` 还露出数组性的判定用到 `dtyp`（DTYP）/ `isTable`（TABLE）/ `tkeyln`（TKEYLN）三个字典字段。

### 补：描述符表的内存结构（`sub_5AB5920`）

`sub_5AB5920(0, attrKey, &desc)`（`0x5AB5920`）就是按属性 key 在**当前元素类型的属性表**里做线性查找：

```c
v3 = *(_DWORD *)(dword_6A54024 + 60 * v4 + 16);   // 从 dabacon 当前 DB 栈取出该类型的属性表
v6 = 0; v7 = 0;
while (1) {
    v8 = (_DWORD *)(v3 + 4 * (v6 + 14));          // 条目起点：表 + 56 字节
    if (a2 == *v8) break;                          // 条目[0] == 属性 key
    v6 += *(_DWORD *)(v3 + 4 * v6 + 60);           // 步进 = 条目[1]（自身长度，单位 dword）
    if (++v7 >= *(_DWORD *)(v3 + 36)) goto NOTFOUND;  // 表 + 36 = 条目个数
}
*a3 = v8;                                          // 描述符就是指向该条目的指针
```

结合 `sub_5AB5BC0` 里的判定，条目布局至少是：

| 字节偏移 | 内容 |
|---|---|
| +0 | 属性 key（hash）|
| +4 | 本条目长度（dword 数，用作步进）|
| +8 | 一个索引，用于 `qword_6453DF8[…]` 取 double（单位换算？）|
| +12 | 一个可被覆盖的长度/精度值 |
| **+20（或 +32）** | **低 20 位 = 隐含块字偏移；为 0 则该属性在显式块** |

+20 还是 +32 由一个 `0x20000000` 标志位选择，该标志来自每-DB-类型的结构（`*(v15+4) + 4*(*(v15+48)) + 40`）。

**这张表就是我们一直在反推的东西的权威版本。** 它是运行期建在内存里的，下一步要找的是“谁填的它”——即向 `dword_6A54024 + 60*n + 16` 指向的区域写入的路径，那里会告诉我们偏移到底是从哪个文件/表读出来的。一旦知道，就能离线拿到全部 1935 个 noun 的权威偏移表，不用再反推。

### 下一个线头（本轮逆向停在这里）

找“谁填的属性表”时，`dword_6A54024`（dabacon 当前 DB 栈）的引用里绝大多数是 opcode trace 包装（每个 4 次），真正管上下文的是 `sub_5AEB6B0`（15 次）和 `sub_5AECBE0`（7 次）。

`sub_5AEB6B0`（370 行）里有一行值得接着追：

```c
sub_5AFC950(v27, 7618377, v46 + 36)
```

`v46 + 36` 正好是属性表里“条目个数”那个字段的位置，而 **7618377** 看着像个记录/字段 key。从 `sub_5AFC950` 接着挖，应该能到“偏移数据从哪个文件读出来”。

另外 `sub_5AEB6B0` 里出现的错误字符串 `4SCEFER:Reference is =%d/%d` 与 `INTERNAL CONSISTENCY ERROR:%s` 可作为定位锚点。

【操作提醒】IDA MCP 里 core.dll 在 session **`core31-retrace`**，每个调用必须带 `"database": "core31-retrace"`；否则会静默打到当前绑定的其它库上（本轮因此白跑了几次查询并得出过错误结论）。`idalib_list` 可查看所有 worker。

#### 修正：`sub_5AFC950` 是红鲦鱼

跟进发现 `sub_5AFC950`（`0x5AFC950`，98 行）**不是**属性表加载器，而是个通用的 per-DB 表登记 setter：按 key 往记录里写两个 dword（key=13387743 → +12/+16，key=7618377 → +20/+24），找不到 key 时报 `Table name is %s`。与隐含块布局无关。

剩下两条可行路径：

1. **继续逆向**：把 `sub_5AB5920` 里的 `v3`（即 `dword_6A54024 + 60*n + 16` 指向的类型记录）回溯到写入方。难点是它是结构体字段而非全局符号，`xrefs` 使不上力。
2. **直接从活进程 dump**（看起来更划算）：结构已完全明确，`dword_6A54024` 是静态地址，用 `run_export_ams_c.bat` 拉起 des.exe 后用 `ReadProcessMemory` 走一遍就能把表读出来。需注意该表是**当前 DB 上下文**的，要覆盖全部 noun 得想办法逐个切类型（或找到持有全部类型表的上层容器）。

#### 地址与 RVA（供下一步直接用）

IDA 里 core.dll 的 imagebase 是 **`0x5170000`**（`idb_path: D:\AVEVA\Everything3D3.1\core.dll.i64`，session `core31-retrace`）。进程里实际地址 = `GetModuleHandle("core.dll")` + RVA：

| 符号 | IDA 地址 | RVA | 作用 |
|---|---|---|---|
| `dword_6A54024` | `0x06A54024` | **`0x18E4024`** | dabacon 当前 DB 栈（入口）|
| `sub_5AB5920` | `0x05AB5920` | `0x945920` | 按 key 查属性描述符 |
| `sub_5AB5BC0` | `0x05AB5BC0` | `0x945BC0` | 分派隐含/显式读取 |
| `sub_5AB5600` | `0x05AB5600` | `0x945600` | 隐含块读取 |
| `sub_5AB4CF0` | `0x05AB4CF0` | `0x944CF0` | 显式块读取 |

推荐做法：**在我们自己的 `GenModelNounLayout` 插件里读**（它本来就跑在 des.exe 进程内，不需要 `ReadProcessMemory`，也不会撞权限）：遍历元素、每遇到一个新类型就把当前属性表 dump 一份。因为表是「当前元素类型」的，靠遍历真实元素恰好能覆盖到实际会出现的那些 noun（实测 `impl_len` 时扫到 615 个），而那正是解析真正需要覆盖的集合。

### 补：`sub_5AB5600` 把整套寻址模型在二进制层面完全确认了

隐含块读取器 `sub_5AB5600`（`0x5AB5600`，130 行）的核心：

```c
v9  = (unsigned __int16 *)(*(_DWORD *)(a1 + 4) + 4 * *(_DWORD *)(a1 + 48));  // 元素记录
v26 = *((_DWORD *)v9 + 10);                        // 记录第 10 个字 = 标志位
v10 = (v26 & 0x20000000) != 0 ? 0xC : 0;           // 选描述符 +20 还是 +32
v11 = *(_DWORD *)((char *)a3 + v10 + 20) & 0xFFFFF;   // 字偏移
if ( v11 >= *v9 ) 报错 "attribute offset is %d";      // 越界检查：记录长度在 word0

// BOOL（a3[2] == 5）：
*(_DWORD *)a7 = (*(int *)&v9[2 * v11] >> (*(int *)((char *)a3 + v10 + 20) >> 20)) & 1;

// DOUBLE / WORD 且标志位未置位：
if ( (v26 & 0x20000000) == 0 && (a4 == 2 || a4 == 6) )
    *(double *)a7 = *(float *)&v9[2 * v11];        // 按 f32 读！
else
    memcpy(a7, &v9[2 * v11], 4 * v15);
```

**三条结论都得到了二进制级别的确认：**

1. **字偏移 = 低 20 位，bit 下标 = `>> 20`**。`v9[2*v11]` 按 u16 索引（=4*v11 字节），确认 v11 是字下标；BOOL 读作 `(word >> (offset>>20)) & 1`。这与 gen-model 解析器里的 `let o = (attr_info.offset >> 0x14); r >> o & 1` **逐位一致**（0x14 = 20）。本 ADR 前面从数据反推出的寻址模型到此得到完全印证。
2. **f32 是按记录标志位决定的，不是按 noun**。`(recordFlags & 0x20000000) == 0 && (type == 2 || type == 6)` 时，值以 **float 存在一个字里**。这印证了前面“f32 不是布局因素”的结论（按 noun 分类当然全失败），并给出了真正的判据：**元素记录第 10 个字的 `0x20000000` 位**。
3. **同一个标志位还选择描述符的哪个字段存偏移（+20 或 +32）**——也就是说两种记录形态（紧凑/f32 与 宽/f64）**各有一套偏移表**。这又解释了为什么拿单一快照去套所有库会有系统性偏差。

另外 word0 是记录长度（越界检查用它）、word10 是标志字，与“隐含块从 word11 开始”完全对得上。
