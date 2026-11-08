# M5（G4）写侧落点：e3d-io 的 GPWL / GPSET / GPITEM 便捷封装

源计划 Phase D、开发计划 §3.1 G4、决策 D3 a：写侧封装放 e3d-io，gen-model 只负责编排。
本文是把 D3 a 落到具体 API 的设计，动手前需要拍板三处（见 §6，均已拍板）；gen-model 侧的编排形状见 §8。

## 1. 现状：原语齐全，缺的是「分组语义」这一层

`vendor/e3d-io/src/write/transaction.rs` 已有全部底层原语，逐个核过签名：

| 原语 | 行 | 用途 |
|---|---|---|
| `Transaction::open(path, &mut TemplateProvider)` | 276 | 打开库写事务 |
| `Transaction::insert(owner, noun, name, build)` | 551 | 在 owner 下插元素；`build` 拿到 `RecordDraft` + `NounTemplate` |
| `Transaction::set(refno, name, value)` | 392 | 文本写属性 |
| `Transaction::set_hunk(refno, name, value)` | 409 | 写 chunk 型属性（hunk=true） |
| `Transaction::rename` / `move_to` / `reorder` / `delete` | 763 / 598 / 669 / 854 | 改名、换 owner、排序、删 |
| `Transaction::add_reference_member(group, member, at)` | 697 | **引用列表**成员（`member_list_is_referenced` 为真的 noun，如 `GROU`） |
| `Transaction::commit(request)` / `commit_reporting` / `stage` | 940 / 950 / 893 | 提交 / 带报告提交 / 只校验不写 |
| `RecordDraft::set_explicit_refno(hash, RefNo)` | record.rs:490 | **写 `GPREF`** 正是这一条 |
| `RecordDraft::set_int_array_hunk(hash, &[i32])` | record.rs:567 | **写 `SCOSEL`** 正是这一条 |
| `RecordDraft::set_explicit_words(hash, type_code, Vec<u32>)` | record.rs:474 | 裸字数组 |
| `RecordDraft::set_explicit_text_ascii(hash, text)` | record.rs:429 | ASCII 文本属性 |

属性名 → hash 走 `e3d_attlib::db1_hash(name)`（`transaction.rs:431` 就是这么做的），不需要硬编码 hash。

**缺口**：上层要写「一个 GPSET 收一个设计元素」时，得自己 `insert(GPSET, GPITEM)` → 拿到 refno →
`set_explicit_refno(db1_hash("GPREF"), target)` → 再管去重、跨库、成员位置。这段是分组语义，
散在调用方就会各写各的。

## 2. 建议新增的 API（e3d-io `write/group.rs`）

```rust
/// GPWL / GPSET / GPITEM 的便捷写入口。语义照 E3D 的 secondary hierarchy：
/// GPWL 下挂 GPSET，GPSET 下挂 GPITEM，GPITEM 的 GPREF 指向真身（可跨库）。
impl<'a> Transaction<'a> {
    /// 在 `world`（通常是 WORL）下建一个 GPWL。`name` 会写进 NAME 表。
    pub fn insert_gpwl(&mut self, world: RefNo, name: &str) -> Result<RefNo, TransactionError>;

    /// 在 `gpwl` 下建一个 GPSET。
    pub fn insert_gpset(&mut self, gpwl: RefNo, name: &str) -> Result<RefNo, TransactionError>;

    /// 给 `gpset` 加一个显式成员：建 GPITEM，把它的 GPREF 写成 `target`。
    /// 回 `GpsetMember::Inserted(item)`；同一 target 已在本组时不重复加，回 `GpsetMember::Existing(item)`。
    /// 两种都用 `item()` 取 GPITEM 的 refno（不是 target）。
    pub fn insert_gpset_member(
        &mut self,
        gpset: RefNo,
        target: RefNo,
        at: MemberPosition,
    ) -> Result<GpsetMember, TransactionError>;

    /// 给 `gpset` 写规则成员：SCOSEL 的 int 数组原样写进去（编码由调用方给）。
    pub fn set_gpset_rules(&mut self, gpset: RefNo, scosel: &[i32]) -> Result<(), TransactionError>;
}
```

四条都只是把已有原语串起来，不新增底层能力：

- `insert_gpwl` / `insert_gpset` = `insert(owner, noun, Some(name), |_, _| Ok(()))`；
- `insert_gpset_member` = `insert(gpset, GPITEM, None, |draft, _| draft.set_explicit_refno(db1_hash("GPREF"), target))`，
  外加「同 target 已存在就回已有的」这条去重（读一遍现有成员，比新增一次便宜）；
- `set_gpset_rules` 直接走 `set_int_array_hunk`，type-20 二进制 hunk，无文本中转、无 0x3FFF 上限
  （e3d-io `write/group.rs:110-118` → `record.rs:567-570` 的 `set_hunk_words(hash, 20, len, payload)`）。

## 3. 三个必须做对的点

**3.1 GPREF 是隐式属性，不是成员链**

`GPITEM` 是 GPSET 的成员链成员（`owner = GPSET`），真身靠 `GPREF` 隐式属性指过去。
所以「加成员」= 插一个 GPITEM + 写它的 GPREF，**不是** `add_reference_member`。
后者是 `GROU` 那种引用列表 noun 的路径（`member_list_is_referenced` 判定），GPITEM 不走这条。
搞混的后果是成员链上出现真身 refno，而 E3D 展开时找不到 GPITEM，分组变空。

**3.2 跨库 GPREF**

`target` 可以在别的库（真库样本里 GPITEM 在 250218、GPREF 指向 250209）。
`Transaction` 是按单库文件打开的（`Transaction::open(path, ...)`），所以跨库成员要：
- 在 GPITEM 所在的库里写 GPREF（refno 的 dbno 段原样带上，`RefNo` 自带）；
- 目标库不需要同时开着——`set_explicit_refno` 只写 refno 值，不校验目标存在。

但**校验目标存在**这件事得由调用方（gen-model）在写之前做，否则写进一个悬空引用，
读侧就会落到 `target_missing` 那个坏槽。这条和读侧三原因码是对偶的。

**3.3 只在副本库演练**

计划里写死了：G4 只在副本库演练，写后用 e3d-io 读回对照 `members / GPREF / SCOSEL`，
有条件再用真 E3D 开库验收。理由是 `Transaction` 是真实的库写，改坏了不可恢复。

**硬约束**：在 `encode(decode(真样本)) == 真样本` 核实之前，不许让真 E3D 打开写过 encode 输出的库。
`scosel::encode` 的往返单测只证明它与 `decode` 自洽，不证明 E3D 认它写出的字。

**3.4 跨库提交**

一个库一个 `Transaction`，按库顺序提交；某个库失败就停下，不再提交后面的库，回报已提交的库号、
失败的库号与错误，已提交的不回滚。口径：同一库内整批不写；跨库按库边界，可能停在中间。

**3.5 只有执行器能查的**

下面两条要碰写会话或 NAME 表，不在写前校验（`validate_ops`）里做：

- 库可写：`Transaction::open` 失败即整批失败，错误里带库号；
- 名字唯一：插入前查 NAME 表，重名就整批失败，回报名字和已有元素的 refno。

`MemberPosition::After` / `Before` 的锚点是成员表里的 `GPITEM`，不是 target：
- 锚点存不存在由 e3d-io 在执行期判（`check_anchor` → `TransactionError::AnchorNotAMember`），
  `validate_ops` 不查。写前查它要整组读一遍成员表，而执行期拒绝不写坏数据。
- 锚点不能是本批新建的 `GPITEM`：那个 refno 在写之前不存在，`MemberPosition` 里也没有
  「本批第几条 AddMember 的产物」这种表达。同一批里要排到新成员旁边，先提交一批、拿到
  GPITEM 的 refno，再写第二批。

## 4. 验收怎么做（不依赖真 E3D）

1. **副本库**：按 §7 把整个项目目录拷一份（不是只拷 `aps250218` / `aps250204` 两个库）。
   真库用例的打开方式是 `DirectTreeService::open("AvevaPlantSample", "ALL")`，收的是项目名，
   所以副本要放在 `DbOption.project_path` 指向的目录下、按项目名布局
   （`<project_path>/AvevaPlantSample/...`）：改的是 `project_path`，不是项目名。
2. **写**：一批 `GroupWriteOp`（形状见 §8）建一个 GPWL → 两个 GPSET：一个放两条 GPITEM
   （一个同库、一个跨库），另一个只写 SCOSEL（`scosel::encode` 的输出），提交。
3. **读回**：用现有 `DirectStore` 读同一个副本，断言：
   - `/tree/children?resolved=1` 对放 GPITEM 的那个 GPSET 返回 2 个成员，`via` 是新建的 GPITEM refno；
   - `GPREF` 读出来的 target 和写进去的一致（跨库那个也要一致）；
   - `SCOSEL` 读回的原始字与写进去的逐字节一致（同 e3d-io 的
     `scosel_reads_back_as_the_words_it_was_written_with`）；
   - `scosel::decode(读回的字) == 当初 encode 的结构`。
4. **对照**：用 `e3d-write apply --dry-run` 对同一批操作跑一遍，确认 `Transaction` 的检查与 CLI 一致
   （CLI 是 `stage` + 回滚，byte-identical 校验）。

`e3d-write` CLI 已存在（`vendor/e3d-io/src/bin/e3d-write.rs`），支持 `--dry-run`、`--report`，
可以直接拿它当对照臂，不必自己再写一套校验。

## 5. 落地顺序

1. e3d-io 加 `write/group.rs` 四条 API + 单测（用 e3d-io 自带的测试夹具，不碰真库）。
2. gen-model 侧：编排层（`group_write.rs` 的 `GroupWriteOp` / `validate_ops` / `plan_stages`，见 §8）不写库；
   执行器按 `Stage` 调 e3d-io，带 `#[ignore]` 的真库用例。
3. 副本库演练 + 读回对照。
4. 真 E3D 开库验收（有条件才做）。

## 6. 已拍板（2026-11-08）

三项都按 a，理由分别是：

**D-M5-1 = a：`insert_gpset_member` 回 GPITEM 的 refno**

调用方要能回答「这个成员是哪个 GPITEM 收的」，这正是读侧 `via` 字段的语义。回 `()` 会让调用方
再查一遍，而那一遍查的正是同一次 insert 的结果。

**D-M5-2 = a：去重放 e3d-io**

和读侧 `resolved_members_on` 的 `seen` 去重对偶——同一 target 在存储里被两个 GPITEM 引用时，
读侧只留第一次；写侧也就该拒绝造出这种状态。放 gen-model 的话每个调用方都得自己记得。
「读一遍再写」的开销只在本组已有成员时才发生，量级是单个 GPSET 的成员数，可接受。

**D-M5-3 = a：`set_gpset_rules` 收 `&[i32]`**

e3d-io 不依赖 `scosel` 的编码逻辑；反过来让 e3d-io 依赖 gen-model 是反向依赖，不可接受。
代价是 `scosel.rs` 下一轮要补 `encode(&Scosel) -> Vec<i32>`，和 `decode` 成对，
往返单测（`decode(encode(x)) == x`）钉住。

## 7. 副本库来源（已核实）

`D:\AVEVA\Projects\E3D3.1\AvevaPlantSample`，1856 个文件、2.2 GB，D 盘当前可用 97.67 GB。
`aps250204`（SCOSEL 规则组）与 `aps250218`（GPITEM 显式组）都在这个项目里，
真库用例的打开方式是 `DirectTreeService::open("AvevaPlantSample", "ALL")`（`direct_tree.rs:778`）。

演练时整个项目目录拷到 `gen-model/.scratch/m5-copy/AvevaPlantSample/`，`DbOption.project_path` 指向
`gen-model/.scratch/m5-copy/`（见 §4 第 1 步）；**不直接写原库**。

## 8. gen-model 侧编排（已拍板，2026-11-08）

落在 `src/data_interface/group_write.rs`，只描述与校验，不写库。

**形状**：`GroupWriteOp` 只有一套，容器字段是 `ContainerRef`——库里已有的元素 `Existing(RefU64)`，
或本批排在前面的第几条 `CreateWorld` / `CreateSet` 的产物 `InBatch(下标)`。`target` 仍是 `RefU64`：
成员必须是库里已有的设计元素，只有容器能是本批产物。

| op | 字段 | 对应 e3d-io |
|---|---|---|
| `CreateWorld` | `world: RefU64`（WORL）、`name` | `insert_gpwl(world, name)` |
| `CreateSet` | `gpwl: ContainerRef`、`name` | `insert_gpset(gpwl, name)` |
| `AddMember` | `gpset: ContainerRef`、`target: RefU64`、`at: MemberPosition`（默认 `Last`） | `insert_gpset_member(gpset, target, at)` |
| `SetRules` | `gpset: ContainerRef`、`scosel: Vec<i32>` | `set_gpset_rules(gpset, &scosel)` |

`CreateSet` 不带成员 / 规则：成员和规则一律走 `AddMember` / `SetRules`，同一件事只有一种写法。

**先全量校验，再写库**：

- 阶段 0（`validate_ops`，只读）：`Existing` 容器读库核对 noun（WORL / GPWL / GPSET）；`InBatch` 只核对下标
  指向的那条种类（`CreateSet` 要指 `CreateWorld`，`AddMember` / `SetRules` 要指 `CreateSet`）——产物由
  `insert_gpwl` / `insert_gpset` 建出，noun 必然对，e3d-io 的 `require_noun` 是第二道；target 读库查
  （库在 MDB、元素在、不是 GPWL / GPSET / GPITEM），本批内不重复。有一条被拒，阶段 1、2 都不执行。
- 阶段 1、2（执行器，按 `Stage`）：每个库先建容器、回填 refno，再写成员和规则。部分写入只可能来自
  执行失败（§3.4）。

原因码：`target_missing` / `db_not_in_mdb`（与读侧坏槽同名同义）、`not_a_group`（容器种类不对，含 world
不是 WORL、`InBatch` 指错）、`duplicate_member`（只管本批内同一 GPSET 下同一 target 第二次出现）、
`not_an_element`。「已在组里」不是拒绝：放行，在 `ValidatedBatch::Ready` 里标成跳过（`already_member`），
与 e3d-io `insert_gpset_member` 回 `Existing` 的幂等语义一致，重跑同一批不会失败。新组本来是空的，
只查本批内重复。

**按库拆阶段**（`plan_stages`，纯函数）：每个库一个 `Stage { dbnum, creates, fills }`，库号按 refno
位解码（`refno_route::refno_dbnum`，不是 `get_0()`），库按在批次里第一次出现的顺序排。容器在 `Stage`
里解析成 `Slot`：`Existing(refno)`，或 `Created(k)` = 本 `Stage` 第 `k` 个 create 的产物，执行器建完回填。
跨库的 `CreateSet`（父 GPWL 在库 A、子 GPSET 落库 B）不支持；这个形状也表达不出来——`CreateSet`
没有另指库的字段，GPSET 总落在它的 GPWL 所在的库。

`AddMember.at` 用 `After` / `Before` 时的锚点限制见 §3.5。
