# direct 模式表达式方言规格：e3d-io 渲染串 ↔ 现有求值器

> **给 `direct-mode-query-surface.md` §6.5.3 的读者**：那一节说「方言不同、现有求值器
> 没见过 `ATTRIB PARA[10 ]`」，来源是我 t-357 的判断。**本文推翻它**：两边是同一种方言。
> 该节已同步更正。

> 日期：2026-08-30。状态：**已实测，结论可执行**。
> 起因：`docs/plans/2026-08-30-e3d-io-gen-model-gap.md` §G4 把「方言不同」列为阻塞项。
> **本文推翻那条判断的一半**：两边其实是同一种方言，见 §0。
> 关联：ADR-053（direct 模式生成读）、`docs/plans/direct-mode-model-generation.md`、
> `docs/plans/2026-08-30-e3d-io-gen-model-gap.md`（G4）。
>
> 证据：探针 `old/vendor/e3d-io/examples/genmodel_expr_dialect_probe.rs`（本轮新增，
> 只加 examples，未动 src/tests），复现命令见 §6。**未改任何生产代码。**

## 0. 结论

**渲染侧与写库侧是同一种方言。不需要扩求值器，只需要三条小规则。**

我在 `…-e3d-io-gen-model-gap.md` §G4 写的「`ATTRIB PARA[10 ]` 与 `ATTRIB RPRO G` 这两种写法
现有求值器没见过」是错的：那两种写法**本来就是写库侧自己的输出格式**
（`old-parse-pdms-db/src/parse_explict_tools.rs:334/354/361/364`），而 `eval_str_to_f64`
对它们各有专门分支。当时的判断来自读测试用例文件，不是读实现，特此更正。

真正的差异只有三条，且都是纯语法搬运：

| 规则 | 渲染侧 | 写库侧 | 证人 / 全语料出现数 |
|---|---|---|---:|
| **A** 方向值的 `AXIS ` 前缀 | `AXIS -Y` | `-Y` | 323 / 4 537 |
| **B** 负点号 | `P1005` | `-P5` | 102 / 10 144 |
| **C** 十分之一制常量 | `DD#25` | `2.5` | 67 / 1 695 |

规则 C 实质上是 **e3d-io 的一个渲染缺陷**（详见 §3.3），建议报给 e3d-io 修；在修好之前
由转换器做映射。

## 1. 证据是怎么来的

不是看例子，是**逐对对拍**：同一个 refno、同一个属性名，把 e3d-io 渲染出的串与 SurrealDB 里
写库侧存的串并排比。

| 侧 | 来源 | 量 |
|---|---|---:|
| 渲染侧 | e3d-io 全量扫 `ams5052_0001`（306 945 键）、`ams5053_0001`（415 918 键）、`ams8000_0001`（6 605 键） | 729 468 个元素、**1 039 467 个属性值**、描述符提取失败 **0** |
| 写库侧 | 8009 上在跑的库，10 个目录 noun（PTCA / SEXT / SCTN / SBOX / SCYL / PTAX / PTMI / SVER / SNOU / SLOO）`SELECT * FROM <NOUN>` | **36 617 行** |
| 交集 | 按 `refno + 属性名` join | **50 307 对** |

> **证据边界（重要）**：50 307 对只占全语料 1 039 467 个值的约 **5%** —— 库里只有
> `cata_closure` 按需拉进来的那部分目录元素。三条规则在**各自的证人上 100% 一致**，
> 但不是全语料验证。每条规则的「证人数 / 全语料出现数」都在上表里写明了，
> 落地时按这个强度取舍。

### 1.1 逐对结果

| 分类 | 对数 | 占比 |
|---|---:|---:|
| 逐字节相同 | 18 395 | 36.6% |
| 只差首尾空白 | 16 455 | 32.7% |
| 只差括号 / 空格（去掉全部括号与空白后完全相等） | 14 956 | 29.7% |
| 规则 A：`AXIS ` 前缀 | 323 | 0.6% |
| 规则 B：`P(1000+n)` ↔ `-P n` | 102 | 0.2% |
| 规则 C：`DD#n` ↔ `n/10` | 67 | 0.1% |
| 系数精度（**e3d-io 更准**，见 §4.1） | 7 | 0.0% |
| **解释不了** | **2** | **0.0%** |

那 2 条还不是方言问题：`SEXT:13244_41434` 的 `PZ` 与 `PHEI` 在写库侧存的是**空串**，
e3d-io 渲染出 `( 0 )` 与 `( ATTRIB DESP[1 ] )` —— 是写库侧丢了值。

## 2. 语法要素逐条对照

「渲染侧写法」取自 e3d-io 的五个渲染器
（`src/record/{catalogue_expr,axis_spec,direction_spec,point_list,catalogue_pml}.rs`），
「求值器怎么处理」取自 `../vendor/old-aios-core/src/rs_surreal/resolve.rs`，行号可复核。
「全语料」是 §1 那三个库的全量计数。

| # | 要素 | 渲染侧 | 写库侧 | 求值器怎么处理 | 机械映射？ | 全语料 |
|---|---|---|---|---|:--:|---:|
| E1 | 属性下标 | `ATTRIB PARA[10 ]` | **同**（`parse_explict_tools.rs:334`+`:361` 的 `format!("ATTRIB {n}")` / `[{num} ]`） | `prepare_eval_str` 删 `ATTRIB`（`resolve.rs:241`）；参数正则 `(:?[A-Z_]+[0-9]*)(\s*\[?\s*N\s*\]?)?`（`:315`）吃带空格的方括号；键 `PARA10` 由 `:147` 写入 context | **无需** | 165 716 |
| E2 | RPRO 限定名 | `ATTRIB RPRO G` | **同**（`:345-349`+`:364`） | 删 `ATTRIB` 后 `(RPRO)\s+([a-zA-Z0-9]+)` → 键 `RPRO_G`（`:319-322`），正是 `:171` 写入 context 的键 | **无需** | 77 951 |
| E3 | 裸属性 | `ATTRIB ANGL` | **同**（`:359`） | 同 E1，键 `ANGL` | **无需** | 115 |
| E4 | 参数 | `PARAM 2` | 同 | 正则跨空格匹配 → 键 `PARAM2`（`:148`） | **无需** | 91 526 |
| E5 | 负参数 | `- PARAM 2` | 同 | 先替换 `PARAM 2`，再由 tinyexpr 处理一元负号 | **无需** | 1 570 |
| E6 | 保温参数 | `IPARAM 1` | 同 | `para_name` 去尾 `M`（`:385-387`）→ 键 `IPARA1`（`:149`，恒 0） | **无需** | 1 526 |
| E7 | 系数乘 | `0.7 TIMES PARAM 2` | 同 | `TIMES`/`MULT` → `*`（`:512`） | **无需** | 40 741 |
| E8 | 求和 | `SUM PARAM 5 IPARAM 1` | 同 | `SUM a b` → `(a + b)`（`:558-566`） | **无需** | — |
| E9 | 设计尺寸 | `DDANGLE` / `DDRADIUS` / `DDHEIGHT` | 同 | 直接取 context（`:516-518`） | **无需** | 688 |
| E10 | 函数 | `TAN ( x )`（有空格） | `TAN( x )`（无空格） | 求值前按空白切词、再用空格拼回并小写（`:500-582`），tinyexpr 忽略空白 | **无需** | SIN 9 150 / TAN 8 198 / COS 7 470 |
| E11 | 括号深度 | `( a / b - c / d )` | `( ( a / b ) - ( c / d ) )` | 标准优先级，算术等价 | **无需** | §1.1 那 29.7% |
| E12 | 一元负号位置 | `( - A / 2 )` | `( -( A / 2 ) )` | `(-A)/2 = -(A/2)`，算术等价 | **无需** | 同上 |
| E13 | 单项加括号 | `( ATTRIB PARA[3 ] )` | `ATTRIB PARA[3 ]` | 无影响 | **无需** | 同上 |
| **E14** | **方向的 AXIS 前缀** | `AXIS -Y` | `-Y` | 方向值不走 `interp`，由方向解析消费 | **规则 A** | 4 537 |
| **E15** | **负点号** | `P1005` | `-P5` | — | **规则 B** | 10 144 |
| **E16** | **十分之一制常量** | `DD#25` | `2.5` | — | **规则 C** | 1 695 |
| E17 | 轴旋转连写 | `Y45-X` | `Y 45 -X` | — | **未定**（只有 1 对证人，见 §5.1） | 1 384 |
| E18 | `... OF ...` | `CENTER TO BOTTOM OF FACE` | — | `([A-Z\s]+) OF (PREV\|NEXT\|\d+/\d+)`（`:266`）匹配不上 `FACE`，原样留下 | **未定**（见 §5.2） | 466 |
| E19 | 轴字母 / 点号 | `X` `-Z` `P3` `P61 P71` | 同 | 方向 / 点表值，不走 `interp` | **无需** | 轴 70 593 / 点 43 493 |
| E20 | 裸数字 | `0` `2550` | 同（写库侧常带前导空格，见 §1.1 那 32.7%） | — | **无需** | 175 837 |

## 3. 三条规则的产式与证人

### 3.1 规则 A · `AXIS ` 前缀

```text
渲染侧 "AXIS " <bare-axis>   →   写库侧 <bare-axis>
其中 <bare-axis> ∈ { X, Y, Z, -X, -Y, -Z }
```

**只在后面是光杆轴字母时剥。** 带转角的形式两边**本来就逐字节相同**，剥了就错：

| 例 | e3d-io | 写库侧 | 处置 |
|---|---|---|---|
| `PTCA:13244_41373.PTCD` | `AXIS -Y` | `-Y` | 剥 |
| `PTCA:13244_41367.PTCD` | `AXIS -X` | `-X` | 剥 |
| `PTCA:13244_63275.PTCD` | `AXIS -X ( ATTRIB PARA[10 ] ) -Z` | **同** | 不剥 |
| `PTCA:13244_67377.PTCD` | `AXIS -X ( ATTRIB ANGL ) Z` | **同** | 不剥 |

证人 386 对中 323 对属于「剥」、63 对属于「本来就相同」，无第三类。
全语料 `AXIS <dir>` 出现 4 537 次，全部落在 `PTCD`（4 529）与 `PTPOS`（8）上。

### 3.2 规则 B · 负点号

```text
渲染侧 "P" (1000 + n)   →   写库侧 "-P" n        (n ≥ 0)
渲染侧 "P" n            →   写库侧 "P" n         (n < 1000，原样)
```

102 对证人 **102/102 一致**，跨 11 个不同取值、4 个 noun：

| e3d-io | 写库侧 | 对数 | 例 |
|---|---|---:|---|
| `P1005` | `-P5` | 46 | `SEXT:13244_53846.PAAX` |
| `P1006` | `-P6` | 41 | `SEXT:13244_52329.PAAX` |
| `P1001` | `-P1` | 4 | `SCYL:13244_683626.PAXI` |
| `P1002` / `P1003` / `P1004` | `-P2` / `-P3` / `-P4` | 各 2 | `SCYL:13244_714050.PAXI` … |
| `P1025`–`P1036` | `-P25`–`-P36` | 各 1 | `SCYL:13244_664305.PAXI` … |

全语料 `P1xxx` 出现 10 144 次（`P1001` 2 495、`P1002` 1 739、`P1004` 1 123…），
分布形状与「1000 + 点号」完全吻合。

> **一处不确定**：语料里有 88 个 `P1000`，按规则映射成 `-P0`，而 `-P0` 是否有意义未验证
> （证人集里没有 `P1000`）。落地时对 `n = 0` 单独报错或记账，**不要静默映射**。

### 3.3 规则 C · 十分之一制常量 —— 同时是 e3d-io 的一个缺陷

```text
渲染侧 "DD#" n   →   写库侧 (n / 10)
```

67 对证人 **67/67 一致**：`DD#25`⇄`2.5`、`DD#137`⇄`13.7`、`DD#206`⇄`20.6`、
`DD#85`⇄`8.5`、`DD#355`⇄`35.5`、`DD#1505`⇄`150.5`、`DD#1519`⇄`151.9`、`DD#213`⇄`21.3`。
例：`SVER:13244_69803.PX`、`SCYL:13244_662411.PDIA`、`PTCA:13244_664258.PX`。

**这不是方言差异，是渲染错了。** `src/record/catalogue_expr.rs:76-79`：

```rust
Operand::DesignDimension(3) => write!(f, "DDRADIUS"),
Operand::DesignDimension(4) => write!(f, "DDANGLE"),
Operand::DesignDimension(5) => write!(f, "DDHEIGHT"),
Operand::DesignDimension(id) => write!(f, "DD#{id}"),
```

3/4/5 三个具名值是对的；**兜底分支把「十分之一制的常量」当成了设计尺寸编号**。
写库侧读同一个操作数得到的是一个数。全语料 1 695 处。

**建议**：报给 e3d-io，把兜底分支改成常量 `id as f64 / 10.0`（不在本文写入面内，故只提建议）。
在修好之前，转换器按上式映射；修好之后这条规则自动消失。

## 4. 落地后会变的东西

### 4.1 有 7 处系数会变得更准（不是回归）

| 例 | e3d-io | 写库侧 |
|---|---|---|
| `SVER:13245_571087.PX` | `-0.175 TIMES PARAM 14` | `-0.18 TIMES PARAM 14` |

系数在文件里是**四十分之一制**（`catalogue_expr.rs` 的 `coefficient`，−7/40 = −0.175），
写库侧把它舍到两位小数。切 direct 之后这些值会变，**双跑对拍会把它们报成差异 —— 那是修对了**。
共 7 对；建议在对拍脚本里按「渲染侧系数与写库侧系数之差 < 0.01 且其余部分相同」单列一类，
不要混进真回归。

### 4.2 有 2 处会多出值

`SEXT:13244_41434` 的 `PZ`、`PHEI` 写库侧是空串，direct 下会读出 `( 0 )` 与
`( ATTRIB DESP[1 ] )`。同样是修对了。

## 5. 未定项（**不猜**）

### 5.1 E17 · 轴旋转连写

e3d-io 的 `axis_spec::rotated_axes`（`src/record/axis_spec.rs:56-67`）把三段连写成
`Y45-X`；写库侧那一条证人是 `Y 45 -X`。**只有 1 对证人**（`PTAX:13244_90734.PAXI`），
不足以立规则。全语料这类串 1 384 处（`classify` 里落进 `UNCLASSIFIED`），集中在
`PCON`(650) / `PBAX`(365) / `PAXI`(193) / `PAAX`(114)。

**处置**：落地前需要补证人 —— 要么把更多带 `PAXI/PBAX/PAAX` 的目录元素摄入库里再 join 一次，
要么用一条 E3D TTY `Q` 定夺。**在此之前按「未定」记账，不要写映射。**

### 5.2 E18 · `... OF ...`

e3d-io 的 `catalogue_pml`（`src/record/catalogue_pml.rs:285`）会渲染出
`CENTER TO BOTTOM OF FACE` 这类串。全语料 466 处，**全部落在 `DATA`/`TEXT`/`SDTE`
这三个 noun 的 `DTIT`/`STEX`/`RTEX` 上**，也就是标题与文本，不是几何标量。
join 里 0 对证人（这三个 noun 不在库里已摄入的 10 个之列）。

求值器那条 `OF` 正则（`resolve.rs:266`）要求右边是 `PREV|NEXT|\d+/\d+`，`FACE` 匹配不上，
串会原样留到 tinyexpr 前，大概率求值失败。

**处置**：先确认生成链是否真的对 `DTIT`/`STEX` 求值 —— 如果只是当文本显示，这条不需要映射。
**未定**，需要盘查询面的人（opus-5-17）确认。

## 6. `rendered_by_shape` 要不要公开 —— 建议：**公开**

现状：五路分派器 `rendered_by_shape` 是 `src/tty.rs:364` 的私有函数；
`extract_element_with_descriptors` 交给调用方的是裸 `DescriptorValue::RawWords(Vec<u32>)`。

**建议公开**，理由三条：

1. **顺序是有语义的，不是随手排的。** `catalogue_expr` → `axis_spec` → `direction_spec`
   → `point_list` → `catalogue_pml` → `text`，前面的更专、后面的更泛。抄错顺序就会把
   一个轴规格当成 PML 程序读。这正是「唯一权威实现」该收口的形状。
2. **已经被抄过两遍了。** 我这两轮探针各抄了一份（`genmodel_gap_probe.rs` 与
   `genmodel_expr_dialect_probe.rs` 里的 `render_by_shape`），转换器落地时会是第三份。
3. **它是 direct 线的必经之路。** 全语料 1 039 467 个属性值里，**705 094 个（67.8%）是
   raw 字元组**，其中 **384 980 个（占全部值的 37.0%）正是靠这五路才渲染得出来**；
   不过这个分派器，这 384 980 个值到消费方手里就只是一串没有含义的 `u32`。

**建议形态**（供 e3d-io 的人定，本文不改 src）：放在
`src/record/mod.rs` 或新的 `src/record/shape.rs`，签名
`pub fn render_by_shape(words: &[u32]) -> Option<(Renderer, String)>`，
返回值带上是哪一路 —— 消费方需要这个来决定「这是标量表达式、方向、还是点表」，
只给字符串会逼它再猜一次。`tty.rs` 改为调用它，保持一处实现。

## 7. 工量

| 项 | 归属 | 工量 |
|---|---|---|
| 三条规则（A/B/C）+ 单元测试 | 属性转换器（opus-5-22） | **约 60 行**，规则本身各 3–8 行，其余是测试与 `n=0` 之类的边界报错 |
| 公开 `rendered_by_shape` | e3d-io | 约 30 行（挪函数 + 加返回枚举 + `tty.rs` 改调用） |
| 修 `DD#n` 兜底分支 | e3d-io | 约 5 行；修完规则 C 作废 |
| E17 补证人 | 侦察 | 半天（多摄入几个 `PTAX`/`SEXT` 到库里，或一条 TTY `Q`） |
| E18 定性 | 查询面盘点（opus-5-17） | 问一句：生成链对 `DTIT`/`STEX` 求值吗 |
| 对拍脚本加「系数精度」白名单类 | 双跑对拍 | 约 10 行 |

**原先 G4 估的「中，且是这次最需要先做对拍的一项」现在收敛为：对拍已做完，落地约 60 行。**

## 8. 复现

```powershell
cd D:\work\plant-code\old\vendor\e3d-io
cargo build --release --example genmodel_expr_dialect_probe

# 全量扫 5052 + 5053 + 8000，频次表进 stdout，逐值 TSV 进 %TEMP%\expr_dialect.tsv
cargo run --release --quiet --example genmodel_expr_dialect_probe

# 指定单库：<db> <vir.dat> [out.tsv] [limit]
cargo run --release --quiet --example genmodel_expr_dialect_probe -- `
    D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams5052_0001 `
    E:\reverse\e3d\shadow_e3d31_aps_all\catvir.dat out.tsv 5000
```

写库侧一半（需要 8009 上有已摄入的目录元素）：

```powershell
cd d:\work\plant-code\old\gen-model
foreach ($n in @('PTCA','SEXT','SCTN','SBOX','SCYL','PTAX','PTMI','SVER','SNOU','SLOO')) {
  .\scripts\Invoke-Surreal8009.ps1 -Sql "SELECT * FROM $n;" | Out-File -Encoding utf8 "db-$n.json"
}
```

join 用的三个一次性脚本（分桶、点号、按形状）本轮跑在
`D:\work\plant-code\.target-opus520\` 下，是临时件，未入任何仓库；
逻辑很短，按 §1.1 的分类口径重写即可：先比逐字节、再比 trim、再比「去掉全部括号与空白」，
剩下的按 A/B/C 三条规则各试一次，仍剩下的就是需要新证据的。

> 提醒：`cargo` 在共享 `target` 上并发编译会链到过期 rlib（症状是「编译器说符号不存在但源码里有」）。
> 本轮全程用独立 `CARGO_TARGET_DIR` 跑，未占用共享目录。
