//! 面板自身的变化 → 本库整库房间归属重算（`RoomOnly`，d-819 / ADR-073 D13）。
//!
//! 逐根路径（ADR-073 D1）只算「本根在哪些面板里」。面板那一侧变了——PANE 的几何 / 属主、
//! FRMW / SBFR 改名（房间号、进出 `room_key_word`）、`project_hd` 下 CWALL / CFLOOR 搬迁——
//! 它名下**别的根**的成员边与 `room_num` 一个都不会被碰到：这是逐根路径的天然盲区
//! （ADR-073 §未决 U1），原来填它的结构触发账本（ADR-010 D12 / ADR-040 D4）已随房间流程退役。
//!
//! 现行填法（d-819）：三条触发，同一种处置。
//!
//! 1. **规划期**：窗口规划器看本窗变更集（e3d-model 的 `ChangeLedger`），碰到房间定义族的
//!    元素（[`ROOM_DEFINITION_NOUNS`]，增 / 删 / 改属主 / 改成员 / 改属性都算）→ 给本库排一条
//!    `ModelWorkAction::RoomOnly` 工作项（[`room_only_work_item`]）。**不解析 `detail` 散文**
//!    ——DESC 之类只改元数据的面板改动会多触发一次整库重算，接受：那是 O(本库构件) 的内存判定
//!    加一批边的重写，而漏掉一次就是静默陈旧。
//! 2. **执行期**：任务开工前对齐（ADR-073 D12）换了房间代号——别的库的面板 / 房间变了、进了
//!    常驻——本库的成员边同样可能陈旧，一并重算（跨库那一半只覆盖「变了之后第一个开工的库」，
//!    §未决 U3 仍开着）。
//! 3. **电平型再武装**（2026-09-17）：库上 durable 侧的整库对齐代号（`dbnum_watermark.model_room_generation`，
//!    ADR-073 D14）与此刻常驻代号不等 = 这个库欠一次整库重算（[`library_alignment_owed`]）。前两条是
//!    边沿型——原因只活在本任务里，拆窗 / 单元失败 / 刷盘没齐 / 重算写库失败都会把它丢掉，而下一窗的
//!    账本不再含那条 PANE 变更、再探又是 `Unchanged`，边就永久陈旧到下一次不相关的 PANE 变化。这一条
//!    按库上的事实判：直到某个任务把 `Aligned` 盖上（整库重算齐了、初始化、整库重排），每个任务开工
//!    都会再点一次。整库重算对不在投影里的根回读库上几何（下段），落盘形态下也盖得上，所以这一条在每个库上
//!    只多跑一次就安静；没有别的去重。
//!
//! 处置：任务在本批根**刷盘之后、收尾之前**（`batch_worker::execute_frozen_batch`），冻一台计算器，先枚举
//! 本库 `DbSet@T` 的根清单，对**投影里**有回执的根按投影里的几何、对清单里**不在投影里**的根按库上读回的
//! 几何（`model_record_reader::durable_records_of_root`，U1 第二半）重算归属、重写两张边
//! （`model_db_adapter::recompute_dbnum_room_edges`），不重生几何、不动模型行、不动空间树。
//!
//! **定向（反向传播，2026-09-17）**：整库是兜底，不是常路。从库上的对齐代号出发，沿**换代段账本**里的段
//! （`room_resident::SwapLedger`：每一次常驻重建对着被换下的那一代算出的面板变化 `from → to → PanelDelta`，谁重建谁记
//! ——本任务自己那一次、`/model/ensure` / 初始化 lane 那一次都在；M1 2026-09-19）与别的库停放给本库的段接到本任务冻的
//! 计算器代号（[`scope_for_task`]）——那么这几段面板动了什么就是欠账的全部：不在册的面板清边、只改号的改列
//! （不查空间），几何变了 / 换了房的拿新盒反查（`room_reverse`：全局树 + 管身范围查询）谁进来、拿库上名下的成员边查谁离开，命中的根只重算它们
//! （`RootScope::Targeted`）；成功即盖 `Aligned`（命中根算齐 = 库上每条边都与这一代一致）。树不就绪 / 折不出根 /
//! 根清单枚举不出来 → 退整库。此前只认本任务自己那一次 `Rebuilt` 带回的段：ensure 先重建了 / 截断窗口丢了那一段 /
//! 没被命中的库，全 MDB 每个库各整库一次（审核 M1）。
//! **定向重算用的是本任务开工冻的那台计算器**（2026-09-19 H1，一任务一台）：准入链的终点 `resident` 就取自它
//! （[`durable_alignment`] 收它的代号），自己那一段的 `Rebuilt.generation` 不等于它 → 整库；收尾那一步再对一次
//! （`batch_worker::run_targeted_room_recompute`）。此前收尾新冻一台「此刻这一代」——scope 按 g2 定、边按 g3 算、盖 g3，
//! 中途 g2 → g3 动的面板命中的根从此永久陈旧、③ 哑掉；整库那一路不受影响（每一根都算），照旧新冻。
//! **ACK 代号不在链上的根（F3，2026-09-19）**：常驻代号是输入摘要、不单调——一块 PANE 的 `.mesh` 暂时读不出（重建 → g2，进保护集
//! 这一段是空段、不进账本）、期间 `/model/ensure` 按 g2 生成了几根（P 不在索引，进 P 的根拿不到 P 的边）、`.mesh` 恢复（重建 → g1，
//! 摘要同值）：库上 g1 == 常驻 g1，③ 不欠、账本没段，那几根的边却是 g2 算的。投影里本库每一枚 ACK 的房间代号（[`acked_roots_for_task`]）
//! 是这一段的证据：库上已与常驻同代时，代号 ≠ 常驻的根就是欠账本身——点名它们走**空段定向**
//! （`Targeted { swaps: [], parked: [], stale_roots }`，[`scope_for_task`]）；库上欠着、链接得通时，代号不在链上任一代的根一并点名
//! （它那一代与链的差链说不了）。定向成功盖章那一刻把代号在链上的 ACK 一并对齐到链尾（`model_db_adapter::align_room_acks_along_chain`），
//! 此后本库「ACK ≠ 库上代号」只剩一种含义——盖章之后另一代写的，读侧 `query_service::room_edges_ready` 拿它一票否决。
//! 算不出的根按 D2 记失败、`model_sesno` 不盖。**覆盖面要自证**：投影只装本进程生成过的根——
//! 初始化的模型行不进投影（ADR-077 D5），落盘形态冷重启后投影是空的——回读库补的正是这一半；重算完
//! 拿回执里覆盖到的根对一遍清单，每一根都在才说「整库」、durable 侧那一格「整库对齐到这一代」才盖；
//! 清单枚举不出来（覆盖面未知）只算了投影那一半、那一格**不盖**（`batch_worker::RoomRecomputeVerdict::PartialCoverage`），
//! 下一个任务由第三条触发再来。它是任务内部的一步，不入队、没有 HTTP 口（ADR-073 §不做「不给房间开新的
//! HTTP 触发口」）。

use crate::data_interface::model_update_plan::{ModelWorkAction, ModelWorkItem};
use crate::data_interface::room_reverse::RoomRoots;
use crate::fast_model::model_memory_store::{ModelMemoryStore, ProjectionScope, packed_refno};
use crate::fast_model::room_resident::RoomReferenceRefresh;
use e3d_io::refno::RefNo;
use std::collections::BTreeSet;

/// 房间定义族：房间节点（FRMW / SBFR）、`project_hd` 的容器（CWALL / CFLOOR）、面板本体（PANE）。
/// 与 `room_topology::RoomHierarchy` 两种口径的并集——按 noun 判，不分口径：不在本项目层级里的
/// noun 在本项目的文件里本来就不出现。
pub(crate) const ROOM_DEFINITION_NOUNS: [&str; 5] = ["PANE", "FRMW", "SBFR", "CWALL", "CFLOOR"];

/// 变更集里碰到房间定义族的元素了没有。`Some(原因)` 点名前几个 `noun refno（改动种类）`；
/// `None` = 这一窗一个都没碰。`Unresolved`（连是什么都读不出）的条目不算——它的 noun 是 `"?"`。
pub(crate) fn room_definition_change(entries: &[e3d_model::ledger::LedgerEntry]) -> Option<String> {
    let hits: Vec<&e3d_model::ledger::LedgerEntry> = entries
        .iter()
        .filter(|entry| entry.kind != e3d_model::ledger::ChangeKind::Unresolved)
        .filter(|entry| {
            ROOM_DEFINITION_NOUNS
                .iter()
                .any(|noun| noun.eq_ignore_ascii_case(&entry.noun))
        })
        .collect();
    if hits.is_empty() {
        return None;
    }
    let sample = hits
        .iter()
        .take(5)
        .map(|entry| {
            format!(
                "{} {}（{}）",
                entry.noun.to_ascii_uppercase(),
                entry.refno,
                entry.kind.label()
            )
        })
        .collect::<Vec<_>>()
        .join("、");
    Some(if hits.len() > 5 {
        format!("{} 个房间定义族元素变了，例如 {sample}", hits.len())
    } else {
        format!("{} 个房间定义族元素变了：{sample}", hits.len())
    })
}

/// 本库这一窗的 `RoomOnly` 工作项：`target_refno = dbnum=<n>`（不是根，一窗最多一条）。
pub(crate) fn room_only_work_item(dbnum: u32, db_type: &str, end_sesno: i32) -> ModelWorkItem {
    ModelWorkItem {
        dbnum,
        db_type: db_type.to_string(),
        source_end_sesno: end_sesno,
        action: ModelWorkAction::RoomOnly,
        target_refno: format!("dbnum={dbnum}"),
        noun: String::new(),
    }
}

/// 第三条触发（电平型，2026-09-17）的纯函数形态：库上 durable 侧的整库对齐代号
/// （`dbnum_watermark.model_room_generation`，ADR-073 D14）与此刻常驻代号不等 = 这个库欠一次整库重算。
///
/// `resident` 为 `None`（开关关着 / 常驻还在装 / `Blocked`）= 没有可对齐的目标，不触发——那种局面下
/// 计算器也冻不出，重算无从谈起。`library` 为 `None`（旧行、从没整库对齐过、关着开关生成的）当然不等：
/// 库上没有这句承诺，就欠一次；重算齐了盖上之后这一条就安静了，直到房间 / 面板真变过（代号变）。
/// 没有进程内的去重：整库重算对不在投影里的根回读库上几何（U1 第二半），落盘形态下也盖得上那一格，
/// 「同一代下反复点、每次都覆盖不全」这种局面不再有（此前为它设的 `PARTIAL_COVERAGE_SETTLED` 去重表
/// 2026-09-17 随之退役）。
pub(crate) fn library_alignment_owed(
    library: Option<&str>,
    resident: Option<&str>,
) -> Option<String> {
    let resident = resident?;
    if library == Some(resident) {
        return None;
    }
    let short = |generation: &str| generation.chars().take(12).collect::<String>();
    Some(match library {
        Some(library) => format!(
            "库上整库房间对齐代号 {} ≠ 此刻常驻代号 {}（欠一次整库重算，D14）",
            short(library),
            short(resident)
        ),
        None => format!(
            "库上没有整库房间对齐代号（从没整库对齐过 / 关着开关生成的），此刻常驻代号 {}（欠一次整库重算，D14）",
            short(resident)
        ),
    })
}

/// 库上 durable 侧的整库对齐代号（`dbnum_watermark.model_room_generation`，D14）与此刻的常驻代号——一次读数，
/// 第三条触发（[`DurableAlignment::owed`]）与定向准入（[`scope_for_task`]：库上是不是**上一代**）都从它出发。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DurableAlignment {
    /// 库上那一格；`None` = 旧行 / 从没整库对齐过 / 关着开关生成的。
    pub library: Option<String>,
    /// 此刻（开工前对齐之后）的常驻代号。
    pub resident: String,
    /// 库上那一格盖章**之后**本库有根按哪几代写过边（`dbnum_watermark.model_room_stray_generations`，F3 落库 2026-09-19）：
    /// 刷盘一组按另一代写边时随那一组同事务记下、盖章时清空。库上 == 常驻时 ③ 不欠，这一格替重启后丢了 ACK 的那几根说话。
    pub stray: Vec<String>,
}

impl DurableAlignment {
    /// 第三条触发（电平型）：库上那一格与常驻代号不等就欠；相等时库上记着盖章之后按另一代写过根（F3 落库）也欠。
    pub(crate) fn owed(&self) -> Option<String> {
        library_alignment_owed(self.library.as_deref(), Some(self.resident.as_str()))
            .or_else(|| stray_generations_owed(self.library.as_deref(), &self.stray))
    }
}

/// F3 落库那一格的纯函数形态：库上记着「盖章之后按这几代写过根」（去掉与库上同代的——那几根的边与盖章一致），
/// 非空 = 欠一次。库上 == 常驻时 ③ 不欠、账本没段、进程内 ACK 可能已随重启丢了，只有这一格还说得出「有根的边是另一代算的」；
/// 欠的范围由 [`scope_for_task`] 定——进程内还有 ACK 就点名（空段定向），没有就整库（多算不漏算），盖章即清。
pub(crate) fn stray_generations_owed(library: Option<&str>, stray: &[String]) -> Option<String> {
    let short = |generation: &str| generation.chars().take(12).collect::<String>();
    let strays: Vec<String> = stray
        .iter()
        .map(String::as_str)
        .filter(|generation| Some(*generation) != library)
        .map(short)
        .collect();
    if strays.is_empty() {
        return None;
    }
    Some(format!(
        "库上记着盖章之后本库有根按另 {} 代写过边（{}），那几根的边不是库上这一代算的（欠一次，F3 落库）",
        strays.len(),
        strays.join("、")
    ))
}

/// 第三条触发的取数：库上那一格与两枚水位同一行（`DbnumState::read`），常驻代号是**本任务冻好的那台计算器**的
/// （`TaskRoomCalculator::generation`，2026-09-19 H1）——不再另读一次生命周期：对齐、冻计算器、读代号是三次独立的读，
/// `/model/ensure` 并发对齐可以插在任意两次之间换代；准入链的终点与收尾定向重算用的计算器必须是同一代，所以终点
/// 从那台计算器上取。冻不出 / 开关关（`None`）= 没有可对齐的目标，直接 `None`；水位行读不出**不**触发（读错不是
/// 「欠着」的证据，只留一句 warn）——那一轮也说不出库上是哪一代，定向准入随之不成立。
pub(crate) async fn durable_alignment(
    dbnum: u32,
    resident: Option<&str>,
) -> Option<DurableAlignment> {
    let resident = resident?;
    let (library, stray) = match crate::data_interface::dbnum_state::DbnumState::read(dbnum).await {
        Ok(state) => state
            .map(|state| {
                (
                    state.model_room_generation,
                    state.model_room_stray_generations,
                )
            })
            .unwrap_or_default(),
        Err(error) => {
            log::warn!(
                "dbnum={dbnum} 读不出水位行上的整库房间对齐代号，这一轮不按「欠整库重算」处理: {error:#}"
            );
            return None;
        }
    };
    Some(DurableAlignment {
        library,
        resident: resident.to_owned(),
        stray,
    })
}

/// 一段换代（`from → to → 面板动了什么`）；定义在常驻模型那边（谁重建谁记，`room_resident::SwapLedger`），这里只是
/// 准入链的一种段。
pub(crate) use crate::fast_model::room_resident::PanelSwap;

/// 这个任务的房间重算要算多大范围（ADR-073 D13 反查，2026-09-17）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RoomRecomputeScope {
    /// 整库：`DbSet@T` 的每一根（今天的 D13）。
    WholeLibrary,
    /// 定向（反向传播）：库上那一代 → 本任务冻的计算器这一代之间面板动了什么就是本库的全部欠账——换代段账本里的段
    /// （`swaps`，每段反查命中的根）加上别的库停放给本库的根级工作项（`parked`，U3 b 档），只重算它们；
    /// 面板级的两类（不在册 / 只改号）不查空间直接写。
    Targeted {
        /// 链上要反查的换代段（链序）：本任务自己那一次重建的、`/model/ensure` / 初始化 lane 重建时记的都在账本里
        /// （M1，2026-09-19）；空 = 欠账全在停放链上。
        swaps: Vec<PanelSwap>,
        /// 别的库停放给本库的（链序）。随 U3-3 接进准入链、U3-4 消费。
        parked: Vec<RoomRoots>,
        /// 投影里 ACK 的房间代号**不在准入链上任一代**的根（F3，2026-09-19；打包 refno 序、去重）：它们的边是链外那一代算的
        /// （库上盖章之后中间那一代 `/model/ensure` 生成的，那一段是空段没进账本），链上各段的反查说不了它那一代与链的差，
        /// 点名重算。库上已与常驻同代时链长 0，这一格就是欠账的全部（`swaps` / `parked` 都空——空段定向）。
        stale_roots: Vec<RefNo>,
        /// 准入链的终点 = 本任务冻的计算器代号（`DurableAlignment::resident`）：收尾的定向重算拿它对那台计算器，
        /// 不等就不是同一段欠账、退整库（H1）。
        generation: String,
    },
}

impl RoomRecomputeScope {
    /// 定向那一档准入链的终点代号（= 本任务冻的计算器代号）；整库 = `None`。
    pub(crate) fn targeted_generation(&self) -> Option<&str> {
        match self {
            Self::WholeLibrary => None,
            Self::Targeted { generation, .. } => Some(generation),
        }
    }
}

/// 这个任务要跑的房间重算：为什么（三条触发合成一句）与算多大。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoomRecomputeRequest {
    pub reason: String,
    pub scope: RoomRecomputeScope,
}

/// 准入链上的一段：换代段（要反查）或停放段（元素已在手，折根即可）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum ChainSegment<'a> {
    Swap(&'a PanelSwap),
    Parked(&'a RoomRoots),
}

impl<'a> ChainSegment<'a> {
    fn from_generation(&self) -> &'a str {
        match self {
            Self::Swap(swap) => &swap.from_generation,
            Self::Parked(entry) => &entry.from_generation,
        }
    }

    fn to_generation(&self) -> &'a str {
        match self {
            Self::Swap(swap) => &swap.to_generation,
            Self::Parked(entry) => &entry.to_generation,
        }
    }
}

/// 投影里本库一根的 ACK 说它的边是哪一代算的（F3 的证据，[`acked_roots`]）：`room_generation` = `Some(代号)`
/// （发布时算了房间、ACK 带代号）；`None` = 发布时没算房间（开关关着 / 计算器没在）——同样不是常驻这一代算的。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AckedRoot {
    pub root: RefNo,
    pub room_generation: Option<String>,
}

/// 投影里本库每一根的 ACK 房间代号（F3）：只收「回执与 ACK 对得上」的根——候选前移了的（在途重生成）由它自己的刷盘另记
/// 新 ACK，点名它是白算一次；`evict_missing_root` 之后只剩 ACK 没回执的（源上已没有的根）也不收。打包 refno 序（`roots()` 的序）。
pub(crate) fn acked_roots(store: &ModelMemoryStore, dbnum: u32) -> Vec<AckedRoot> {
    let scope = ProjectionScope::Current;
    store
        .roots(scope)
        .into_iter()
        .filter(|root| root.dbno() == dbnum)
        .filter_map(|root| {
            let receipt = store.receipt(scope, root)?;
            let ack = store.publication_ack(scope, root)?;
            if ack.dbnum != dbnum || !ack.matches(&receipt) {
                return None;
            }
            Some(AckedRoot {
                root,
                room_generation: if ack.room_membership {
                    ack.room_generation
                } else {
                    None
                },
            })
        })
        .collect()
}

/// [`acked_roots`] 的执行体取数：两个执行体在读过库上那一格之后调。说不出常驻代号 / 库上那一格读不出（`alignment` 为 `None`）
/// 就没有可比对的目标，不取。
pub(crate) fn acked_roots_for_task(
    dbnum: u32,
    alignment: Option<&DurableAlignment>,
) -> Vec<AckedRoot> {
    if alignment.is_none() {
        return Vec::new();
    }
    acked_roots(ModelMemoryStore::global(), dbnum)
}

/// ACK 房间代号不在 `on_chain` 里任一代的根（没算房间的 `None` 也算），打包 refno 序、去重。
fn stale_acked_roots(acked: &[AckedRoot], on_chain: &BTreeSet<&str>) -> Vec<RefNo> {
    let mut stale: Vec<RefNo> = acked
        .iter()
        .filter(|entry| {
            !entry
                .room_generation
                .as_deref()
                .is_some_and(|generation| on_chain.contains(generation))
        })
        .map(|entry| entry.root)
        .collect();
    stale.sort_by_key(|root| packed_refno(*root));
    stale.dedup();
    stale
}

/// 定向准入（纯函数，链式）。只有「库上那一代 → 本任务冻的计算器这一代之间，面板动了什么本进程**全知道**」时才定向：
/// 从库上 `model_room_generation` 出发，每一步接一段——换代段账本里的一段（`PanelSwap::from → to`，谁重建谁记，
/// 本任务自己那一次重建的也在里面；M1 2026-09-19）或别的库停放给本库的一段（`RoomRoots::from → to`，U3 b 档）
/// ——接到 `resident`（本任务冻的计算器代号）就是定向：链上各段的欠账（换代段反查、停放的元素折根）加起来就是
/// 本库欠账的全部，收尾用那一台计算器算每一根、盖它的代号。
///
/// 规则：同一个起点上**停放段优先**（元素是别的库反查时已经找出来的、免一次空间查询；同两代的换代段是同一个问题的
/// 另一种问法，答案等价），再看换代段；深度优先找第一条到得了 `resident` 的路，绕圈（面板挪走又挪回成环）靠已访问集
/// 挡；一条都接不上 = 整库。终点必须**恰是** `resident`：对齐装上 g2、冻计算器时常驻已是 g3（`/model/ensure` 插在两次
/// 读之间）时，账本里若有 g2 → g3 那一段就接上、两段都反查、按 g3 算；没有就整库（H1）——不拿 g2 的段冒充到 g3。
///
/// 其余都整库：库上从没盖过（欠的不止这几段，diff 说不了）、常驻说不出代号、账本与槽都接不出链（重启后都丢了）、
/// 换了代而面板变化为空的那一段没进账本（换代另有原因，例如关键字——这一段接不上）、库上已是这一代却被问到
/// （规划器排了 `RoomOnly`）而投影里没有一枚代号不等的 ACK。
///
/// **`acked` 是投影里本库每一根 ACK 的房间代号（F3，2026-09-19）**：库上已与常驻同代时，代号 ≠ 这一代的根就是欠账本身
/// （盖章之后中间那一代 `/model/ensure` 生成的——那一段是空段没进账本、③ 又不欠），点名它们走空段定向；链接得通时，代号不在
/// 链上任一代的根一并点名——链上各段说的是「这几代之间面板动了什么」，它那一代与链的差链说不了。代号在链上的根不点名：它那一代
/// 到链尾之间面板动了什么各段都反查过，没命中就是边没变。
pub(crate) fn scope_for_task(
    library_generation: Option<&str>,
    resident_generation: Option<&str>,
    swaps: &[PanelSwap],
    parked: &[RoomRoots],
    acked: &[AckedRoot],
) -> RoomRecomputeScope {
    let (Some(library), Some(resident)) = (library_generation, resident_generation) else {
        return RoomRecomputeScope::WholeLibrary;
    };
    if library == resident {
        // 库上已是这一代：链上没有欠账。投影里 ACK 代号 ≠ 这一代的根是唯一说得清的欠账（F3）——只算它们；一根都没有
        // 而被问到（规划器排了 `RoomOnly` / 本任务刚换回这一代）按整库办，与今天同。
        let stale_roots = stale_acked_roots(acked, &BTreeSet::from([resident]));
        if stale_roots.is_empty() {
            return RoomRecomputeScope::WholeLibrary;
        }
        return RoomRecomputeScope::Targeted {
            swaps: Vec::new(),
            parked: Vec::new(),
            stale_roots,
            generation: resident.to_owned(),
        };
    }
    let mut visited = BTreeSet::new();
    let mut path: Vec<ChainSegment<'_>> = Vec::new();
    if !walk_chain(library, resident, swaps, parked, &mut visited, &mut path) {
        return RoomRecomputeScope::WholeLibrary;
    }
    let mut on_chain: BTreeSet<&str> = BTreeSet::from([library, resident]);
    let mut used_swaps = Vec::new();
    let mut used_parked = Vec::new();
    for segment in path {
        on_chain.insert(segment.from_generation());
        on_chain.insert(segment.to_generation());
        match segment {
            ChainSegment::Swap(swap) => used_swaps.push(swap.clone()),
            ChainSegment::Parked(entry) => used_parked.push(entry.clone()),
        }
    }
    RoomRecomputeScope::Targeted {
        swaps: used_swaps,
        parked: used_parked,
        stale_roots: stale_acked_roots(acked, &on_chain),
        generation: resident.to_owned(),
    }
}

/// 深度优先：从 `cursor` 出发沿段走到 `target`，走到了 `path` 里就是链序的各段。同一起点停放段先试；已访问的代号不再进。
fn walk_chain<'a>(
    cursor: &str,
    target: &str,
    swaps: &'a [PanelSwap],
    parked: &'a [RoomRoots],
    visited: &mut std::collections::BTreeSet<String>,
    path: &mut Vec<ChainSegment<'a>>,
) -> bool {
    if cursor == target {
        return true;
    }
    if !visited.insert(cursor.to_owned()) {
        return false;
    }
    let candidates = parked
        .iter()
        .filter(|entry| entry.from_generation == cursor)
        .map(ChainSegment::Parked)
        .chain(
            swaps
                .iter()
                .filter(|swap| swap.from_generation == cursor && !swap.panels.is_empty())
                .map(ChainSegment::Swap),
        );
    for segment in candidates {
        path.push(segment.clone());
        if walk_chain(
            segment.to_generation(),
            target,
            swaps,
            parked,
            visited,
            path,
        ) {
            return true;
        }
        path.pop();
    }
    false
}

/// 换代段账本（M1）的取数，两个执行体在读过库上那一格之后调：库上欠着（③）才拿账本里的段去接准入链；不欠 / 说不出
/// 不取。账本是进程一份、不按库清——段说的是常驻换了什么，与哪个库无关。
pub(crate) fn swaps_for_task(alignment: Option<&DurableAlignment>) -> Vec<PanelSwap> {
    match alignment.map(DurableAlignment::owed) {
        Some(Some(_)) => crate::fast_model::room_resident::swap_segments(),
        _ => Vec::new(),
    }
}

/// 停放链（U3 b 档）的取数，两个执行体在开工前对齐、读过库上那一格之后调：库上欠着（③）才拿本库槽里的停放物去接
/// 准入链；库上已与常驻同代——停放物说的都是它已越过的代——顺手清掉（回执一句）；说不出（开关关 / 常驻没就绪 /
/// 水位读不出）不动槽、也不接链。
pub(crate) fn parked_for_task(dbnum: u32, alignment: Option<&DurableAlignment>) -> Vec<RoomRoots> {
    match alignment.map(DurableAlignment::owed) {
        Some(Some(_)) => crate::data_interface::room_reverse::pending(dbnum),
        Some(None) => {
            let cleared = crate::data_interface::room_reverse::clear(dbnum);
            if cleared > 0 {
                println!(
                    "dbnum={dbnum} 库上房间对齐代号已与常驻同代，槽里 {cleared} 段停放物说的都是越过的代，清掉"
                );
            }
            Vec::new()
        }
        None => Vec::new(),
    }
}

/// 这个任务要不要跑房间重算、为什么、算多大。三条触发合成一句：规划器排的 `RoomOnly` 工作项
/// （本窗碰了房间定义族）、开工前对齐换了房间代号、库上整库对齐代号欠着（[`DurableAlignment::owed`]）；第四句是
/// F3——投影里有 ACK 房间代号不在准入链上的根（`acked`，[`acked_roots_for_task`]），库上与常驻同代、前三条都没点亮时
/// 单独也触发（空段定向）。都没有 = `None`。范围由 [`scope_for_task`] 定（账本里的换代段 `swaps` + 停放链 `parked` +
/// 点名的 `stale_roots`）；库上已与常驻同代却被 ①/② 问到仍按整库办——它们说不出欠什么，整库覆盖 F3 点名的那几根。
/// 定向里接了哪几段、点名了几根，原因各多一句。
pub(crate) fn request_for_task(
    side_actions: &[ModelWorkItem],
    refresh: Option<&RoomReferenceRefresh>,
    alignment: Option<&DurableAlignment>,
    swaps: &[PanelSwap],
    parked: &[RoomRoots],
    acked: &[AckedRoot],
) -> Option<RoomRecomputeRequest> {
    let mut reasons = Vec::new();
    if side_actions
        .iter()
        .any(|item| item.action == ModelWorkAction::RoomOnly)
    {
        reasons.push("本窗变更集碰到房间定义族（PANE / FRMW / SBFR / CWALL / CFLOOR）".to_string());
    }
    if let Some(RoomReferenceRefresh::Rebuilt {
        generation_changed: true,
        diff,
        ..
    }) = refresh
    {
        reasons.push(format!(
            "开工前对齐换了房间代号{}",
            diff.as_deref()
                .map(|diff| format!("（在册视图之差 {diff}）"))
                .unwrap_or_default()
        ));
    }
    if let Some(owed) = alignment.and_then(DurableAlignment::owed) {
        reasons.push(owed);
    }
    // 库上已与常驻同代却被 ①/② 问到：它们说不出欠什么，整库（与今天同；F3 点名的根整库也覆盖）。其余由准入链定，
    // 投影里 ACK 代号不在链上的根一并点名（F3）——库上与常驻同代、前三条都没点亮时，它单独就是一条触发。
    let library_current = alignment.is_some_and(|alignment| alignment.owed().is_none());
    let scope = if library_current && !reasons.is_empty() {
        RoomRecomputeScope::WholeLibrary
    } else {
        scope_for_task(
            alignment.and_then(|alignment| alignment.library.as_deref()),
            alignment.map(|alignment| alignment.resident.as_str()),
            swaps,
            parked,
            acked,
        )
    };
    if let RoomRecomputeScope::Targeted {
        swaps: used_swaps,
        parked: used,
        stale_roots,
        ..
    } = &scope
    {
        if !stale_roots.is_empty() {
            reasons.push(format!(
                "投影里 {} 根的 ACK 房间代号不在准入链上（库上盖章之后中间那一代生成的，F3），点名重算：例如 {}",
                stale_roots.len(),
                stale_roots
                    .iter()
                    .take(5)
                    .map(RefNo::to_string)
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
        if !used_swaps.is_empty() {
            reasons.push(format!(
                "沿换代段账本接了 {} 段：{}",
                used_swaps.len(),
                used_swaps
                    .iter()
                    .map(PanelSwap::summary)
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
        if !used.is_empty() {
            reasons.push(format!(
                "别的库的面板变化停放到本库 {} 段：{}",
                used.len(),
                used.iter()
                    .map(RoomRoots::summary)
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
    }
    if reasons.is_empty() {
        return None;
    }
    Some(RoomRecomputeRequest {
        reason: reasons.join("；"),
        scope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::room_resident::PanelDelta;
    use e3d_model::ledger::{ChangeKind, LedgerEntry};

    fn entry(noun: &str, kind: ChangeKind, id: u32) -> LedgerEntry {
        LedgerEntry {
            refno: RefNo::new(24383, id),
            noun: noun.into(),
            kind,
            detail: String::new(),
            preempted_by: None,
        }
    }

    /// 房间定义族任一 noun、任一种改动都触发；别的 noun 不触发；读不出的条目不算。
    #[test]
    fn a_change_to_any_room_definition_noun_triggers_and_nothing_else_does() {
        assert!(room_definition_change(&[]).is_none());
        assert!(
            room_definition_change(&[
                entry("BRAN", ChangeKind::AttributesModified, 1),
                entry("EQUI", ChangeKind::Reparented, 2),
                entry("ZONE", ChangeKind::MembersChanged, 3),
            ])
            .is_none(),
            "构件 / 容器变了是逐根路径的事，不是房间定义"
        );
        assert!(
            room_definition_change(&[entry("?", ChangeKind::Unresolved, 4)]).is_none(),
            "连是什么都读不出的条目不算"
        );
        for (noun, kind) in [
            ("PANE", ChangeKind::AttributesModified),
            ("pane", ChangeKind::Created),
            ("FRMW", ChangeKind::AttributesModified),
            ("SBFR", ChangeKind::Reparented),
            ("CWALL", ChangeKind::MembersChanged),
            ("CFLOOR", ChangeKind::Deleted),
        ] {
            let reason = room_definition_change(&[
                entry("BRAN", ChangeKind::AttributesModified, 1),
                entry(noun, kind, 7),
            ])
            .unwrap_or_else(|| panic!("{noun} {kind:?} 必须触发"));
            assert!(
                reason.contains(&noun.to_ascii_uppercase()) && reason.contains("24383/7"),
                "{reason}"
            );
        }
    }

    /// 原因点名前 5 个，多了说总数。
    #[test]
    fn the_reason_names_a_bounded_sample() {
        let entries: Vec<_> = (0..8)
            .map(|id| entry("PANE", ChangeKind::AttributesModified, id))
            .collect();
        let reason = room_definition_change(&entries).unwrap();
        assert!(
            reason.starts_with("8 个房间定义族元素变了，例如 "),
            "{reason}"
        );
        assert_eq!(reason.matches("PANE 24383/").count(), 5, "{reason}");
    }

    /// 工作项：`RoomOnly`，目标是库不是根，一窗一条（同键去重）。
    #[test]
    fn the_room_only_item_targets_the_library_not_a_root() {
        let item = room_only_work_item(7997, "DESI", 163);
        assert_eq!(item.action, ModelWorkAction::RoomOnly);
        assert_eq!(item.target_refno, "dbnum=7997");
        assert_eq!((item.dbnum, item.source_end_sesno), (7997, 163));
        assert!(item.noun.is_empty());
        assert!(
            !item.action.is_room_recalc(),
            "它要随 side_actions 交给任务，不能被 `is_room_recalc` 那道旧过滤拦下"
        );
    }

    /// 源码钉（d-819 / ADR-073 D13 / 源码钉 17）：接线的三处各在其位——规划器从账本判房间定义族并排
    /// `RoomOnly`；执行体把「要不要」交出去；收尾段在**刷盘之后、收尾之前**、`flush.all_acked()` 门内跑，
    /// 跑不齐就扣 `all_roots_ok`。
    /// 从 `start` 起到第一行只有 `}` 的地方（LF / CRLF 都认）。
    fn function_body<'a>(source: &'a str, start: &str) -> &'a str {
        let body = source.split_once(start).expect("function start").1;
        let end = body
            .find("\n}\r\n")
            .or_else(|| body.find("\n}\n"))
            .expect("function end");
        &body[..end]
    }

    #[test]
    fn the_room_only_recompute_is_wired_after_the_durable_barrier_and_before_task_end() {
        let body = function_body(
            include_str!("window_root_plan.rs"),
            "pub(crate) async fn build_model_update_plan_from_window(",
        );
        assert!(
            body.contains("room_definition_change(")
                && body.contains("sources.plan.ledger.entries()"),
            "规划器从本窗账本判房间定义族: {body}"
        );
        assert!(body.contains("room_only_work_item("), "排 RoomOnly 工作项");

        let worker = include_str!("batch_worker.rs");
        let executor = worker
            .split_once("async fn execute_frozen_batch_body(")
            .expect("执行体")
            .1
            .split_once("fn batch_regen_is_allowed(")
            .expect("执行体边界")
            .0;
        assert!(
            executor.contains("room_recompute::request_for_task("),
            "执行体把「要不要整库重算」交出去: {executor}"
        );
        // 第三条触发（电平型）：两个执行体都在开工前对齐**之后**、冻好本任务的计算器之后读库上那一格、喂给
        // `request_for_task`。顺序要紧——对齐之前读到的常驻代号是上一代，比对会白点一次（或漏点一次）；常驻代号
        // 从冻好的那台取（`rooms.generation()`，H1 2026-09-19），不另读生命周期——准入链的终点与收尾定向重算用的
        // 计算器必须是同一台。
        for (label, body) in [
            ("批次执行体", executor),
            (
                "ModelOnly 执行体",
                function_body(worker, "async fn execute_model_only_body("),
            ),
        ] {
            let refresh = body
                .find("refresh_room_reference_before_task(job.dbnum)")
                .unwrap_or_else(|| panic!("{label}: 开工前对齐"));
            let frozen = body
                .find("TaskRoomCalculator::freeze()")
                .unwrap_or_else(|| panic!("{label}: 一任务一台，对齐之后冻一次"));
            let owed = body
                .find("room_recompute::durable_alignment(job.dbnum, rooms.generation())")
                .unwrap_or_else(|| {
                    panic!("{label}: 要拿本任务冻的计算器代号读库上的整库对齐代号（第三条触发 / 定向准入）: {body}")
                });
            let parked = body
                .find("room_recompute::parked_for_task(job.dbnum, alignment.as_ref())")
                .unwrap_or_else(|| panic!("{label}: 停放链（U3 b 档）按库上那一格取: {body}"));
            let swaps = body
                .find("room_recompute::swaps_for_task(alignment.as_ref())")
                .unwrap_or_else(|| panic!("{label}: 换代段账本（M1）按库上那一格取: {body}"));
            // F3（2026-09-19）：投影里本库每一根 ACK 的房间代号也在读过库上那一格之后取、喂给 `request_for_task`——
            // 库上已与常驻同代时代号不等的根就是欠账本身（空段定向），链接得通时代号不在链上的根一并点名。
            let acked = body
                .find("room_recompute::acked_roots_for_task(job.dbnum, alignment.as_ref())")
                .unwrap_or_else(|| {
                    panic!("{label}: 投影里 ACK 的房间代号（F3）按库上那一格取: {body}")
                });
            let request = body
                .find("room_recompute::request_for_task(")
                .unwrap_or_else(|| panic!("{label}: request_for_task"));
            assert!(
                refresh < frozen
                    && frozen < owed
                    && owed < parked
                    && parked < swaps
                    && swaps < acked
                    && acked < request,
                "{label}: 对齐 → 冻计算器 → 读库上那一格 → 取停放链 → 取账本 → 取 ACK 代号 → 合成触发，顺序不能乱"
            );
            assert!(
                body.contains("alignment.as_ref(),")
                    && body.contains("&swaps,")
                    && body.contains("&parked,")
                    && body.contains("&acked,"),
                "{label}: 库上那一格、账本段、停放链与 ACK 代号要真的传进 request_for_task（三条触发、定向准入与 F3 点名都从它出发）: {body}"
            );
            assert!(
                body.contains("room_calculator: rooms.frozen().cloned(),"),
                "{label}: 冻的那台随回执交给收尾段，定向重算复用它（H1）: {body}"
            );
            assert!(
                !body.contains("resident_room_generation()"),
                "{label}: 常驻代号只从冻好的那台取，不另读生命周期（H1）"
            );
        }
        // ModelOnly 执行体同样：启动追平窗口与稳态窗口共用同一条规划入口，`RoomOnly` 工作项随
        // `side_actions` 进来，这里此前硬编码 `room_recompute: None`，停机期间的 PANE 改动被静默吃掉。
        let model_only = function_body(worker, "async fn execute_model_only_body(");
        assert!(
            model_only.contains("room_recompute::request_for_task(")
                && model_only.contains("Some(&room_reference)"),
            "ModelOnly 执行体也要把「要不要整库重算」交出去（三条触发都带上）: {model_only}"
        );
        // 规划失败的早退（模型半边没跑）仍是 `None`；跑完的那一份回执必须接住上面那句的结果。
        assert_eq!(
            model_only.matches("room_recompute: None").count(),
            1,
            "只有规划失败的早退才硬编码不重算: {model_only}"
        );
        assert!(
            model_only.contains("whole_library_model_only(job)"),
            "整库重排（起点 0）每一根都刚算过，不再空转一遍: {model_only}"
        );
        // 栅栏（等刷盘）自 2026-09-18 起在追平门之外，房间重算与模型收尾判决仍在追平门之内；
        // 三者的先后不变：刷盘之后、判决之前。
        let tail = worker
            .split_once(
                "let flush = if model_half_awaits_durable(initialization_closed_task, &job.db_type)",
            )
            .expect("DESI durable 栅栏")
            .1
            .split_once("registry.set_stage(&job.task_id, \"task_end\")")
            .expect("收尾那一笔")
            .0;
        let flush = tail.find("await_units_durable(").expect("先等刷盘");
        let rooms = tail
            .find("run_room_recompute(")
            .expect("房间重算（定向 / 整库）在收尾段，带着 T 好对 DbSet@T 的根清单自证覆盖面");
        let outcome = tail.find("model_outcome_of(flush").expect("模型收尾判决");
        assert!(
            flush < rooms && rooms < outcome,
            "刷盘之后、模型收尾判决之前: {tail}"
        );
        assert!(
            tail.contains("let task_rooms = result.room_calculator.clone();")
                && tail.contains("task_rooms.as_ref(),")
                && tail.contains("file_latest_sesno,"),
            "分派带着本任务开工冻的那台计算器（定向复用它，H1）与 T: {tail}"
        );
        assert!(
            tail.contains("Some(request) if flush.all_acked()"),
            "刷盘没齐就不跑（那批根已判死）: {tail}"
        );
        assert!(
            tail.contains("outcome.all_roots_ok = false"),
            "算不齐扣 all_roots_ok、水位不盖: {tail}"
        );
        // 覆盖面不全（投影里没有本库全部根）只算「触到的根更新了」：durable 侧「整库对齐到这一代」那一格
        // 不许盖——盖了就是替没算过的根说话，正是 D14 要挡的「拿上一代的边冒充这一版」。
        assert!(
            tail.contains("RoomRecomputeVerdict::library_generation")
                && tail.contains("RoomRecomputeVerdict::rooms_ok"),
            "收尾要分开问两句：算齐了没有（水位）、覆盖整库了没有、对齐到哪一代（对齐代号）: {tail}"
        );

        // 旧过滤（「房间重算项不在内」）不得把 RoomOnly 拦在 side_actions 之外。
        let manual = include_str!("manual_update.rs");
        assert!(
            manual.contains("&& !item.action.is_room_recalc()"),
            "side_actions 的过滤只拦 RoomRecalc*，RoomOnly 经它交给任务"
        );
    }

    fn alignment(library: Option<&str>, resident: &str) -> DurableAlignment {
        DurableAlignment {
            library: library.map(str::to_owned),
            resident: resident.to_owned(),
            stray: Vec::new(),
        }
    }

    /// 三条触发各自成句，都没有就是 `None`；代号没变的重建不触发。
    #[test]
    fn a_task_recomputes_rooms_on_either_trigger() {
        let item = room_only_work_item(7997, "DESI", 163);
        assert!(request_for_task(&[], None, None, &[], &[], &[]).is_none());
        assert!(
            request_for_task(
                &[],
                Some(&RoomReferenceRefresh::Rebuilt {
                    previous_key: Some("a".into()),
                    key: "b".into(),
                    generation_changed: false,
                    generation: "g".into(),
                    previous_generation: Some("g".into()),
                    diff: None,
                    panels: None,
                    elapsed_ms: 1,
                }),
                None,
                &[],
                &[],
                &[],
            )
            .is_none(),
            "文件前进但房间 / 面板没变：代号不变，不重算"
        );
        assert!(
            request_for_task(
                &[],
                Some(&RoomReferenceRefresh::Unchanged { key: "a".into() }),
                Some(&alignment(Some("g"), "g")),
                &[],
                &[],
                &[],
            )
            .is_none(),
            "库上与常驻同一代：不欠"
        );
        let planned =
            request_for_task(std::slice::from_ref(&item), None, None, &[], &[], &[]).unwrap();
        assert!(planned.reason.contains("房间定义族"), "{planned:?}");
        assert_eq!(planned.scope, RoomRecomputeScope::WholeLibrary);
        let swapped = request_for_task(
            &[],
            Some(&RoomReferenceRefresh::Rebuilt {
                previous_key: None,
                key: "b".into(),
                generation_changed: true,
                generation: "b-gen".into(),
                previous_generation: None,
                diff: Some("removed=1 recalc=0".into()),
                panels: None,
                elapsed_ms: 1,
            }),
            None,
            &[],
            &[],
            &[],
        )
        .unwrap();
        assert!(
            swapped.reason.contains("换了房间代号") && swapped.reason.contains("removed=1"),
            "{swapped:?}"
        );
        // 第三条：库上欠着，原因原样带进来；单独一条也触发。
        let owed = request_for_task(
            &[],
            None,
            Some(&alignment(Some("old"), "new")),
            &[],
            &[],
            &[],
        )
        .unwrap();
        assert!(owed.reason.contains("old ≠ 此刻常驻代号 new"), "{owed:?}");
        assert_eq!(owed.scope, RoomRecomputeScope::WholeLibrary);
        let all = request_for_task(
            std::slice::from_ref(&item),
            Some(&RoomReferenceRefresh::Rebuilt {
                previous_key: None,
                key: "b".into(),
                generation_changed: true,
                generation: "new".into(),
                previous_generation: None,
                diff: None,
                panels: None,
                elapsed_ms: 1,
            }),
            Some(&alignment(None, "new")),
            &[],
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(
            all.reason.matches("；").count(),
            2,
            "三条触发合成一句: {all:?}"
        );
    }

    fn swap(from: &str, to: &str, panels: PanelDelta) -> PanelSwap {
        PanelSwap {
            from_generation: from.into(),
            to_generation: to.into(),
            panels,
        }
    }

    fn moved_panel() -> PanelDelta {
        use crate::fast_model::room_resident::{PanelBounds, PanelMove};
        use aios_core::{RefU64, RefnoEnum};
        PanelDelta {
            recalc: vec![PanelMove {
                panel: RefnoEnum::from(RefU64::from_two_nums(24381, 7)),
                old: Some(PanelBounds([[0.0; 3], [1.0; 3]])),
                new: Some(PanelBounds([[5.0; 3], [6.0; 3]])),
            }],
            geometry_changed: 1,
            ..PanelDelta::default()
        }
    }

    /// 定向准入的真值表（ADR-073 D13 反查，2026-09-17；H1 / M1 2026-09-19 改成沿账本接段）：只有「库上盖过 + 本任务冻得出
    /// 计算器 + 换代段账本里从库上那一代到冻的这一代接得通、每一段面板变化非空」才定向；缺任何一条整库——库上更早 /
    /// 从没盖过、常驻说不出代号、账本里那一段面板变化为空（换代另有原因）、账本空（重启后）、对齐与冻计算器之间又换了
    /// 一代而账本没有后一段。定向时把链上各段原样带走，终点就是冻的那一代。
    #[test]
    fn a_task_goes_targeted_only_when_the_ledger_explains_the_librarys_whole_debt() {
        let delta = moved_panel();
        let own = swap("g1", "g2", delta.clone());
        let targeted = RoomRecomputeScope::Targeted {
            swaps: vec![own.clone()],
            parked: Vec::new(),
            stale_roots: Vec::new(),
            generation: "g2".into(),
        };
        assert_eq!(
            scope_for_task(Some("g1"), Some("g2"), &[own.clone()], &[], &[]),
            targeted,
            "库上 g1、冻 g2、账本 g1 → g2：定向，带着这一段的面板变化"
        );
        assert_eq!(
            targeted.targeted_generation(),
            Some("g2"),
            "准入链的终点就是冻的那一代"
        );
        assert_eq!(RoomRecomputeScope::WholeLibrary.targeted_generation(), None);
        for (label, library, resident, ledger) in [
            ("库上是更早的代", Some("g0"), Some("g2"), vec![own.clone()]),
            ("库上从没盖过", None, Some("g2"), vec![own.clone()]),
            (
                "常驻说不出代号（冻不出）",
                Some("g1"),
                None,
                vec![own.clone()],
            ),
            (
                "面板变化为空（换代另有原因）",
                Some("g1"),
                Some("g2"),
                vec![swap("g1", "g2", PanelDelta::default())],
            ),
            (
                "账本空（重启后 / 从没换过代）",
                Some("g1"),
                Some("g2"),
                Vec::new(),
            ),
            (
                "账本里的段不从库上那一代起",
                Some("g1"),
                Some("g2"),
                vec![swap("g0", "g2", delta.clone())],
            ),
            (
                // H1（2026-09-19）：对齐装上 g2、冻计算器时常驻已是 g3，账本里没有 g2 → g3——这一段说不了 g2 → g3。
                "对齐与冻计算器之间又换了一代，账本没有后一段",
                Some("g1"),
                Some("g3"),
                vec![own.clone()],
            ),
        ] {
            assert_eq!(
                scope_for_task(library, resident, &ledger, &[], &[]),
                RoomRecomputeScope::WholeLibrary,
                "{label}"
            );
        }
        // M1：账本里有 g2 → g3（ensure 重建时记的）就接上——两段都反查、按冻的 g3 算。
        let ensure = swap("g2", "g3", delta.clone());
        assert_eq!(
            scope_for_task(
                Some("g1"),
                Some("g3"),
                &[own.clone(), ensure.clone()],
                &[],
                &[]
            ),
            RoomRecomputeScope::Targeted {
                swaps: vec![own.clone(), ensure.clone()],
                parked: Vec::new(),
                stale_roots: Vec::new(),
                generation: "g3".into(),
            },
            "对齐与冻之间换的那一代账本里有：两段接起来定向"
        );
        // 账本里的段不分谁记的：本任务 `Unchanged`（ensure 先重建了），库上 g1、冻 g2、账本 g1 → g2 → 定向。
        assert_eq!(
            scope_for_task(
                Some("g1"),
                Some("g2"),
                &[ensure.clone(), own.clone()],
                &[],
                &[]
            ),
            targeted,
            "谁重建谁记，谁来接都接得上（M1）"
        );
        // 库上已是这一代：不欠；问到这里的（规划器排了 `RoomOnly`）按整库办。
        assert_eq!(
            scope_for_task(Some("g2"), Some("g2"), &[own.clone()], &[], &[]),
            RoomRecomputeScope::WholeLibrary
        );
        // `request_for_task` 把范围带上：三条触发同时点亮（规划器 + 换代 + ③）仍是定向，多一句说接了哪几段。
        let item = room_only_work_item(7997, "DESI", 163);
        let request = request_for_task(
            std::slice::from_ref(&item),
            Some(&RoomReferenceRefresh::Rebuilt {
                previous_key: Some("k1".into()),
                key: "k2".into(),
                generation_changed: true,
                generation: "g2".into(),
                previous_generation: Some("g1".into()),
                diff: None,
                panels: Some(delta.clone()),
                elapsed_ms: 1,
            }),
            Some(&alignment(Some("g1"), "g2")),
            &[own.clone()],
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(request.scope, targeted);
        assert_eq!(request.reason.matches("；").count(), 3, "{request:?}");
        assert!(
            request.reason.contains("沿换代段账本接了 1 段：g1 → g2（removed=0 renumbered_rooms=0 recalc=1 (geometry_only 1)）"),
            "{}",
            request.reason
        );
    }

    fn parked(from: &str, to: &str, by: u32, elements: &[i64]) -> RoomRoots {
        RoomRoots {
            from_generation: from.into(),
            to_generation: to.into(),
            elements: elements
                .iter()
                .map(|element| (*element, vec![*element + 1]))
                .collect(),
            parked_by: by,
            parked_at: std::time::Instant::now(),
        }
    }

    /// 准入链的真值表（U3 b 档，2026-09-17；M1 2026-09-19 账本段并入）：从库上那一代出发沿停放物 / 账本段 `from → to`
    /// 逐段接，接到冻的那一代就定向——只停放链 / 停放链 + 账本段 / 两段停放接起来按链序 / 库上已越过的一段不要 /
    /// 同一起点停放段优先于同两代的账本段（免一次反查）/ 一条路断了换另一条；链断 / 绕圈 / 库上从没盖过 / 常驻说不出
    /// 代号 / 账本段面板变化为空而链要靠它 → 整库。`request_for_task` 接了哪几段各多一句原因。
    #[test]
    fn the_admission_walks_the_parked_chain_from_the_library_generation_to_the_resident() {
        let delta = moved_panel();
        let g1_g2 = parked("g1", "g2", 7_997, &[10, 11]);
        let g2_g3 = parked("g2", "g3", 7_999, &[12]);

        // 只停放链：本任务没换代，库上 g1，停放 g1→g2，冻 g2。
        assert_eq!(
            scope_for_task(Some("g1"), Some("g2"), &[], &[g1_g2.clone()], &[]),
            RoomRecomputeScope::Targeted {
                swaps: Vec::new(),
                parked: vec![g1_g2.clone()],
                stale_roots: Vec::new(),
                generation: "g2".into(),
            }
        );
        // 停放链 + 账本段：库上 g1，停放 g1→g2，账本 g2→g3（本任务自己那一次重建），冻 g3。
        let chained = scope_for_task(
            Some("g1"),
            Some("g3"),
            &[swap("g2", "g3", delta.clone())],
            &[g1_g2.clone()],
            &[],
        );
        assert_eq!(
            chained,
            RoomRecomputeScope::Targeted {
                swaps: vec![swap("g2", "g3", delta.clone())],
                parked: vec![g1_g2.clone()],
                stale_roots: Vec::new(),
                generation: "g3".into(),
            }
        );
        assert_eq!(chained.targeted_generation(), Some("g3"));
        // 两段停放接起来：库上 g1，停放 g1→g2、g2→g3（先后无关），冻 g3；用到的按链的顺序排。
        assert_eq!(
            scope_for_task(
                Some("g1"),
                Some("g3"),
                &[],
                &[g2_g3.clone(), g1_g2.clone()],
                &[]
            ),
            RoomRecomputeScope::Targeted {
                swaps: Vec::new(),
                parked: vec![g1_g2.clone(), g2_g3.clone()],
                stale_roots: Vec::new(),
                generation: "g3".into(),
            }
        );
        // 库上已越过的一段不要：库上 g2，停放 g1→g2、g2→g3，冻 g3。
        assert_eq!(
            scope_for_task(
                Some("g2"),
                Some("g3"),
                &[],
                &[g1_g2.clone(), g2_g3.clone()],
                &[]
            ),
            RoomRecomputeScope::Targeted {
                swaps: Vec::new(),
                parked: vec![g2_g3.clone()],
                stale_roots: Vec::new(),
                generation: "g3".into(),
            }
        );
        // 同一起点停放段优先：库上 g1，槽里 g1→g2，账本也有 g1→g2，冻 g2——用停放的（元素已在手，免一次反查）。
        assert_eq!(
            scope_for_task(
                Some("g1"),
                Some("g2"),
                &[swap("g1", "g2", delta.clone())],
                &[g1_g2.clone()],
                &[]
            ),
            RoomRecomputeScope::Targeted {
                swaps: Vec::new(),
                parked: vec![g1_g2.clone()],
                stale_roots: Vec::new(),
                generation: "g2".into(),
            }
        );
        // 一条路断了换另一条：库上 g1，停放 g1→g2 到不了 g3，账本有 g1→g3——走账本那条。
        assert_eq!(
            scope_for_task(
                Some("g1"),
                Some("g3"),
                &[swap("g1", "g3", delta.clone())],
                &[g1_g2.clone()],
                &[]
            ),
            RoomRecomputeScope::Targeted {
                swaps: vec![swap("g1", "g3", delta.clone())],
                parked: Vec::new(),
                stale_roots: Vec::new(),
                generation: "g3".into(),
            }
        );
        for (label, library, resident, ledger, slot) in [
            (
                "链断：停放 g2→g3，库上 g1",
                Some("g1"),
                Some("g3"),
                Vec::new(),
                vec![g2_g3.clone()],
            ),
            (
                "链断：停放 g1→g2 到不了冻的 g3",
                Some("g1"),
                Some("g3"),
                Vec::new(),
                vec![g1_g2.clone()],
            ),
            (
                "绕圈：g1→g2、g2→g1，冻 g3",
                Some("g1"),
                Some("g3"),
                Vec::new(),
                vec![g1_g2.clone(), parked("g2", "g1", 7_998, &[13])],
            ),
            (
                "库上从没盖过",
                None,
                Some("g2"),
                Vec::new(),
                vec![g1_g2.clone()],
            ),
            (
                "常驻说不出代号",
                Some("g1"),
                None,
                Vec::new(),
                vec![g1_g2.clone()],
            ),
            (
                "账本段面板变化为空而链要靠它：g1→g2 停放、账本 g2→g3 空 delta",
                Some("g1"),
                Some("g3"),
                vec![swap("g2", "g3", PanelDelta::default())],
                vec![g1_g2.clone()],
            ),
            (
                "账本段不在链上：库上 g1、停放 g1→g2、账本 g5→g3",
                Some("g1"),
                Some("g3"),
                vec![swap("g5", "g3", delta.clone())],
                vec![g1_g2.clone()],
            ),
            (
                "槽空、账本空、却欠着（重启后都丢了）",
                Some("g1"),
                Some("g2"),
                Vec::new(),
                Vec::new(),
            ),
        ] {
            assert_eq!(
                scope_for_task(library, resident, &ledger, &slot, &[]),
                RoomRecomputeScope::WholeLibrary,
                "{label}"
            );
        }
        // 库上已是这一代：不欠；停放物是越过的代（执行体在 `parked_for_task` 里顺手清掉）。
        assert_eq!(
            scope_for_task(Some("g2"), Some("g2"), &[], &[g1_g2.clone()], &[]),
            RoomRecomputeScope::WholeLibrary
        );

        // `request_for_task`：接了停放链的定向多一句原因，点名停的是谁、多少；③ 那一句照旧。
        let unchanged = RoomReferenceRefresh::Unchanged { key: "k2".into() };
        let request = request_for_task(
            &[],
            Some(&unchanged),
            Some(&alignment(Some("g1"), "g3")),
            &[],
            &[g1_g2.clone(), g2_g3.clone()],
            &[],
        )
        .unwrap();
        assert_eq!(
            request.scope,
            RoomRecomputeScope::Targeted {
                swaps: Vec::new(),
                parked: vec![g1_g2, g2_g3],
                stale_roots: Vec::new(),
                generation: "g3".into(),
            }
        );
        assert!(
            request.reason.contains("g1 ≠ 此刻常驻代号 g3")
                && request.reason.contains(
                    "别的库的面板变化停放到本库 2 段：2 个元素（g1 → g2，dbnum=7997 停放）、1 个元素（g2 → g3，dbnum=7999 停放）"
                ),
            "{}",
            request.reason
        );
    }

    /// 第三条触发的真值表（电平型，2026-09-17）：库上那一格与此刻常驻代号不等就欠——包括库上没有那一格；
    /// 相等不欠；常驻说不出代号（开关关 / 还在装 / `Blocked`）没有可对齐的目标，不欠。
    #[test]
    fn the_library_owes_a_recompute_until_its_durable_alignment_matches_the_resident() {
        assert!(library_alignment_owed(Some("efd4ebe1"), Some("efd4ebe1")).is_none());
        let stale =
            library_alignment_owed(Some("00aa11bb22cc33dd"), Some("efd4ebe1ffff0000")).unwrap();
        assert!(
            stale.contains("00aa11bb22cc")
                && stale.contains("efd4ebe1ffff")
                && stale.contains("欠一次整库重算"),
            "{stale}"
        );
        let never = library_alignment_owed(None, Some("efd4ebe1")).unwrap();
        assert!(
            never.contains("没有整库房间对齐代号") && never.contains("efd4ebe1"),
            "{never}"
        );
        assert!(
            library_alignment_owed(Some("efd4ebe1"), None).is_none()
                && library_alignment_owed(None, None).is_none(),
            "常驻说不出代号 = 没有可对齐的目标"
        );
    }

    fn acked(root: u32, generation: Option<&str>) -> AckedRoot {
        AckedRoot {
            root: RefNo::new(24383, root),
            room_generation: generation.map(str::to_owned),
        }
    }

    /// F3（2026-09-19）：常驻代号是输入摘要、不单调——库上 g1、中间一代 g2 是空段（进保护集）没进账本、期间 `/model/ensure`
    /// 按 g2 生成了几根、常驻又回到 g1：库上 g1 == 常驻 g1，③ 不欠、账本没段，那几根的边却是 g2 算的。投影里 ACK 的房间代号
    /// 是唯一证据：库上已与常驻同代时，代号 ≠ 这一代的根（没算房间的 `None` 也算）走空段定向、只算它们；都对得上才是「不欠」；
    /// 链接得通时，代号不在链上任一代的根一并点名，在链上的不点（各段反查过）；链断退整库时不看。`request_for_task`：
    /// F3 单独就是一条触发；库上同代却被 ①/② 问到仍整库（它们说不出欠什么）；说不出常驻代号时 ACK 不看。
    #[test]
    fn a_stale_ack_in_an_aligned_library_is_the_debt_itself_and_goes_targeted_on_its_own() {
        let stale_two = [
            acked(10, Some("g1")),
            acked(30, Some("g2")),
            acked(20, None),
        ];
        let empty_segment = RoomRecomputeScope::Targeted {
            swaps: Vec::new(),
            parked: Vec::new(),
            stale_roots: vec![RefNo::new(24383, 20), RefNo::new(24383, 30)],
            generation: "g1".into(),
        };
        assert_eq!(
            scope_for_task(Some("g1"), Some("g1"), &[], &[], &stale_two),
            empty_segment,
            "库上 g1 == 常驻 g1：ACK 不是 g1 的两根（另一代 / 没算房间）就是欠账，打包 refno 序"
        );
        assert_eq!(
            scope_for_task(Some("g1"), Some("g1"), &[], &[], &[acked(10, Some("g1"))]),
            RoomRecomputeScope::WholeLibrary,
            "都对得上：没有可点名的欠账（被 ①/② 问到才会走到这里，按整库办）"
        );
        // 链接得通：库上 g1、冻 g2、账本 g1 → g2；ACK g1 / g2 在链上不点名，gx（中间那一代）与没算房间的点名。
        let own = swap("g1", "g2", moved_panel());
        let mixed = [
            acked(10, Some("g1")),
            acked(30, Some("g2")),
            acked(20, Some("gx")),
            acked(40, None),
        ];
        let off_chain = vec![RefNo::new(24383, 20), RefNo::new(24383, 40)];
        assert_eq!(
            scope_for_task(Some("g1"), Some("g2"), &[own.clone()], &[], &mixed),
            RoomRecomputeScope::Targeted {
                swaps: vec![own.clone()],
                parked: Vec::new(),
                stale_roots: off_chain.clone(),
                generation: "g2".into(),
            }
        );
        // 停放段的两端同样算在链上。
        let slot = parked("g1", "g2", 7_997, &[11]);
        assert_eq!(
            scope_for_task(Some("g1"), Some("g2"), &[], &[slot.clone()], &mixed),
            RoomRecomputeScope::Targeted {
                swaps: Vec::new(),
                parked: vec![slot],
                stale_roots: off_chain,
                generation: "g2".into(),
            }
        );
        assert_eq!(
            scope_for_task(Some("g1"), Some("g2"), &[], &[], &mixed),
            RoomRecomputeScope::WholeLibrary,
            "链断退整库：每一根都算，ACK 不看"
        );

        // `request_for_task`：Unchanged + 库上同代 + 有代号不等的 ACK → F3 单独触发、空段定向、原因点名。
        let unchanged = RoomReferenceRefresh::Unchanged { key: "k".into() };
        let current = alignment(Some("g1"), "g1");
        let request = request_for_task(&[], Some(&unchanged), Some(&current), &[], &[], &stale_two)
            .expect("F3 alone triggers");
        assert_eq!(request.scope, empty_segment);
        assert!(
            request
                .reason
                .contains("投影里 2 根的 ACK 房间代号不在准入链上")
                && request.reason.contains("24383/20、24383/30"),
            "{}",
            request.reason
        );
        assert_eq!(request.reason.matches("；").count(), 0, "只有这一句");
        assert!(
            request_for_task(
                &[],
                Some(&unchanged),
                Some(&current),
                &[],
                &[],
                &[acked(10, Some("g1"))],
            )
            .is_none(),
            "都对得上：不欠"
        );
        // 库上同代却被 ① 问到：整库（① 说不出欠什么），F3 那一句不加。
        let item = room_only_work_item(7997, "DESI", 163);
        let planned = request_for_task(
            std::slice::from_ref(&item),
            Some(&unchanged),
            Some(&current),
            &[],
            &[],
            &stale_two,
        )
        .unwrap();
        assert_eq!(planned.scope, RoomRecomputeScope::WholeLibrary);
        assert!(!planned.reason.contains("F3"), "{}", planned.reason);
        // ② 同理：本任务刚换回这一代。
        let back = RoomReferenceRefresh::Rebuilt {
            previous_key: Some("k2".into()),
            key: "k1".into(),
            generation_changed: true,
            generation: "g1".into(),
            previous_generation: Some("g2".into()),
            diff: None,
            panels: None,
            elapsed_ms: 1,
        };
        let rebuilt =
            request_for_task(&[], Some(&back), Some(&current), &[], &[], &stale_two).unwrap();
        assert_eq!(rebuilt.scope, RoomRecomputeScope::WholeLibrary);
        // 库上欠着、链接得通：③ 那一句 + F3 一句 + 账本一句，范围带着点名的根。
        let chained = request_for_task(
            &[],
            Some(&unchanged),
            Some(&alignment(Some("g1"), "g2")),
            &[own.clone()],
            &[],
            &mixed,
        )
        .unwrap();
        assert_eq!(
            chained.reason.matches("；").count(),
            2,
            "{}",
            chained.reason
        );
        assert!(
            matches!(&chained.scope, RoomRecomputeScope::Targeted { stale_roots, .. } if stale_roots.len() == 2),
            "{chained:?}"
        );
        // 说不出常驻代号（冻不出 / 开关关）：没有可比对的目标，ACK 不看。
        assert!(request_for_task(&[], Some(&unchanged), None, &[], &[], &stale_two).is_none());
    }

    /// F3 落库（2026-09-19）：库上 == 常驻、③ 不欠、账本没段，而水位行上记着「盖章之后本库有根按另一代写过边」
    /// （`model_room_stray_generations`）——那是刷盘按另一代写边时同事务记下的、重启也不丢的证据：欠一次。范围照 F3：
    /// 进程内还有代号不等的 ACK 就点名（空段定向）；重启后 ACK 丢了就整库（多算不漏算）。与库上同代的记录不算证据；
    /// 库上 ≠ 常驻时 ③ 那一句在前，这一格不另说。
    #[test]
    fn the_library_owes_a_recompute_when_the_watermark_remembers_writes_at_another_generation() {
        assert!(stray_generations_owed(Some("g1"), &[]).is_none());
        assert!(
            stray_generations_owed(Some("g1"), &["g1".to_string()]).is_none(),
            "与库上同代的记录不是证据（那几根的边与盖章一致）"
        );
        let owed =
            stray_generations_owed(Some("g1"), &["g1".to_string(), "g2".to_string()]).unwrap();
        assert!(
            owed.contains("另 1 代写过边（g2）") && owed.contains("F3 落库"),
            "{owed}"
        );
        let mut current = alignment(Some("g1"), "g1");
        assert!(current.owed().is_none());
        current.stray = vec!["g2".to_string()];
        assert!(
            current
                .owed()
                .is_some_and(|reason| reason.contains("F3 落库")),
            "库上 == 常驻却记着另一代写过根：欠"
        );
        let mut behind = alignment(Some("g1"), "g2");
        behind.stray = vec!["gx".to_string()];
        assert!(
            behind
                .owed()
                .is_some_and(|reason| reason.contains("g1 ≠ 此刻常驻代号 g2")),
            "库上 ≠ 常驻：③ 那一句在前"
        );

        // 重启后：进程内没有 ACK → 整库；原因只有 F3 落库那一句。
        let unchanged = RoomReferenceRefresh::Unchanged { key: "k".into() };
        let request = request_for_task(&[], Some(&unchanged), Some(&current), &[], &[], &[])
            .expect("the watermark cell alone triggers");
        assert_eq!(request.scope, RoomRecomputeScope::WholeLibrary);
        assert!(
            request.reason.contains("F3 落库") && request.reason.matches("；").count() == 0,
            "{}",
            request.reason
        );
        // 进程还活着：投影里 ACK 代号不等的根就是那几根 → 空段定向点名它们（两句原因）。
        let request = request_for_task(
            &[],
            Some(&unchanged),
            Some(&current),
            &[],
            &[],
            &[acked(10, Some("g1")), acked(30, Some("g2"))],
        )
        .unwrap();
        assert_eq!(
            request.scope,
            RoomRecomputeScope::Targeted {
                swaps: Vec::new(),
                parked: Vec::new(),
                stale_roots: vec![RefNo::new(24383, 30)],
                generation: "g1".into(),
            }
        );
        assert_eq!(
            request.reason.matches("；").count(),
            1,
            "{}",
            request.reason
        );
    }

    /// 源码钉（ADR-073 D13 反查，2026-09-17）：收尾段的分派——请求说定向先走 `run_targeted_room_recompute`，它回 `None`
    /// （树不就绪 / 根清单枚举不出来 / 有命中却折不出根 / 停放的元素折不出根 / 覆盖面对不上）就退
    /// `run_room_only_recompute`；定向那一路先枚举根清单、再 `room_reverse::affected_roots`（自己那一段）、再
    /// `room_reverse::fold_parked`（停放链）、两路的根并起来以 `RootScope::Targeted` 调 `recompute_dbnum_room_edges`，
    /// 面板级语句经 `render_panel_delta_statements`（只有自己那一段有）；算不出的根与整库同一处置
    /// （`settle_room_recompute_failures`）；成功的结论是 `Targeted(重算那台计算器的代号)`，与 `WholeLibrary` 同样说得了
    /// 「整库对齐到这一代」。本库的槽只在分派处、按成功的结论清（U3 b 档）。
    #[test]
    fn the_targeted_recompute_falls_back_to_the_whole_library_when_the_reverse_lookup_cannot_answer()
     {
        let worker = include_str!("batch_worker.rs");
        let dispatch = function_body(worker, "async fn run_room_recompute(");
        assert!(
            dispatch.contains("RoomRecomputeScope::WholeLibrary =>")
                && dispatch.contains("RoomRecomputeScope::Targeted {")
                && dispatch.contains("stale_roots,")
                && dispatch.contains("} => {"),
            "两种范围各走一路: {dispatch}"
        );
        assert!(
            dispatch.contains("run_targeted_room_recompute(")
                && dispatch.contains("swaps,")
                && dispatch.contains("parked,")
                && dispatch.contains("stale_roots,")
                && dispatch.contains("None =>")
                && dispatch.matches("run_room_only_recompute(job, ").count() == 2,
            "定向定不了（None）退整库；整库直接整库；点名的根（F3）随范围交给定向那一路: {dispatch}"
        );
        // H1（2026-09-19）：定向那一路拿的是本任务开工冻的那台（`task_rooms`）与准入链的终点代号；没冻出（`None`）
        // 同样退整库，不在收尾新冻。
        assert!(
            dispatch.contains("let targeted = match task_rooms {")
                && dispatch.contains("Some(rooms) => {")
                && dispatch.contains("request.scope.targeted_generation(),")
                && dispatch.contains("rooms,"),
            "定向复用本任务冻的计算器、带上准入链终点: {dispatch}"
        );
        assert!(
            !dispatch.contains("frozen_room_calculator()"),
            "分派处不新冻计算器（整库那一路在 run_room_only_recompute 里自己冻）"
        );
        // 清槽（U3 b 档 / L3）：只在分派处、结论是 `WholeLibrary` / `Targeted` 才动槽；整个执行体别处不清。
        // 定向清的是消费过的那几段（`clear_consumed(job.dbnum, parked)`，parked = 接进准入链的快照，L3 2026-09-19），
        // 整库才整槽清（`clear(job.dbnum)`）。
        let decided = dispatch
            .find("let verdict = match &request.scope {")
            .expect("先定结论");
        let consumed = dispatch
            .find("room_reverse::clear_consumed(job.dbnum, parked)")
            .expect("定向成功后只清消费过的段");
        let cleared = dispatch
            .find("room_reverse::clear(job.dbnum)")
            .expect("整库成功后清本库的槽");
        assert!(
            decided < consumed && decided < cleared,
            "结论定了才动槽: {dispatch}"
        );
        assert!(
            dispatch.contains(
                "(RoomRecomputeVerdict::Targeted(_), RoomRecomputeScope::Targeted { parked, .. }) => {"
            ) && dispatch.contains(
                "(RoomRecomputeVerdict::WholeLibrary(_) | RoomRecomputeVerdict::Targeted(_), _) => {"
            ),
            "定向结论按快照清消费过的段；整库结论整槽清；Failed / PartialCoverage / Disabled 留着: {dispatch}"
        );
        assert_eq!(
            worker.matches("room_reverse::clear(").count(),
            1,
            "执行体里整槽清只此一处（准入处的顺手清在 room_recompute::parked_for_task）"
        );
        assert_eq!(
            worker.matches("room_reverse::clear_consumed(").count(),
            1,
            "执行体里按段清只此一处"
        );
        let targeted = function_body(worker, "async fn run_targeted_room_recompute(");
        // H1（2026-09-19）：定向路**不**新冻——用传进来的那台；入口先对准入链终点与它的代号，不等退整库。
        assert!(
            !targeted.contains("frozen_room_calculator()"),
            "定向路复用本任务开工冻的计算器，不在收尾新冻「此刻这一代」: {targeted}"
        );
        let guard = targeted
            .find("if expected_generation != Some(rooms.resident_generation()) {")
            .expect("入口对准入链终点与计算器代号");
        assert!(
            guard
                < targeted
                    .find("library_generation_roots(dbnum, target_sesno)")
                    .expect("先枚举根清单"),
            "代号对不上在枚举根清单之前就退整库: {targeted}"
        );
        assert!(
            targeted.contains("&swap.to_generation,"),
            "停放给别的库的那一段以自己这一段的 to 收尾（== 计算器代号，入口对过）: {targeted}"
        );
        // 停放（U3 b 档；F1 2026-09-19 改在反查完成就停）：折不出根的分类过关之后、`recompute_dbnum_room_edges(` 之前，
        // **不以结论为条件**——反查已经完整，本库算没算齐不改变别的库欠什么；此前等结论定为 `Targeted` 才停，`Failed`
        // 一次就把 B 库的欠账连同证据（第一笔已删的成员边）一起丢了。树不就绪 / 反查读库失败 / 仍卡住的 `return None`
        // 都在它之前，反查说不了完整时到不了停放。
        let park = targeted
            .find("park_foreign_hits(")
            .expect("反查完成后把别的库的命中停放给它");
        let classified = targeted
            .find("if !still_stuck.is_empty() {")
            .expect("折不出根的最后一层分类");
        let recompute_at = targeted
            .find("recompute_dbnum_room_edges(")
            .expect("定向重算");
        let decided = targeted
            .find("Some(uncovered) if uncovered.is_empty() => {")
            .expect("覆盖面对上的那一臂");
        assert!(
            classified < park && park < recompute_at && park < decided,
            "停放在分类过关之后、重算之前，不等结论: {targeted}"
        );
        assert_eq!(
            targeted.matches("park_foreign_hits(").count(),
            1,
            "只此一处停放"
        );
        assert!(
            targeted.contains("Some(RoomRecomputeVerdict::Targeted(generation))"),
            "定向的结论"
        );
        assert!(
            !function_body(worker, "async fn run_room_only_recompute(")
                .contains("park_foreign_hits("),
            "整库那一路不停放：反查没跑、说不了别的库欠什么"
        );
        let enumerate = targeted
            .find("library_generation_roots(dbnum, target_sesno)")
            .expect("先枚举 DbSet@T 的根清单（折根的靶子）");
        let lookup = targeted
            .find("affected_roots(target.db(), store, dbnum, &swap.panels, &library_roots)")
            .expect("自己那一段：反查命中的根");
        let parked = targeted
            .find("fold_parked(parked, &library_roots)")
            .expect("停放链：折成本库的根（U3 b 档）");
        let recompute = targeted
            .find("recompute_dbnum_room_edges(")
            .expect("定向重算");
        assert!(
            enumerate < lookup && lookup < parked && parked < recompute,
            "根清单 → 反查 → 折停放 → 重算，顺序不能乱: {targeted}"
        );
        assert!(
            targeted.contains("for swap in swaps {"),
            "链上每一段换代各反查一次（M1：账本里的段不止本任务自己那一次）；没有换代段就没有反查那一路: {targeted}"
        );
        assert!(
            targeted.contains("for (swap, foreign) in swaps.iter().zip(&foreign_by_swap) {"),
            "停放按段：每一段的别的库命中连同**那一段**的 from → to 停出去，别的库的链才接得上: {targeted}"
        );
        assert!(
            targeted.contains(".chain(parked_roots.iter())")
                && targeted.contains(".chain(stale_roots.iter())")
                && targeted.contains("RootScope::Targeted {")
                && targeted.contains("roots: &roots,")
                && targeted.contains("panel_statements: &panel_statements,")
                && targeted.contains("render_panel_delta_statements(&swap.panels)"),
            "三路的根并起来只重算它们（反查命中 + 停放折出 + F3 点名），面板级语句（只有换代段有）随 RootScope::Targeted 交出去: {targeted}"
        );
        // F3（2026-09-19）：盖章那一臂里、结论之前，把链上的 ACK 一并对齐到链尾——链外的代号是下一个任务要点名的证据，不动。
        let stamped = targeted
            .find("Some(uncovered) if uncovered.is_empty() => {")
            .expect("覆盖面对上的那一臂");
        let aligned = targeted
            .find("align_room_acks_along_chain(store, dbnum, &on_chain, &generation)")
            .expect("盖章时对齐链上的 ACK（F3）");
        let verdict_at = targeted
            .find("Some(RoomRecomputeVerdict::Targeted(generation))")
            .expect("定向的结论");
        assert!(
            stamped < aligned && aligned < verdict_at,
            "链上的 ACK 在覆盖面对上之后、结论之前对齐: {targeted}"
        );
        assert_eq!(
            targeted.matches("align_room_acks_along_chain(").count(),
            1,
            "只此一处对齐；`Failed` / 退整库都不动 ACK"
        );
        assert!(
            !function_body(worker, "async fn run_room_only_recompute(")
                .contains("align_room_acks_along_chain("),
            "整库那一路每一根都算、ACK 由 recompute_dbnum_room_edges 自己对齐，不走链"
        );
        // F2（2026-09-19）：要反查的面板自己的 `room_panel_relate` 也补在面板级语句里（按本任务计算器的在册视图），
        // 面板级语句整串进最后一笔（F1：`recompute_dbnum_room_edges` 里作 epilogue，不再是 prelude）。
        assert!(
            targeted.contains("render_recalc_panel_topology_statements(")
                && targeted.contains("swap.panels.recalc.iter().map(|entry| entry.panel)"),
            "链上每一块 recalc 面板自己的拓扑边随面板级语句一起写: {targeted}"
        );
        let adapter = function_body(
            include_str!("../fast_model/model_db_adapter.rs"),
            "pub async fn recompute_dbnum_room_edges(",
        );
        assert!(
            adapter
                .contains("pack_publication_transactions(\"\", &pieces, panel_statements, false)"),
            "面板级语句作 epilogue 进最后一笔：命中的根都重写之后才删面板级的边，中途失败不丢 durable_panel_members 的证据: {adapter}"
        );
        assert!(
            targeted.contains("room_recompute_uncovered(Some(&roots), &report.covered_roots)"),
            "覆盖面对的是并起来的根: {targeted}"
        );
        for (fallback, marker) in [
            (
                "计算器不是准入链终点那一代（H1）",
                "if expected_generation != Some(rooms.resident_generation()) {",
            ),
            ("树不就绪", "Ok(ReverseLookup::TreeUnavailable(state)) =>"),
            (
                // M2（2026-09-19）：折不出根的元素（反查命中的 + 停放链上的）只有**还在 DbSet@T 上**的才拦；
                // 已不存在的（本窗删掉的残留）忽略。
                "有命中 / 停放的元素还在 T 上却折不出根",
                "if !stuck.is_empty() {",
            ),
        ] {
            let at = targeted
                .find(marker)
                .unwrap_or_else(|| panic!("{fallback}: {targeted}"));
            let rest = &targeted[at..];
            let ret = rest.find("return None;").expect("退整库");
            assert!(
                ret < rest
                    .find("recompute_dbnum_room_edges(")
                    .unwrap_or(usize::MAX),
                "{fallback} → 在重算之前就退整库（return None）: {rest}"
            );
        }
        assert!(
            targeted.contains("settle_room_recompute_failures(")
                && function_body(worker, "async fn run_room_only_recompute(")
                    .contains("settle_room_recompute_failures("),
            "算不出的根两路同一处置"
        );
        assert!(
            targeted.contains("let generation = rooms.resident_generation().to_string();")
                && targeted.contains("Some(RoomRecomputeVerdict::Targeted(generation))"),
            "定向的结论带着重算那台计算器的代号: {targeted}"
        );
        let verdict = function_body(worker, "    fn library_generation(&self) -> Option<&str> {");
        assert!(
            verdict.contains(
                "Self::WholeLibrary(generation) | Self::Targeted(generation) => Some(generation)"
            ),
            "定向与整库同样说得了「整库对齐到这一代」: {verdict}"
        );
    }

    /// 源码钉（ADR-073 D13 / U1 第二半，2026-09-17）：整库重算对不在投影里的根**回读库上几何**——收尾段先枚举
    /// `DbSet@T` 的根清单、再把它交给 `recompute_dbnum_room_edges` 靶着回读；`recompute_dbnum_room_edges` 对点名
    /// 的根经 `durable_records_of_root` 读库、经 `durable_room_inputs` 折成同一形状、走**同一个**
    /// `calculate_for_root` / `render_root_room_edges`；投影里有回执的根不回读。此前为「同一代下反复点、每次都
    /// 覆盖不全」设的去重表（`PARTIAL_COVERAGE_SETTLED` / `settle_partial_coverage` / `rearm_library`）随之退役，
    /// 第三条触发只看库上的事实。
    #[test]
    fn the_library_recompute_reads_back_the_durable_geometry_of_roots_outside_the_projection() {
        let recompute = function_body(
            include_str!("batch_worker.rs"),
            "async fn run_room_only_recompute(",
        );
        let enumerate = recompute
            .find("library_generation_roots(dbnum, target_sesno)")
            .expect("先枚举 DbSet@T 的根清单");
        let call = recompute
            .find("recompute_dbnum_room_edges(")
            .expect("整库重算");
        assert!(
            enumerate < call && recompute.contains("expected.as_deref(),"),
            "根清单在重算之前取、并作为靶子交给 recompute_dbnum_room_edges: {recompute}"
        );
        for retired in ["settle_partial_coverage(", "rearm_library("] {
            assert!(
                !recompute.contains(retired),
                "去重表已退役，收尾段不该再有 {retired}: {recompute}"
            );
        }
        // 只看本模块的生产段——这段测试自己的字面量不算。
        let production = include_str!("room_recompute.rs")
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test module")
            .0;
        assert!(
            !production.contains("static PARTIAL_COVERAGE_SETTLED")
                && !production.contains("fn settle_partial_coverage(")
                && !production.contains("fn rearm_library("),
            "去重表已退役，本模块不该再定义它"
        );

        let adapter = include_str!("../fast_model/model_db_adapter.rs");
        let body = adapter
            .split_once("pub async fn recompute_dbnum_room_edges(")
            .expect("recompute_dbnum_room_edges")
            .1;
        let body = &body[..body
            .find("\n}\r\n")
            .or_else(|| body.find("\n}\n"))
            .expect("end")];
        assert!(
            body.contains("library_roots: Option<&[RefNo]>"),
            "点名的根清单进签名: {body}"
        );
        assert!(
            body.contains("!projected_set.contains(&packed_refno(*root))"),
            "投影里有回执的根不回读: {body}"
        );
        let read = body
            .find("model_record_reader::durable_records_of_root(")
            .expect("回读库上几何");
        let fold = body
            .find("room_model::durable_room_inputs(&rows)")
            .expect("折成房间判定的输入");
        assert!(read < fold, "先读库、再折形状: {body}");
        assert_eq!(
            body.matches("rooms.calculate_for_root(").count(),
            1,
            "两种来源走同一个判定入口（共用的那一段）: {body}"
        );
        assert_eq!(
            body.matches("rooms.render_root_room_edges(").count(),
            1,
            "两种来源走同一套渲染: {body}"
        );
    }
}
