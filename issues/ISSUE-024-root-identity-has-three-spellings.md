# Issue #024: 同一个根有三种字符串拼法，跨边界比较静默失配（同一个坑第二次）

## 📋 Issue 信息

- **Issue ID**: #024
- **类型**: Enhancement 🔧（根治一类反复出现的 Bug）
- **优先级**: High 🟠
- **状态**: Open 📝
- **创建日期**: 2026-08-27
- **相关模块**: `aios_core::RefU64` / `RefnoEnum`、`src/data_interface/model_update_pending.rs`、
  `src/fast_model/occ_generate.rs`、`src/data_interface/increment_pipeline.rs`

## 🔍 问题描述

「一个生成根」在代码里是 `String`，而同一个根有**三种拼法**同时在用：

| 拼法 | 出处 | 例 |
|---|---|---|
| `A/B` 斜杠 | `RefU64::to_pdms_str()`（`aios_core/src/types/refno.rs:467`） | `24381/100677` |
| `A_B` 下划线 | `RefnoEnum` / `RefU64` 的 `Display`（同文件 `:861` / `:320`） | `24381_100677` |
| `A_B` 下划线 | `record_id_of` 自己 `replace('/', "_")`（`model_update_pending.rs:271`） | `24381_100677` |

三者都是 `String`，编译器分不出来，比较写成 `a == b` 或 `set.contains(&x)` 一律通过。
而拼错的后果**恰好是静默的**：集合查不到就是「没有」，`DELETE` 匹配不上就是「零行」，
两者都不报错、都不告警、都长得像正常跑完。

这个坑已经踩了两次。

### 第一次：`record_id_of` 两头算出的 id 不一致

入队时算的 record id 与收口时算的不一致（dbnum 那一位上有人传了 Ref0），`DELETE` 静默命中
零行：任务清不掉、每一轮重跑一次完整生成，而日志里一切正常。修法是改成按
`(action, target_refno)` 谓词寻址，不再重算 id——记在 `model_update_pending.rs:1192-1198`
的注释里。

### 第二次：2026-08-27 现场，1462 个根空转 50 分钟

`run_regen_group` 的合批收口拿**报告侧的下划线**去查**队列侧的斜杠**：

```rust
// 报告侧：occ_generate.rs:371 —— refno: RefnoEnum，Display 是 "{get_0}_{get_1}"
let root = refno.to_string();               // "24381_100677"
report.completed.push(root);

// 队列侧：model_update_pending.rs:561 —— to_pdms_str() 是 "{get_0}/{get_1}"
target_refno: root.root.to_pdms_str(),      // "24381/100677"

// 比较：两个字符串结构上永远不可能相等
completed.contains(&job.target_refno)       // 恒 false
```

失配率取决于**行的来源**（见下面「已扫结果」第 1 条）：三条主要入队路径写的是斜杠，
它们的行一个都配不上；只有启动覆盖回填那一条当时写的是下划线，它的行反而配得上。现场
那 1462 个根落在前者。连锁反应：
`settlements` 恒为空 → `clear_regen_work_batch` 没行可删 → `record_failure` 一次都不调 →
`attempts` 永远 0 → `MAX_ATTEMPTS` 永远够不着 → 死信永远是空的。于是现场看到的是
118 页 `page_claimed=100 / page_completed=0 / remaining=1462`、每一页任务都报 `succeeded`、
`/health` 一路 `ok`、日志一个字都没有，而 CPU 一直在满负荷生成同一批根。

这行比较由 `5f7ef21f`（2026-08-26，合批生成那个特性）引入，事故发生在 08-27——不是潜伏
很久，是落地即坏，第一次跑大批就现形。

## 🔬 问题分析

### 根本原因

**根身份没有类型**。三种拼法共用 `String`，跨边界比较全靠人记得先归一，而漏一处的
反馈信号是「静默地少处理一批数据」。这与 AGENTS.md「静默失效是最高级别缺陷」直接冲突。

### 当前处置（治标，已在飞）

`root_identity_key(&str) -> String` 走一趟 `RefU64::from_str` 再 `to_string()`，所有比较点
先过它；解析失败原样返回（**不能** `unwrap_or_default()`，那会把两个不同的坏根折成同一
个零值）。另配一条源码断言守住「查 `completed` / `disposed` 之前必须过这一道」。

这挡住了第二次事故，但它仍然是一个**靠自觉调用**的归一函数：新写一处比较、或者在别的
文件里跨同一条边界，编译器不会拦。守卫断言只覆盖 `run_regen_group` 一个函数体。

### 影响范围（2026-08-27 已逐处扫过）

跨「生成器报告 ↔ 队列行 ↔ record id」边界的地方全部过了一遍，**又找到一处已经在错的**：

1. 🔴 **`sync_and_seed_model_coverage`（`:1422`）写进 `target_refno` 的是下划线**——
   `pe_thing_to_refno(..).to_string()` 是 `RefnoEnum` 的 `Display`，而增量（`:561`）、
   房间（`:591`）、级联（`:1787`）、按需生成（`on_demand_model.rs:100`）四条路都用
   `to_pdms_str()` 的斜杠。`record_id_of` 折斜杠为下划线，两种拼法算出同一个 record id，
   `render_upsert` 又是 `UPSERT {id} SET … target_refno = '…'`，所以它们**抢同一行**：
   不长重复行，但字段值取决于最后一个 upsert 的人。代价落在精确查这个字段的两处——
   `staged_settlement_revision`（`batch_worker.rs:2963`）经 `current_regen_revision` 查
   `target_refno = '<斜杠>'`，撞上下划线行就命中零行、返回 `Ok(None)`，与「本来就没有
   这行」无从区分，于是本窗口跳过收口、那个根在空闲轮里被重生成一遍，而只有 `Err`
   那一支会 warn；`retry_pending_unit` 同样按精确字段寻址，拼法不对读起来就是 404。
   **已修**：`eef945bf`（统一成斜杠 + 前提断言 + 源码守卫）。库里已存的行不迁移，
   `record_id_of` 归一保证下次入队会改写字段，从此不再被入队的行仍是旧拼法。
2. ✅ `verify_repair_jobs_page` / `verify_required_panel_geometry`：`available` 与
   `required_now` 两侧同为 `to_pdms_str()`，且 `registered_required_panels` 先过
   `RefU64::from_str` 解析（两种拼法都吃）。一致。
3. ✅ `increment_pipeline.rs`：读侧一律 `RefnoEnum::from(unit.root_refno.as_str())` 先解析，
   写侧一律 `to_pdms_str()`。一致。
4. ✅ 房间任务寻址：`to_pdms_str()`，与 ADR-010 §7 的 `(action, target)` 口径一致。
5. ✅ `record_id_of` 的两个调用点（`:287`、`:2998`）：都过归一，拼法无关。
6. ✅ `run_regen_group` 内的 `busy_roots` / `skip_generation` / `roots` 三个集合：两侧同为
   队列拼法。当前一致——但**没有任何机制保证它继续一致**，这正是本 issue 要根治的形态。
7. ⚠️ HTTP 面 `retry_pending_unit` / `current_regen_revision` 的入参由人手打进来，两种拼法
   都可能出现。第 1 条修完后行里只会存斜杠，暴露面收窄，但类型上仍然挡不住。

## 🛠️ 解决方案

### 方案概述

把「根身份」做成 newtype，让**比较只可能发生在同一种类型之间**，拼法转换变成显式的、
有名字的动作。

```rust
/// 一个生成根的身份。构造只有一条路（从 `RefU64` 解析），比较只在本类型之间发生，
/// 三种字符串拼法各自是它的一个**输出**，不再是它本身。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RootKey(RefU64);

impl RootKey {
    /// 唯一入口。解析不了就是解析不了，不给一个「看着像真的」的近似值。
    pub fn parse(raw: &str) -> Result<Self, RootKeyError>;
    /// 队列行 `target_refno` 字段的拼法（斜杠）。要拿去 SurrealQL 寻址时才调。
    pub fn queue_field(&self) -> String;
    /// 生成器报告与 record id 的拼法（下划线）。
    pub fn compact(&self) -> String;
}
```

要点：

1. **不实现** `Deref<Target = str>`、`From<String>`、`AsRef<str>`——一旦它能悄悄退化成
   `String`，这个 newtype 就白做了。
2. 两个输出方法都带名字，读代码时看得见「此刻在用哪一种拼法、为什么」。
3. `PendingModelWork::target_refno` 与 `TargetedGenerationReport::completed` 都换成
   `RootKey`，反序列化时解析一次；解析失败的行按现有纪律阻断/告警，不静默跳过。
4. `record_id_of` 的 `replace('/', "_")` 换成 `key.compact()`，第三种拼法从此不是「另一段
   代码里的巧合」而是同一个类型的同一个方法。

### 风险评估

- 改动面不小（`PendingModelWork` 是序列化类型，`target_refno` 进过 SurrealDB）。**库里已存
  的字段拼法不动**——那是数据迁移，属于另一件事；本 issue 只管内存中的类型。
- 解析失败的行现在会被显式拒绝，而不是带着坏字符串一路往下走。需要先确认库里没有历史
  遗留的不可解析 `target_refno`（`root_joins_regen_batch` 已经在拿 `RefU64::from_str` 过滤，
  说明这类行确实存在过）。

## 🧪 测试验证

1. `RootKey::parse` 对三种拼法归一到同一个值，对不同的根不归一，对坏值返回 `Err`
   （不是一个零值）。
2. `queue_field()` / `compact()` 各自的输出格式钉死，与 `to_pdms_str` / `Display` 逐字符一致。
3. 源码断言：`model_update_pending.rs` 与 `occ_generate.rs` 里不得再出现裸的
   `target_refno == ` / `completed.contains(&<String>)` 形态（仿 `concurrency.rs` 那条
   `no_hardcoded_fanout_width_survives_in_fast_model` 的按目录扫描写法）。
4. 回归：构造「报告用下划线、队列用斜杠」的一页，断言全部收口——退回 `String` 比较即红。

### 验证标准

删掉 `root_identity_key` 之后，第二次事故的形状**在编译期**就写不出来。

## 🔄 后续行动

### 立即行动

- [x] 逐处核上面「影响范围」名单（2026-08-27）：扫出第三处已经在错的（覆盖回填写的是
      下划线拼法），已修于 `eef945bf`；其余五处当前一致，第 6、7 条只是「碰巧一致」，
      没有机制保证
- [ ] `RootKey` 落地，先只在 `model_update_pending` ↔ `occ_generate` 这条边界上换掉
- [ ] 换完删掉 `root_identity_key` 与它那条调用点守卫断言（类型接管之后它们是死重）

### 预防措施

- [ ] 库里 `target_refno` 是否存在不可解析的历史行，查一次并记录
- [ ] `CONTEXT.md` 补一条术语：「根身份」只有一个类型，三种拼法是它的输出而不是它本身

### 监控计划

页级停滞告警（`model_drain.page_starved`，2026-08-27 同批加入）是这类静默失配的兜底
探测器：整页认领、零收口、待办不动连撞三页即 `/health` degraded。它不能替代类型，
但下一次再有谁把两种拼法比到一起，至少 80 秒内会有人知道。

## 📚 相关文档

- 第一次事故的处置记录：`model_update_pending.rs:1192-1198`（谓词寻址取代重算 record id）
- 第二次事故的引入点：`5f7ef21f feat(model): bounded credential-checked root generation with process-local pending`
- 术语：`CONTEXT.md`「生成根」「Ref0 库归属」
- AGENTS.md：「`Ref0` 不是 `dbnum`，`file_stem` 不是 `file_name`，观察值不是权威值」——
  本 issue 是同一条纪律在「根身份」上的第三个实例

## 🏷️ 标签

bug-class type-safety silent-failure model-generation refno
