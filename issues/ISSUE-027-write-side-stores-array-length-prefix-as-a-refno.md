# Issue #027: 写库侧把「引用数组的长度前缀」当成 refno 存了进去，`CRFA` 全库写成 `pe:3_0`

## 📋 Issue 信息

- **Issue ID**: #027
- **类型**: Bug 🐛（**静默写入一个语法合法、指向不存在元素的引用**，不报错）
- **优先级**: Medium 🟡 —— 见「影响范围」：`CRFA` today 是模型中立键，生成链不消费它，
  所以**现在没有可见损害**；但缺陷在**通用的显式属性解码器**里，同一条路径上换个
  模型相关的属性就是错值。定级按「机制通用 + 当前无损」取中。
- **状态**: Open 📝
- **创建日期**: 2026-08-30
- **发现于**: ADR-053 direct/DB 全量对拍（`src/bin/direct_attmap_probe.rs`，dbnum 7333）
- **归属仓**: `D:\work\plant-code\old\vendor\old-parse-pdms-db`（**不在 gen-model**，
  登记在这里的理由同 #026：该仓没有 `issues/`，而这条是 gen-model 对拍门上的挂账）
- **相关模块**: `src/parse.rs::parse_raw_explicit_attrs`（`ElementType` 臂 1932-1936）

## 🔍 问题描述

显式属性的解码按 **schema 声明的形状**（`attr_info.default_val`）分派。
`CRFA` 在吊架件家族的 noun 上被声明成**标量** `ElementType`，
但文件里它实际存的是**引用数组**，布局是 `[u32 长度][u64 引用] × 长度`。

标量那一臂直接把载荷开头的 8 字节读成 `(word0, word1)`，
**没有跳过长度前缀**。于是：

```
文件载荷： [3][0 0][0 0][23717 114144]      ← 3 槽数组，前两槽空，真引用在第 3 槽
标量臂读： (word0=3, word1=0)               ← 读的是「长度」和「空槽 0 的高位字」
写进库：   pe:3_0
```

`pe:3_0` 不是引用，是**数字 3 和一个空槽拼出来的**。

### 复现步骤

**① 库侧：看这个属性只有三种取值**

```powershell
cd d:\work\plant-code\old\gen-model
.\scripts\Invoke-Surreal8009.ps1 -Sql "SELECT CRFA, count() FROM PCLA GROUP BY CRFA;"
.\scripts\Invoke-Surreal8009.ps1 -Sql "SELECT CRFA, count() FROM HELE GROUP BY CRFA;"
.\scripts\Invoke-Surreal8009.ps1 -Sql "SELECT CRFA, count() FROM PCOM GROUP BY CRFA;"
```

实测（2026-08-30，8009 上 `ams-rvm-rebuild-20260824`）：

| 表 | `null` | `pe:3_0` | `pe:4_0` | 其它取值 |
|---|---:|---:|---:|---:|
| `PCLA` | 7694 | 2788 | 2 | **0** |
| `HELE` | 273 | 1132 | 0 | **0** |
| `PCOM` | 7 | 0 | 2 | **0** |
| 合计 | | **3920** | **4** | **0** |

**三张表、3924 行非空值，只有两种取值。** 真引用集合不可能长这样。

**② 库侧：这两个 id 根本不存在**

```powershell
.\scripts\Invoke-Surreal8009.ps1 -Sql "SELECT count() FROM pe WHERE id = type::thing('pe','3_0') GROUP ALL;"
.\scripts\Invoke-Surreal8009.ps1 -Sql "SELECT count() FROM pe WHERE id = type::thing('pe','4_0') GROUP ALL;"
```

两条都返回 `"result":[]`（即 0 行）。**库里存着指向不存在元素的引用。**

**③ 文件侧：同一个元素，直读拿到的是什么**

```powershell
cd d:\work\plant-code\old\gen-model
$env:CARGO_TARGET_DIR="D:\Rust\target"
cargo run --release --bin direct_attmap_probe -- `
    --dbnum 7333 --refnos 23717_114143,23717_62660 --dump-keys CRFA
```

```
[dump] 23717_114143 CRFA type=PCLA storage=8 status=DecodedExplicit
       raw=Some(RefNoArray([RefNo(0/0), RefNo(0/0), RefNo(23717/114144)]))
       db=Some(RefU64Type(3_0))
[dump] 23717_62660  CRFA type=PCLA storage=8 status=DecodedExplicit
       raw=Some(RefNoArray([RefNo(0/0), RefNo(0/0), RefNo(23717/62648)]))
       db=Some(RefU64Type(3_0))
```

**两个元素的真引用各不相同（`114144` / `62648`），库里却都是 `3_0`。**

### 预期行为

`PCLA:23717_114143.CRFA` 存 `pe:23717_114144`（或数组 `[pe:23717_114144]`）。

### 实际行为

存 `pe:3_0` —— 长度前缀 3，加上空槽 0 的高位字 0。

## 🔬 问题分析

### 根本原因

`src/parse.rs::parse_raw_explicit_attrs`。该函数先读 8 字节头
`(explict_hash: i32, attr_type_num: u16, type_len: u16)`，取载荷
`tmp_input = &l[..type_len * 4]`（`type_len` 是**载荷字数**），
然后 **按 schema 声明** `attr_info.default_val` 分派：

```rust
// src/parse.rs:1932-1936 —— 标量声明：从载荷开头直接读两个字
ElementType(_) => {
    let (_, (ref_0, ref_1)) = tuple((be_u32, be_u32))(tmp_input)?;
    let refno = RefU64::from_two_nums(ref_0, ref_1);
    att_value = Some(RefU64Type(refno));
}

// src/parse.rs:1955-1966 —— 数组声明：先吃掉长度前缀，再逐个读
RefU64Array(_) => {
    let (tmp_input, len) = be_u32(tmp_input)?;      // ← 长度前缀在这里被跳过
    ...
}
```

两臂对**同一段载荷**的布局理解不同：数组臂知道开头有长度前缀，标量臂不知道。
`CRFA` 在这些 noun 上 schema 声明是标量、文件实存是数组，于是走进标量臂，
把长度前缀当成了 `word0`。

`RefU64::from_two_nums(n, m) = (n << 32) + m`（`vendor/old-aios-core/src/types/refno.rs:421`），
显示成 `n_m`。所以 `from_two_nums(3, 0)` 就是 `pe:3_0`。

**为什么恒是 `_0`**：`ref_1` 读的是长度前缀之后的 4 字节 = 槽 0 的高位字。
本语料里这些元素的槽 0 恒为空，所以恒是 0。
这也解释了上表「其它取值 0 个」——**若哪个元素的真引用落在槽 0，库里会出现
`pe:3_23717` 这种值**。一个都没有，反过来印证了「读到的是空槽」这个解释。

### 这不是 schema 声明错了就能开脱的

schema 声明与文件实存不一致（`CRFA` 在 24 个 noun 上声明数组、在 21 个吊架件 noun 上
声明标量）确实是上游的乱，但解码器**手上有足够信息发现自己读错了**：
`type_len` 就在眼前。标量引用的载荷应当是 **2 个字**；
3 槽数组是 `1 + 2×3 = 7` 个字，4 槽是 `9` 个字。
**`type_len != 2` 却按标量读，是解码器自己没有校验。**

### 影响范围

- **当前无可见损害，但这个「无损」判定本身是软的**：
  `attribute_affects_model("CRFA") == false`，所以对拍门放它过。
  **但要看清它为什么是 false**——`CRFA` 并没有被谁认定成 data-only，
  它只是**没被登记在任何清单里**，`classify_attribute_effect` 兜底返回 `Unknown`，
  而 `attribute_affects_model` 把 `Unknown` 和 `DataOnly` 一同判为「不影响模型」
  （`model_impact.rs:205-214`、`297-302`）。
  **更要紧的是**：同一模块的 `classify_attribute_effect_with_meta` 会把
  「未登记 + 引用类型（`att_type == ELEMENT`）」升级成 `DependencyCascade`，
  也就是**影响模型**。`CRFA` 恰恰就是引用类型——正因如此它才被声明成 `ElementType`。
  换句话说，**按名字判它中立，按类型判它不中立**；对拍门用的是前者。
  所以「无损」的结论只在「生成链今天确实不读 `CRFA`」这个前提下成立，
  不能当成「这批垃圾值永远无害」。见「后续行动」里的定性项。
- **机制是通用的**：出问题的不是 `CRFA` 这一个键，而是
  「标量引用臂不校验载荷长度」这条**所有显式引用属性都走的路径**。
  换一个模型相关的引用属性遇到同样的 schema/文件形状不一致，就是错值进模型。
- **对 direct 线的直接影响**：这 2984 处（dbnum 7333 全量）曾把
  `direct_attmap_probe` 的等价门顶红。查清是库错之后，探针已加
  `neutral_mismatches` 桶——**中立键的值分歧不计冲突，但逐键点名打印**，
  不是悄悄豁免。见 `src/bin/direct_attmap_probe.rs::diff_maps`。

### 相关代码

| 位置 | 角色 |
|---|---|
| `old-parse-pdms-db/src/parse.rs:1932-1936` | `ElementType` 臂，缺陷本体 |
| `old-parse-pdms-db/src/parse.rs:1955-1966` | `RefU64Array` 臂，正确处理长度前缀的对照 |
| `old-parse-pdms-db/src/parse.rs:1821-1831` | `type_len` 的来源与 schema 分派入口 |
| `old-aios-core/src/types/refno.rs:421` | `from_two_nums`，`3_0` 的拼装点 |
| `gen-model/src/data_interface/direct_attmap.rs` | direct 侧同一形状冲突的处理（投影臂 + `view_divergence`） |
| `gen-model/src/bin/direct_attmap_probe.rs` | 发现者；`neutral_mismatches` 桶 |

## 🛠️ 解决方案

### 方案概述

**标量引用臂必须先用 `type_len` 确认载荷真是一个标量引用**，
不是就别按标量读——要么按数组读，要么明确报「读不出」，
**不能拿前 8 字节硬凑一个引用出来**。

### 技术实现（建议，未提交）

```rust
ElementType(_) | RefU64Type(_) => {
    // 标量引用载荷恰是 2 个字。数组载荷是 [u32 长度][u64]×n = 1 + 2n 个字，
    // 直接按标量读会把长度前缀当成 word0（见 ISSUE-027）。
    if type_len == 2 {
        let (_, (ref_0, ref_1)) = tuple((be_u32, be_u32))(tmp_input)?;
        att_value = Some(RefU64Type(RefU64::from_two_nums(ref_0, ref_1)));
    } else if type_len >= 3 && (type_len - 1) % 2 == 0 {
        // schema 说标量、文件是数组：按文件的实际形状读，别猜。
        att_value = Some(parse_refu64_array(tmp_input)?);
    } else {
        att_value = None;   // 说不出所以然就别产出值
    }
}
```

配套：`RefU64Array` 臂里那段循环抽成 `parse_refu64_array`，两处共用。

### 下游怎么消费，需要单独拍板

改完之后这些行的值会从 `pe:3_0` 变成**数组**（多数只有 1 个实槽）。
读侧 schema 仍把 `CRFA` 声明成标量，**读出来仍是空**——
也就是说「库里存对了」不等于「生成侧看得见」。
direct 侧对同一情形的处理已经落地，可直接对齐：

- 恰 1 个实槽 → 投影成标量（`direct_attmap.rs` 的投影臂）；
- 0 个实槽 → `unset`；
- 多个实槽 → 交自然数组 + 记 `view_divergence`，**绝不挑一个**。

### 风险评估

- 改的是**所有显式引用属性**的公共路径，不止 `CRFA`。
  `type_len == 2` 是绝大多数属性的既有情形，走的还是原来那一臂，**行为不变**；
  变的只有此前会被静默读错的那些。
- 需要重灌库才能看到效果，成本在重灌不在改码。
- 重灌后 direct/DB 对拍会把这 3924 个值报成差异 —— **那是修对了**，
  与 #026 的「修对了会被对拍报成差异」同一性质。

## 🧪 测试验证

### 测试计划

1. **单测**（`old-parse-pdms-db`）：手工构造三段载荷——
   标量（2 字）、3 槽数组（7 字，槽 0 空、槽 2 实）、4 槽数组（9 字），
   都按 `ElementType` 声明喂进去：第一段出原值，
   第二段出 `[null, null, 23717/114144]` 而**不是** `3_0`，第三段同理。
2. **重灌回归**：重灌 7333 后重跑 ①，`pe:3_0` / `pe:4_0` 计数应归零。
3. **对拍**：重跑 `direct_attmap_probe --dbnum 7333 --sample 200000`，
   `CRFA` 应从 `neutral_mismatch_hist` 消失。

### 验证标准

- `SELECT CRFA, count() FROM PCLA GROUP BY CRFA` 里不再出现 `pe:3_0` / `pe:4_0`；
- 全库不存在指向 `pe` 表中不存在 id 的引用值；
- 探针 `neutral_mismatch_hist` 不含 `CRFA`。

## 📊 修复效果

### 修复前

```
PCLA:23717_114143.CRFA   文件: [null, null, 23717/114144]   库: pe:3_0      ✗
PCLA:23717_62660.CRFA    文件: [null, null, 23717/62648]    库: pe:3_0      ✗
PCLA 表 CRFA 取值：{null: 7694, pe:3_0: 2788, pe:4_0: 2}
```

### 修复后

```
PCLA:23717_114143.CRFA   文件: [null, null, 23717/114144]   库: pe:23717_114144   ✓
PCLA:23717_62660.CRFA    文件: [null, null, 23717/62648]    库: pe:23717_62648    ✓
PCLA 表 CRFA 取值：数千个互不相同的真引用
```

## 📚 相关文档

- **决策**: ADR-053（direct 模式生成读）
- **计划**: `docs/plans/direct-mode-model-generation.md`；
  `.planning/2026-08-30-direct-read-model-generation/task_plan.md` Phase 1
- **同类**: #026（e3d-io 把十分之一制常量读成设计尺寸编号）——
  同样是「解码器在拿不准的分支上硬凑一个看着像答案的值」
- **会话存档**: `上下文/会话-2026-08-30-接力DR6W-9DOL.md` §③（归因过程）

## 🔄 后续行动

### 立即行动

- [ ] 在 `old-parse-pdms-db` 加 `type_len` 校验 + 3 条单测
- [ ] 拍板下游消费：读侧 schema 是否给这 21 个吊架件 noun 的 `CRFA` 改成数组声明
- [ ] 重灌 7333 后重跑对拍，确认 `CRFA` 退出 `neutral_mismatch_hist`
- [ ] **给 `CRFA` 一个真正的定性**（当前它只是 `Unknown` 兜底）：
      查清生成链是否有任何路径消费它（含 `cascade_refnos` 的引用级联），
      然后登记进 `model_impact` 的静态清单。
      在它还是 `Unknown` 的这段时间里，对拍门对它的豁免属于**未经证实的假设**。

### 预防措施

- [ ] **给所有「按 schema 声明解码」的分支加载荷长度校验**：
      本 issue 是标量引用臂，但同一个 `match` 里的
      `IntegerType` / `BoolType` / `Vec3Type` 也都是**从载荷开头直接读、不看 `type_len`** 的
      （`Vec3Type` 甚至读出了一个长度 `v` 却 `let _len = v` 丢掉，恒读 3 个 f64），
      形状不一致时同样会静默凑值。
      对照组是 `DoubleType` / `DoubleArrayType`——它们会看 `tmp_input.len()`，
      说明「该校验」在这个文件里本来就有先例。应逐臂过一遍。
- [ ] **加一条数据体检**：库里任何 `pe:*` 引用值都应能在 `pe` 表里查到。
      本缺陷只要有这条断言就会在灌库当天暴露，而不是三个月后被对拍捡到。

### 监控计划

把「悬空引用计数」做成灌库后的常驻校验：
`SELECT count() FROM <noun> WHERE <ref_attr> != NONE AND <ref_attr>.id = NONE`。
它不该非零。

## 💡 经验教训

1. **两个实现并排跑是最便宜的定性手段**：这条缺陷在库里躺了很久，
   单看库是看不出来的——`pe:3_0` 语法完全合法。是把文件侧和库侧摆在一起才露馅。
   与 #026 同一条经验。
2. **「取值种类少得离谱」是极强的信号**：3924 行引用只有 2 种取值，
   这种分布本身就在喊「它不是数据」。**分布体检比逐值体检便宜得多。**
3. **别急着让门转绿**：这 2984 处一度是等价门上的红字，
   最省事的做法是在 direct 侧加个特例迁就 `3_0`。
   那样会把库的缺陷洗成绿灯，还会让 direct 侧跟着错。
   **先问「谁对」，再问「门怎么绿」。**
4. **豁免必须留名**：探针的 `neutral_mismatches` 桶不计冲突，但逐键打印。
   一个不打印的豁免和一个没发现的 bug，事后看没有区别。

## 🏷️ 标签

`bug` `old-parse-pdms-db` `write-side` `refno` `silent-wrong-value` `direct-mode` `data-integrity`

---

**创建者**: 9DOL
**最后更新**: 2026-08-30
