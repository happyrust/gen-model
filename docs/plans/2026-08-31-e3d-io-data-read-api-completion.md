# e3d-io 数据读取 API 完备性分析 + 完善开发计划（2026-08-31）

> 任务口径（用户原话）：分析 `vendor/e3d-model` 的数据接口是否已经完备；参考 ida-bridge
> 分析 `core.dll` 与 `Core3D.dll`；进一步完善 e3d-model 依赖的**数据读取 API 函数**；用
> plannotator 制定开发计划。
>
> 前置事实：**数据读取 API 落在 `vendor/e3d-io`**（用户 2026-08-30 拍板，S1 已落 `db_element.rs`）。
> `vendor/e3d-model` 是「按 core.dll 方案另立的生成算法」，它**一律经 e3d-io 直读**库文件。
> 因此「e3d-model 的数据接口」= e3d-model 经由 e3d-io 拿到的读侧能力。本计划只谈**读侧**，
> 是 `docs/plans/2026-08-30-core-dll-api-alignment.md`（对标矩阵）在「生成算法另立」口径下的续篇。

## 0. 结论（TL;DR）

- **结构件路径：数据接口已完备。** DFS 遍历、noun 分类、环拉伸（PANE/FLOOR/GWALL）、
  基本体、回转体、负几何——所需的读能力（对标矩阵 A 身份 / B 导航 / C typed getter /
  D 引用跳转 / E 名字）都已在 e3d-io `DbElement` 门面落地。ams1112 全库 30940 元素账已平，
  生成 4476 件正体、478 处负体，GWALL 局部轮廓 17/20 逐轴 <0.05mm 对上 E3D。
- **目录 / 参数化路径：数据接口尚不完备，且卡在同一处。** 774 个 `catalog_pending` 元素
  （FITT 86 / PFIT 200 / SBFI 140 / FIXING 255 / WALL 14 / STWALL 76 / CMFI 3，以及后续 PIPE/BRAN/
  GENSEC）建不出来，根因**不是几何算法缺失，而是数据接口读不出目录几何要用的数**：
  e3d-io 能把目录参数表达式**解码并渲染成文本**，却**不能求值成数字**。
- **一句话缺口**：`catalogue_expr.rs` / `catalogue_pml.rs` 只有 `render()`（→字符串），
  没有 `evaluate()`（→f64）。core.dll 的权威求值器是 `exprlib/EXEV*`。补上「求值 + 求值所需的
  参数环境读取（设计参数 / 目录几何实例 / qualifier·下标·UDA·owner 链属性）」，目录路径即通。

## 1. 逆向证据（ida-bridge，两个映像轴各自坐实）

> IDB：`D:\ida_scratch\plant3\core.dll.i64`（0x5，数据读取真身，本次开 `replica\core.dll.i64`
> 为 `idalib-22484`）、`D:\ida_scratch\plant3\Core3D.dll.i64`（0x10，调用读 API 的生成/interop 模块，
> 已开 `idalib-35724`）。地址分两轴，勿并列比较（对标矩阵 §2 已述）。

### 1a. core.dll（0x5xxxxxx）——读侧真身，本计划的对标落点

| FORTRAN | core.dll 地址 | 语义 | 关联缺口 |
|---|---|---|---|
| `EXEVRE` | `0x51b485a` | 表达式求值 → 实数 | **G1** |
| `EXEVF` | `0x51b3c17` | 表达式求值 → 实数(F) | G1 |
| `EXEVPE` | `0x51b3d33` | 表达式求值 → 位置 | G1 |
| `EXEVLG` | `0x51b3ae7` | 表达式求值 → 逻辑 | G1 |
| `EXEVRD` | `0x51b474b` | 表达式求值 → 方向 | G1 |
| `EXEVTX` | `0x51b37da` | 表达式求值 → 文本 | G1 |
| `GATPAR` | `0x51b228f` | 取目录/设计参数（PARAM[] 向量） | **G2** |
| `GATINS` | `0x51b2e1e` | 取目录几何实例 | G2 |
| `GTGINS` | `0x51b2f4e` | 取目录几何集实例 | G2 |
| `GTSINS` | `0x51b2ee9` | 取目录几何集实例(S) | G2 |
| `GATPOS` | `0x51e1621` | 取位置属性 | C（已复刻） |
| `GATDIR` | `0x51e1dfd` | 取方向属性 | C（已复刻） |
| `PPEVST` | `0x51ec783` | P-point / 连接点求值 | **G4** |

### 1b. Core3D.dll（0x10xxxxxx）——谁在调这些读 API（import core.dll 4859 处）

- 全部读 API 在 Core3D 里是一段 `0x108e96fc–0x108eb6d2` 的 jmp 桩，逐一跳回上表 core.dll 0x5 真身
  （`EXEV* / GATPAR / GATINS / GTGINS / CSTCT2 / CGTCT2 / CGTCPR / PPEVST` 都在这段）。
- 管线（teach/0009 已证）：`add/ADDDES → build/MODCMP →(I*COM 谓词分类) build/ELMODL →
  SGDRAW/GMDRAW → cachegml/GTGEOM → libgm gm_Create*`。**几何缓存键在目录层**（同型号构件复用一份几何）。
- `cachegml/GTGEOM`（`0x10341d2e`）反编译实证：`GATINS` 取实例 → `DGETI/DGETF/DGETNO` 读数
  → `CSTCT2/CGTCT2` 目录分类 → `I*COM` 谓词判几何种类 → 派发 `catgeom(0x10714FC0)` / `primgeom(0x10343B80)`。
- `CSG_TreeBuilderPrimitive::getCSGTree`（`prim.c`）实证：读 noun+属性建正体，负几何（洞）来自
  **基本体下方** + **FIXING 的 TMPL 子树**（`addHolesBelowTemplate`，`DB_Iterator` 走 NOUN_TMPL）。

## 2. e3d-io 读侧现状（对标矩阵逐组核对）

| 组 | 能力 | 现状 | 判定 |
|---|---|---|---|
| A 身份 | refno/db_no/exists/element_type/noun_hash/stored_name/is_descendant_of | `db_element.rs` 全落 | ✅ |
| B 导航 | owner/members(原序游标)/member(i)/first/last/next/previous/members_of_type | 全落（NXTITM 语义） | ✅ |
| C typed getter | string/int/double/bool/ref/各数组/position/direction/orientation | 全落（按文件自报形状投影） | ✅ 标量/数组 |
| C qualifier | `ATTRIB X[i]` 下标 / `ATTRIB X Y` qualifier / `:UDA` / owner 链属性 | **无门面**；`uda_catalog.rs` 在但未接 | ❌ **G3** |
| D 引用跳转 | get_element/get_element_array（跨库走池 + resolver 补开） | 全落 | ✅ |
| E 名字 | find_named（Named 档全扫，原生无 name 表） | 全落 | ✅ |
| F 表达式 | 目录参数表达式 / PML 表达式 | `catalogue_expr.rs`+`catalogue_pml.rs` 只 `render()`→文本 | ❌ **G1**（无 evaluate） |
| — 目录几何 | SPRE/CATR→CATA→GMSET 链、设计参数向量、几何集图元 | 机制零件在（跨库 ref、typed getter），**无目录门面** | ❌ **G2** |
| B world | Db.World/WorldMembers/GetFirstWorld | `is_world` 位在；gen_ams 手工扫根 | 🔶 **G5**（非阻塞） |
| G 差分 | index::diff 机制在，句柄门面缺 | 生成链不消费 | 🚫 记账 |
| I MDB | SYS 库 CURD 成员清单 / Db.Name | 现靠 DbOption/定位器 | 🔶 **G6**（非阻塞） |

> 关键更正（相对对标矩阵）：矩阵写于 `catalogue_expr/catalogue_pml` 落地**之前**，把 G4 表达式
> 押「修法 A：字符串方言对齐 EXEV*」。现状已变——e3d-io 有了**结构化解码器**（AST + 逆波兰），
> 且逐条对过 E3D `Q` 渲染。故本计划改押「直接对结构化形式求值」（见 §4 决策 1），绕开字符串方言。

## 3. 缺口清单（按阻塞度排序）

- **G1 · 目录参数表达式求值（主缺口，阻塞全部目录几何）**
  - 缺：`catalogue_expr::evaluate` / `catalogue_pml::evaluate`——把已解码的
    `PARAM n / -PARAM n / IPARAM n / DDRADIUS·DDANGLE·DDHEIGHT / 常量 / SUM·DIFFERENCE·TANF·SINF·COSF·ATANT /
    ATTRIB X[i] / ATTRIB X Y / ATTRIB X OF owner…` 绑定到一个参数环境，返回 `Option<f64>`（不懂就 None，响亮）。
  - core.dll 落点：`exprlib/EXEV*`（§1a）。
- **G2 · 求值环境 = 目录几何实例读取**
  - 缺：目录感知门面——(a) 解 SPRE/CATR/PRTREF 引用链进 CATA 库到几何集（GMSET）；
    (b) 暴露设计参数向量（`GATPAR` 语义，PARAM[]）与绝热参数（IPARAM）；
    (c) 走几何集图元（SBOX/SCYL/SEXT/SDSH/SCTO/SREV…）并读其表达式值属性（PTCA/PTMI/PTAX/Px/Py/Pz…）。
  - 机制零件（跨库 ref、typed getter、数组读）都在，缺的是把它们收拢成目录门面。
- **G3 · DbElement 的 qualifier / 下标 / UDA / owner 链属性 getter（G1 的环境后端）**
  - PML 表达式引用 `ATTRIB PARA[5]`（下标）、`ATTRIB RPRO RADI`（qualifier）、`:UDA`、
    `ATTRIB NAMN OF PSPE OF PIPE`（owner 链）——门面目前都读不出。`uda_catalog.rs` 需接进来并带 sesno。
- **G4 · P-point / 连接点求值（`ppointlib/PPEVST`）** — 管件 arrive/leave 用，后续里程碑。
- **G5 ·（非阻塞）World 定位门面** — Db.World/WorldMembers/GetFirstWorld；gen_ams 现手工扫根。
- **G6 ·（非阻塞）MDB 完整化 / 差分门面** — 生成链不消费，记账即可。

## 4. 决策点（需用户拍板，plannotator 卡点）

1. **求值路线**：(A) **对结构化形式直接数值求值**（推荐——e3d-io 已有解码器且 TTY 对过）｜
   (B) 复刻/对齐 EXEV* 字符串方言（对标矩阵旧押 修法 A，现已过时）。
2. **首个目录里程碑范围**：(A) **FITT/SBFI/FIXING 等结构目录件**（不含管件；ams1112 里 ~481 件，推荐）｜
   (B) 连管件 PIPE/BRAN + P-point 一起（更大，含 G4）。
3. **落点**：G1 求值器 + G2/G3 门面落 **e3d-io**（与 S1 一致），e3d-model 只消费。确认。
4. **验收基准**：目录几何用 **E3D TTY 导出 RVM** 对拍（与结构件同口径）｜或先只做 evaluate 的
   单元级对拍（对 E3D `Q` 数值）再上 RVM。

## 5. 分阶段开发计划

> 每阶段：交付物 + 验收判据（宪法口径：静默缺件是最高级别缺陷，evaluate 不懂必须 None 且记账）。

- **P0 · 证据与接口定形（小，无行为改动）**
  - 反编译 core.dll `EXEVRE`(0x51b485a) / `GATPAR`(0x51b228f) / `GATINS`(0x51b2e1e) 定死求值器
    的入参/出参与设计参数布局；在 ams1112 上对一个真 FITT/SBFI 坐实 SPRE/CATR→GMSET 引用链。
  - 交付：证据短记 + P1–P3 的 Rust API 签名。判据：签名评审通过。
- **P1 · 表达式求值器（G1）**
  - e3d-io 新增 `ParamEnv` trait（`param(n)/iparam(n)/design_dimension(id)/attribute(name,qual/idx,owner)`）
    + `catalogue_expr::evaluate(&expr,&env)->Option<f64>` + `catalogue_pml` 加 evaluate 通路。
  - 判据：现有 `render()` 测试语料逐条 evaluate 对上 E3D `Q` 数值；ams1112 目录元素评估维度对上 TTY。
- **P2 · DbElement 环境后端 getter（G3）**
  - `get_double_at(name,i)`（下标）/ `get_qualified(name,qual)`（qualifier）/ `get_uda(name)`（接
    `uda_catalog` 带 sesno）/ `attribute_of_owner_chain(name,[owners])`（owner 链）。
  - 判据：真语料逐属性对 TTY。
- **P3 · 目录几何读取门面（G2）+ 接进 e3d-model**
  - 给定设计元素：解 SPRE/CATR→CATA 组件→GMSET，暴露设计参数向量，产出几何集图元 + 经 P1 评估好的维度。
  - e3d-model ELMODL 目录分支消费之，真出目录件几何；`catalog_pending` 下降。
  - 判据：ams1112 代表性 FITT/SBFI/FIXING 图元清单 + 评估维度对上 E3D；RVM 对拍（按决策 4）。
- **P4 ·（后续）P-point（G4）** 管件路径。
- **P5 ·（记账）World/MDB 门面（G5/G6）** 生成链需要时再薄封装。

## 6. 非目标（记账，不做）

写侧全家族、租约并发、规则系统、表达式**字符串方言**模拟（本计划对结构化形式求值，不碰字符串）、
几何算法层（libgm/manifold 属生成算法，不是读 API）、差分主链消费。任一格日后被生成链证实要消费，翻开重判。

## 7. P0 产出（证据与接口定形，2026-08-31 落）

### 7.1 反编译坐实（core.dll，`idalib-22484`）

- **`GATPAR`（0x51b228f，`exppdms/GATPAR`）**：`*a6=0.0` 先清结果；`*a5`=参数序号（1 起，`<=0` 报 223）；
  `GATRAR(a2,a3,a4,_4C,&v12,a8)` 把设计参数读成 **double 数组**入 `_4C[205]`、个数 `v12`；
  命中即 `*a6 = _4C[2*(*a5)-2]`（double 占 2 个 dword，故 `PARAM[n]` = 数组第 n 个，1 起），
  越界 `*a8=223`。特殊 hash `886905`/`860359` 走 `RVPLG` 派生回退。→ `param(n)` = 1 起索引进 f64 向量。
- **`GATINS`（0x51b2e1e，`catdblib/GATINS`）**：函数体只做 `dword_6B2A1C4 = *a1`——**设「当前目录实例」全局**，
  之后 `GATPAR` 用它解参数（`GATPAR` 里 `*a4!=886905 || (dword_6B2A1C4&1)` 就是读这个全局）。
  → 绑定顺序 = `GATINS(instance)` 定环境，再 `GATPAR(n)` 取参数。
- **`EXEVRE`（0x51b485a，`exprlib/EXEVRE`）**：薄派发器，真求值在 `sub_51B4976`；`*a5==425` 是特殊状态，
  正常求值后 `sub_51D1C4D(a3,a4,a5)` 落结果。语义 = 「给上下文求值到实数、状态回 a5」。**我方不复刻它的字节码 VM**——
  e3d-io 已有结构化解码器，直接对结构求值即可（决策 1 = A）。

### 7.2 P1–P3 Rust API 签名（e3d-io）

```rust
// P1 · crate::record::catalogue_eval（新）——对已解码结构求值
/// 求值环境：镜像 GATINS(实例)→GATPAR(n)。不懂一律 None（响亮，不猜 0）。
pub trait ParamEnv {
    fn param(&self, n: i32) -> Option<f64>;          // PARAM n，1 起（GATPAR 语义）
    fn iparam(&self, n: i32) -> Option<f64>;         // IPARAM n
    fn design_dimension(&self, id: i32) -> Option<f64>; // DDRADIUS=3/DDANGLE=4/DDHEIGHT=5…
    fn attribute_number(&self, r: &AttribRef) -> Option<f64>; // 喂 PML 的 ATTRIB 原子
}
pub struct AttribRef<'a> {
    pub name: &'a str,
    pub subscript: Option<f64>,     // ATTRIB PARA[i]
    pub qualifier: Option<&'a str>, // ATTRIB RPRO RADI
    pub owner_chain: &'a [&'a str], // ATTRIB NAMN OF PSPE OF PIPE
}
// 元组形（catalogue_expr）：Scaled/Function(SUM·DIFFERENCE·TANF·SINF·COSF)/Named/DD 全覆盖
pub fn evaluate(expr: &CatalogueExpression, env: &dyn ParamEnv) -> Option<f64>;
// PML 形（catalogue_pml）：在现有逆波兰 render 通路旁加一条 eval 通路
pub fn evaluate(words: &[i32], env: &dyn ParamEnv) -> Option<f64>;

// P2 · DbElement 门面（ParamEnv 的后端）
impl DbElement {
    pub fn get_double_at(&self, name: &str, index: usize) -> Result<Option<f64>, DbElementError>;        // ATTRIB X[i]
    pub fn get_qualified(&self, name: &str, qualifier: &str) -> Result<Option<DescriptorValue>, DbElementError>; // ATTRIB X Y
    pub fn get_uda(&self, name: &str) -> Result<Option<DescriptorValue>, DbElementError>;                 // :UDA（接 uda_catalog + sesno）
    pub fn attribute_of_owner_chain(&self, name: &str, chain: &[&str]) -> Result<Option<DescriptorValue>, DbElementError>;
}

// P3 · crate::catalog（新）——目录几何读取，供 e3d-model ELMODL 目录分支消费
pub struct CatalogGeometry { pub primitives: Vec<CatalogPrimitive> }
pub struct CatalogPrimitive {
    pub kind: CatalogPrimKind,  // 按 I*COM 家族分类（SBOX/SCYL/SEXT/SDSH/SCTO…）
    pub transform: [f64; 12],   // D3_Transform::asArray12 局部放置
    pub dims: CatalogDims,      // 经 P1 评估好的数值维度
    pub is_negative: bool,      // 洞（LNEGIT / addHolesBelowTemplate）
}
impl DbSet {
    /// 设计元素 → SPRE/CATR → CATA 组件 → GMSET；绑定设计参数向量（GATPAR/GATINS 语义）；
    /// 逐图元经 P1 评估表达式值属性。跨库进 CATA 走既有池 + resolver。
    pub fn catalog_geometry(&self, element: &DbElement) -> Result<Option<CatalogGeometry>, DbElementError>;
}
```

### 7.3 P0 未完项（明记，折进 P1 第一步）

「在 ams1112 上对一个真 FITT/SBFI 坐实 SPRE/CATR→GMSET 引用链、确认设计参数存哪个属性」需要
构建并跑 e3d-io/e3d-model（`e3d-descriptor extract` 或 `element_probe`）连真数据、且要跨开 ams5052 目录库。
这与 P1 写 `ParamEnv` 时的首个真数据对拍**天然是同一步**（求值器要对着真参数验证），故并入 **P1 第一步**执行，
不在 P0 单独强跑（也避免在未加锁的共享 `vendor/e3d-model` 工作树里构建撞上在飞会话）。
