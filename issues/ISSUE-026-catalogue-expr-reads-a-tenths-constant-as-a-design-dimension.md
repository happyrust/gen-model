# Issue #026: e3d-io 把「十分之一制常量」读成了「设计尺寸编号」，渲染出 `DD#137` 而不是 `13.7`

## 📋 Issue 信息

- **Issue ID**: #026
- **类型**: Bug 🐛（**静默给出看着像答案的错值**，不报错）
- **优先级**: High 🟠 —— 见「影响范围」：它落在 direct 模式目录几何的必经之路上，
  且失败形态是「一个长得像关键字的 token」而不是异常
- **状态**: Open 📝
- **创建日期**: 2026-08-30
- **发现于**: t-403 表达式方言逐对对拍（`docs/specs/direct-mode-expression-dialect.md` §3.3）
- **归属仓**: `D:\work\plant-code\old\vendor\e3d-io`（**不在 gen-model**，登记在这里是因为
  e3d-io 仓没有 `issues/`，而这条挡的是 gen-model 的 direct 线）
- **相关模块**: `src/record/catalogue_expr.rs`（`Operand::decode` 56-65、
  `impl fmt::Display for Operand` 69-83）；消费方 `src/tty.rs:364 rendered_by_shape`、
  `src/record/descriptor.rs`

## 🔍 问题描述

目录几何的参数属性存的是表达式元组，`catalogue_expr` 把它渲染成 E3D 显示文本。
其中**负的操作数**有两种含义：少数几个是设计尺寸关键字（DDRADIUS / DDANGLE / DDHEIGHT），
其余是**以十分之一为单位的字面常量**。

现在的解码把两者的分界画在「是不是 10 的整数倍」上，于是 `-137` 这种不是整十的负数
被归成了设计尺寸，渲染出 `DD#137`；而它其实是常量 **13.7**。

### 复现步骤

```powershell
cd D:\work\plant-code\old\vendor\e3d-io
cargo run --release --quiet --example genmodel_expr_dialect_probe -- `
    D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams5052_0001 `
    E:\reverse\e3d\shadow_e3d31_aps_all\catvir.dat
# 输出里 "DD#n (unnamed)" 一行即是；TSV 里 grep 'DD#'
```

写库侧同一个元素同一个属性的值，可用 8009 上在跑的库对照：

```powershell
cd d:\work\plant-code\old\gen-model
.\scripts\Invoke-Surreal8009.ps1 -Sql "SELECT PX FROM SVER:13244_69803;"
```

### 预期行为

`SVER:13244_69803.PX` 渲染成 `( -2.5 )`（或 `-2.5`），与写库侧一致。

### 实际行为

渲染成 `( DD#25 )`。`DD#25` 不是数、不是已知关键字，也不报错。

## 🔬 问题分析

### 根本原因

`src/record/catalogue_expr.rs:56-65`：

```rust
fn decode(raw: i32) -> Self {
    match raw {
        0 => Operand::Zero,
        n if n >= NEGATED_PARAM_BASE => Operand::NegatedParam(n - NEGATED_PARAM_BASE),
        n if n >= IPARAM_BASE => Operand::IParam(n - IPARAM_BASE),
        n if n > 0 => Operand::Param(n),
        n if n % CONSTANT_SCALE == 0 => Operand::Constant(-n / CONSTANT_SCALE),   // ← 只收整十
        n => Operand::DesignDimension(-n),                                        // ← 其余全归它
    }
}
```

`CONSTANT_SCALE = 10`。**`% 10 == 0` 这个守卫是错的**：常量以十分之一为单位存，
但并不只取整十。`-250` 落进 `Constant(25)` 渲染成 `25` ✓，而 `-25` 落进
`DesignDimension(25)` 渲染成 `DD#25` ✗——它其实是 2.5。

也就是说：**能被 10 整除的常量碰巧走对了，不能被整除的全部走进了兜底分支。**
兜底分支的注释写着「Only the two ids the sample exercised are named; the rest keep
their number so a reader can tell an unrecognised keyword from a wrong one」——
它的本意是保守，但保守的前提（落进来的都是关键字）不成立。

### 证据：67 对逐对对拍，67/67 一致

把 e3d-io 渲染串与 8009 库里写库侧存的串按 `refno + 属性名` join
（`docs/specs/direct-mode-expression-dialect.md` §1）：

| e3d-io | 写库侧 | 对数 | 证人 |
|---|---|---:|---|
| `DD#25` | `2.5` | 48 | `SVER:13244_69803.PX` |
| `DD#137` | `13.7` | 10 | `SCYL:13244_662411.PDIA` |
| `DD#206` | `20.6` | 3 | `PTCA:13244_662391.PZ` |
| `DD#85` | `8.5` | 2 | `PTCA:13244_664255.PY` |
| `DD#1505` | `150.5` | 1 | `PTCA:13244_664258.PX` |
| `DD#355` | `35.5` | 1 | `PTCA:13244_662402.PX` |
| `DD#1519` | `151.9` | 1 | `PTCA:13244_662385.PZ` |
| `DD#213` | `21.3` | 1 | `SCYL:13244_662426.PDIA` |

**规则 = 除以 10。** 8 个不同取值、3 个 noun，无一例外。
注意上表每个值都不是 10 的整数倍——正因为整十的那些已经走对了分支，
所以对拍能看见的只有走错的这一半。

### 影响范围

- 全语料（`ams5052_0001` + `ams5053_0001` + `ams8000_0001`，729 468 个元素、
  1 039 467 个属性值）里 **`DD#` 出现 1 695 次**。
- 它落在 **direct 模式目录几何的必经之路**上：`extract_element_with_descriptors`
  交出 `RawWords` → `rendered_by_shape` → 消费方求值。渲染成 `DD#137` 之后，
  `aios_core::eval_str_to_f64` 认不出这个 token，整条表达式求值失败或落 0，
  几何随即错形 —— **而这不会报错**。
- 目前没有实际损害：direct 线还没上线，DB 模式走的是写库侧的值（是对的）。
  **它是一颗埋着的雷，不是正在漏的水。**

### 相关代码

| 位置 | 角色 |
|---|---|
| `src/record/catalogue_expr.rs:56-65` | `Operand::decode`，缺陷本体 |
| `src/record/catalogue_expr.rs:69-83` | `Display`，`DD#{id}` 的输出点 |
| `src/record/catalogue_expr.rs:33` | `const CONSTANT_SCALE: i32 = 10` |
| `src/tty.rs:364` | `rendered_by_shape`，五路分派器（私有，见 #「后续行动」） |
| `old-parse-pdms-db/src/parse_explict_tools.rs` | 写库侧同一操作数读成数的那条路径，是对照基准 |

## 🛠️ 解决方案

### 方案概述

把分界从「是不是整十」改成「是不是那几个具名的设计尺寸 id」：
**负操作数里只有少数几个 id 是关键字，其余一律是十分之一制常量。**

### 技术实现（建议，未提交）

```rust
/// 具名设计尺寸的 id。其余负操作数都是十分之一制常量。
const DESIGN_DIMENSION_IDS: [i32; 3] = [3, 4, 5];   // DDRADIUS / DDANGLE / DDHEIGHT

n if DESIGN_DIMENSION_IDS.contains(&-n) => Operand::DesignDimension(-n),
n => Operand::Constant(-n),                          // 值以十分之一计
```

同时 `Operand::Constant` 要从 `i32`（整数）改成能表达 `2.5` 的形式
（`Constant(i32)` 存**十分之一**、`Display` 里再除以 10 并按 `format_number` 去尾零，
避免引入浮点误差）。

### ⚠️ 未定：`DD#6` 与 `DD#7` 不能跟着一起改

语料里除了上面那批大值，还有 **`DD#6` 60 次、`DD#7` 8 次**，
而 **join 里没有它们的写库侧证人**（那两个值没出现在已摄入的 10 个 noun 上）。

- 若按上面的规则改，它们会变成 `0.6` 与 `0.7` —— 数值上很像常见系数，看着合理；
- 但它们也完全可能就是 id 6 / id 7 的设计尺寸关键字，只是本采样没见过它们的名字。

**这两个值必须先有证据再动**，两条路任选：
① 把带 `DD#6` / `DD#7` 的目录元素摄入 8009 后重跑 join；
② 用一条 E3D TTY `Q` 直接看 E3D 自己怎么印。

在拿到证据前，建议实现成：`3/4/5` 具名、`1/2/6/7/8/9` **显式报未知**
（保留 `DD#n` 或返回 `None`，让它可被审计），`≥10` 按常量。
**不要为了让语料全绿而把 6/7 一起吞掉** —— 那正是本 issue 批评的那种「兜底吞掉」。

### 风险评估

- **改动会让 1 695 个值从字符串变成数**，双跑对拍会把它们报成差异。
  那是修对了，不是回归 —— 与 `docs/specs/direct-mode-expression-dialect.md` §4.1
  记的 7 处系数精度差异同一性质，建议对拍脚本一并单列。
- `DD#6` / `DD#7` 若判错，错的是 68 个值的数量级（0.6 vs 尺寸 6），
  几何会明显变形而不是微偏，**反而容易在对拍里被抓到** —— 但仍应先取证。

## 🧪 测试验证

### 测试计划

1. **单测**（`catalogue_expr.rs` 的 `mod tests`）：把上表 8 个真实取值钉成用例，
   `decode(-25)` → `Constant`、`Display` 出 `2.5`；`decode(-4)` → `DesignDimension(4)`
   → `DDANGLE`；`decode(-250)` → `25`（不能被这次改动弄坏）。
2. **语料回归**：重跑 `examples/genmodel_expr_dialect_probe`，
   `DD#n (unnamed)` 一档应从 1 695 降到只剩未定的 `DD#6`/`DD#7`（68 个）。
3. **逐对对拍**：重跑 §「证据」那套 join，`RULE DD#n -> n/10` 一档应从 67 降到 **0**
   （因为不再需要这条映射规则）。

### 验证标准

对拍分桶里 `RULE DD#n -> n/10` 归零，且 `UNEXPLAINED` 不增加。

## 📊 修复效果

### 修复前

```
SVER:13244_69803.PX     e3d-io: ( DD#25 )      写库侧: -2.5
SCYL:13244_662411.PDIA  e3d-io: DD#137         写库侧: 13.7
```

### 修复后

```
SVER:13244_69803.PX     e3d-io: ( -2.5 )       写库侧: -2.5     ✓
SCYL:13244_662411.PDIA  e3d-io: 13.7           写库侧: 13.7     ✓
```

## 📚 相关文档

- **规格**: `docs/specs/direct-mode-expression-dialect.md`（§3.3 是本 issue 的证据源，
  §4.1 记了同类的「修对了会被对拍报成差异」）
- **缺口清单**: `docs/plans/2026-08-30-e3d-io-gen-model-gap.md` §G4
- **决策**: ADR-053（direct 模式生成读）
- **探针**: `old/vendor/e3d-io/examples/genmodel_expr_dialect_probe.rs`

## 🔄 后续行动

### 立即行动

- [ ] 取 `DD#6` / `DD#7` 的证据（摄入后重 join，或一条 TTY `Q`）
- [ ] 改 `Operand::decode` 与 `Operand::Constant` 的表示，加 3 条单测
- [ ] 重跑探针与 join，确认 `RULE DD#n -> n/10` 归零
- [ ] 通知属性转换器（opus-5-22）：修好后规则 C 作废，别再留映射

### 预防措施

- [ ] **给兜底分支加一条「说不出所以然就别渲染」的纪律**：
      `DesignDimension(id)` 现在的兜底会印出一个**长得像关键字的 token**，
      调用方分不清「一个我不认识的关键字」与「一个被读错的数」。
      建议未知 id 返回 `None`，让 `rendered_by_shape` 走到下一路或最终报错。
- [ ] **公开 `rendered_by_shape`**（`src/tty.rs:364` 现为私有）。
      它是五路渲染器的唯一正确调用顺序，现在已被抄了两遍（两个探针），
      转换器落地会是第三遍 —— 抄错顺序就会把轴规格当 PML 读。
      详见 `direct-mode-expression-dialect.md` §6。

### 监控计划

探针的 `DD#n (unnamed)` 计数是常驻指标：它不该再增长；
若某个新库让它跳起来，说明遇到了本 issue 未覆盖的 id 段。

## 💡 经验教训

1. **兜底分支要能回答「它跳过的东西谁会发现」**：这里的兜底印出一个像模像样的
   `DD#137`，既不报错也不为空，于是没有任何人会发现。宪法「静默失效是最高级别缺陷」
   条说的就是这个形状。
2. **「样本里只见过 3/4/5」不等于「其余都是同一类」**：注释诚实地写了它只认得两三个 id，
   但代码把「不认得」当成了「是同一类的另一个」。
3. **两个实现并排跑，是最便宜的定性手段**：这条缺陷不是读代码读出来的，
   是把两侧对同一个元素同一个属性的输出摆在一起看出来的。67 对就够定性了。

## 🏷️ 标签

`bug` `e3d-io` `catalogue-expression` `direct-mode` `silent-wrong-value` `high-priority`

---

**创建者**: opus-5-20
**最后更新**: 2026-08-30
