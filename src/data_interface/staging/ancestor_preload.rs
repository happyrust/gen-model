//! 模型工作项祖先链的**解析式预载**（2026-08-07 方案 W1，决议 D2–D4/D8/D9）。
//!
//! 暂存窗口内的 ancestor / transform 消费者（`fn::ancestor(pe).refno.*` →
//! `get_world_transform`、`update_inst_relate_aabbs_by_refnos` 的 `in.noun`…）
//! 需要工作项目标、带产物子树节点及其**完整祖先链（到顶，含 WORL）**的
//! `pe` + `ATT_{noun}` + `ATT_UDA` + 链上 `pe_owner` 边都在暂存库里。名词表行
//! 缺失时 `.refno.*` 静默 NONE、`get_position().unwrap_or_default()` 把缺失当
//! (0,0,0)——未变更祖先带真 POS 时窗口内算出的世界变换**丢位移且不报错**。
//!
//! 数据源是 **db 文件部分解析**（D3，ADR-017 读路由规则①），不从持久层拷贝：
//! 索引定位（[`parse_file_db_index_data`]，不展开成员树）→ 沿元素自带的 owner
//! 迭代上溯到顶 → 复用 CATA 惰性兜底同一套渲染函数落 `pe`/`ATT_*`/`ATT_UDA`，
//! 保证与解析层落库形状同构。写入走 [`execute_generation_preload`]
//! （StagingOnly，不进 journal）：这些行是窗口前旧态，持久层本来就有，随
//! journal 写回只会白胀资源配额；`INSERT IGNORE` / `record::exists` 守卫保证
//! 不回退本窗口解析已写的新态行。
//!
//! **删除目标刻意不在本模块的种子里**：被删元素已从文件 refno 索引消失，解析
//! 必然失败；其子树拓扑与旧产物是「窗口前旧态、文件里没有」的数据，与旧生成
//! 产物同类（ADR-017 读路由规则②），继续由 `preload.rs` 从持久层点查拷入——
//! 删除级联的暂存子树遍历（`collect_pe_subtree_refnos` → `active_data_db`）
//! 依赖那份拷贝。
//!
//! 失败语义 fail-closed（D8）：解析断链 / 会话号越过窗口终点（read-your-future，
//! 事实基线 7 的封口）/ 链深超出 `fn::ancestor` 的 9 跳展开预算（D9 响亮探针）/
//! 装载后完整性验证不过——任一情况整批失败带修法，不开模型工作。CATA/DESI
//! 惰性闭包保留作运行期最后一道网（兜 CATA 漏边），不再承担 DESI 祖先正确性。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use aios_core::{NamedAttrMap, RefU64, RefnoEnum};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

use crate::data_interface::cata_closure::{dedupe_members, is_valid_ref0};

/// `fn::ancestor` 的手写 `owner.owner.…` 展开预算（`common.surql`，9 跳）。
///
/// 种子到顶的 hop 数超过它时，窗口内的祖先 / 变换读取会**静默截断**——与本次
/// 任务的目标直接冲突，所以在解析阶段就响亮失败（D9：本期只加探针不扩函数）。
pub(crate) const ANCESTOR_HOP_BUDGET: usize = 9;

/// 迭代上溯的防御性深度上限（owner 链成环 / 文件损坏时的最后一道闸）。
const WALK_DEPTH_CAP: usize = 64;

/// 落库分批大小（与 CATA 惰性兜底同一量级）。
const INSERT_CHUNK: usize = 500;

/// 一个从文件解析出来的祖先链元素（含种子自身）。
#[derive(Debug, Clone)]
pub(crate) struct AncestorElement {
    pub refno: RefU64,
    /// 元素自带 OWNER；`ref0` 无效（0 / 哨兵）= 到顶（WORL 的 owner）。
    pub owner: RefU64,
    /// 合并后的完整属性表（TYPE / REFNO / OWNER / POS / SESNO / UDA…）。
    pub att: NamedAttrMap,
    /// 收敛后的成员表（链边槽位的唯一事实来源，与解析层落库同构）。
    pub children: Vec<RefU64>,
}

/// 种子集合的祖先闭包：种子自身 ∪ 全部祖先（到顶，含 WORL），可直接落暂存。
#[derive(Debug, Default)]
pub(crate) struct AncestorClosure {
    /// 首次发现顺序；写入幂等，顺序不承载语义。
    pub elements: Vec<AncestorElement>,
    /// 每个种子到顶的 hop 数（不含自身；种子即 WORL 时为 0）。
    pub seed_hops: BTreeMap<RefU64, usize>,
    /// 本库的 WORL（链的合法终点，来自文件索引）。
    pub world_refno: RefU64,
}

impl AncestorClosure {
    pub(crate) fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }
}

/// 「需要祖先数据的种子集合」（D2）：Transform 目标 + Transform 子树里带产物的
/// 节点 + RegenRoot 根 + 本批新单元根。
///
/// DeleteCleanup 目标**刻意排除**：被删元素已从文件索引消失，解析必然失败；其
/// 子树拓扑走 `preload.rs` 的持久层拷贝（见模块文档）。房间目标不进种子：房间
/// 轮有自己的工作集预载（D2 明确不动）。
pub(crate) fn ancestor_seed_refnos(
    plan_items: &[crate::data_interface::model_update_plan::ModelWorkItem],
    new_units: &[crate::data_interface::manual_update::UnitTask],
    transform_targets: &[RefnoEnum],
    transform_model_refnos: &[RefnoEnum],
) -> Vec<RefU64> {
    use crate::data_interface::model_update_plan::ModelWorkAction;
    let mut seeds = std::collections::BTreeSet::new();
    seeds.extend(
        transform_targets
            .iter()
            .chain(transform_model_refnos)
            .map(RefnoEnum::refno),
    );
    seeds.extend(
        plan_items
            .iter()
            .filter(|item| item.action == ModelWorkAction::RegenRoot)
            .map(|item| RefnoEnum::from(item.target_refno.as_str()).refno()),
    );
    seeds.extend(
        new_units
            .iter()
            .map(|unit| RefnoEnum::from(unit.root_refno.as_str()).refno()),
    );
    seeds
        .into_iter()
        .filter(|refno| is_valid_ref0(refno.get_0()))
        .collect()
}

/// 沿 owner 迭代上溯解析种子的祖先闭包。
///
/// `lookup` 是数据源 seam：生产传文件索引解析（[`AncestorParseSession`]），
/// 测试传内存表。返回 `Ok(None)` = 元素不在数据源里（断链，fail-closed）。
///
/// 三道守卫都在这里、在任何暂存写与持锁**之前**执行：
/// 1. **断链**：链上任何元素定位/解析失败 → 整批失败；
/// 2. **sesno 封口**（事实基线 7）：元素的会话号超过窗口终点 = 文件在冻结后又
///    被新会话触及本链（read-your-future），拒绝把未来态当旧态预载；失败重排
///    后由既有冻结重扫/吸收路径扩窗收敛；
/// 3. **9 跳预算**（D9）：种子到顶的 hop 数超出 `fn::ancestor` 展开预算 →
///    响亮失败，不许静默截断。
pub(crate) async fn resolve_ancestor_closure<F, Fut>(
    seeds: &[RefU64],
    world_refno: RefU64,
    end_sesno: i32,
    mut lookup: F,
) -> anyhow::Result<AncestorClosure>
where
    F: FnMut(RefU64) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Option<AncestorElement>>>,
{
    let mut owner_of: HashMap<RefU64, RefU64> = HashMap::new();
    let mut elements = Vec::new();
    let mut seed_hops = BTreeMap::new();

    let mut ordered_seeds = Vec::new();
    let mut seen_seeds = HashSet::new();
    for &seed in seeds {
        anyhow::ensure!(
            is_valid_ref0(seed.get_0()),
            "祖先预载收到无效种子 {}（ref0 为空/哨兵）——工作项种子集合有误",
            seed.to_pe_key()
        );
        if seen_seeds.insert(seed) {
            ordered_seeds.push(seed);
        }
    }

    for &seed in &ordered_seeds {
        let mut current = seed;
        let mut walked = 0usize;
        loop {
            if owner_of.contains_key(&current) {
                break; // 这一段链已由更早的种子解析过
            }
            let element = lookup(current).await?.ok_or_else(|| {
                anyhow::anyhow!(
                    "祖先链断裂：{}（种子 {} 的祖先）不在文件索引里或无法解析。\
                     修法：若该元素已被本窗口之后的会话删除/搬移，等下一批扩窗吸收后重试；\
                     若持续出现，检查该 db 文件的所有权数据",
                    current.to_pe_key(),
                    seed.to_pe_key()
                )
            })?;
            anyhow::ensure!(
                element.refno == current,
                "祖先解析返回了错误的元素：要 {} 得 {}",
                current.to_pe_key(),
                element.refno.to_pe_key()
            );
            let element_sesno = element.att.sesno();
            anyhow::ensure!(
                element_sesno <= end_sesno,
                "祖先预载封口：{} 的会话号 {element_sesno} 超过窗口终点 {end_sesno}\
                 ——文件在本批冻结后又有新会话触及这条链（read-your-future）。\
                 修法：本批失败重排，下一次扫描把新会话吸收进窗口后自然收敛",
                current.to_pe_key()
            );
            let owner = element.owner;
            owner_of.insert(current, owner);
            elements.push(element);
            if !is_valid_ref0(owner.get_0()) {
                anyhow::ensure!(
                    current == world_refno,
                    "祖先链在 {} 处到顶，但它不是本库的 WORL（{}）——文件所有权数据异常",
                    current.to_pe_key(),
                    world_refno.to_pe_key()
                );
                break;
            }
            walked += 1;
            anyhow::ensure!(
                walked <= WALK_DEPTH_CAP,
                "种子 {} 的 owner 链超过防御深度 {WALK_DEPTH_CAP}（疑似成环），拒绝继续",
                seed.to_pe_key()
            );
            current = owner;
        }

        // hop 数按已解析的 owner 表重算（覆盖提前 break 的共享链段），顺带兜住
        // 跨种子拼出来的环。
        let mut hops = 0usize;
        let mut cursor = seed;
        while let Some(&owner) = owner_of.get(&cursor) {
            if !is_valid_ref0(owner.get_0()) {
                break;
            }
            hops += 1;
            anyhow::ensure!(
                hops <= WALK_DEPTH_CAP,
                "种子 {} 的 owner 链成环（{hops} 跳仍未到顶），拒绝继续",
                seed.to_pe_key()
            );
            cursor = owner;
        }
        anyhow::ensure!(
            hops <= ANCESTOR_HOP_BUDGET,
            "种子 {} 到顶需要 {hops} 跳，超出 fn::ancestor 的 {ANCESTOR_HOP_BUDGET} 跳展开预算\
             ——窗口内的祖先/变换读取会静默截断（D9 探针，宁可响亮失败）。\
             修法：扩容 common.surql 的 fn::ancestor 展开层数并带灌库版本验证（另立项），\
             或降低该子树的层级深度",
            seed.to_pe_key()
        );
        seed_hops.insert(seed, hops);
    }

    Ok(AncestorClosure {
        elements,
        seed_hops,
        world_refno,
    })
}

/// 生产数据源：一次窗口打开一个文件索引会话（整文件字节快照 + refno 索引，
/// **不展开成员树**——上溯只用元素自带的 owner 与成员块）。
pub(crate) struct AncestorParseSession {
    index: parse_pdms_db::parse::DbIndexData,
}

impl AncestorParseSession {
    /// 读文件 + 建 refno 索引。快照时点 = 此刻；随后所有解析都出自这份字节，
    /// 配合 [`resolve_ancestor_closure`] 的逐元素 sesno 封口构成 W1 的时点纪律。
    pub(crate) fn open(path: &Path) -> anyhow::Result<Self> {
        let index = parse_pdms_db::parse::parse_file_db_index_data(&path.to_path_buf())?;
        Ok(Self { index })
    }

    pub(crate) async fn resolve(
        &self,
        seeds: &[RefU64],
        end_sesno: i32,
    ) -> anyhow::Result<AncestorClosure> {
        resolve_ancestor_closure(seeds, self.index.world_refno, end_sesno, |refno| {
            parse_ancestor_element(&self.index, refno)
        })
        .await
    }
}

/// 按索引定位并解析单个元素（与 `cata_closure::parse_refnos_with_session` 同一
/// 解析路径，但定位失败/越界/解析失败都**上抛**而不是跳过——祖先数据直接决定
/// 模型正确性，这里没有「按 cache-miss 处理」的余地）。
async fn parse_ancestor_element(
    index: &parse_pdms_db::parse::DbIndexData,
    refno: RefU64,
) -> anyhow::Result<Option<AncestorElement>> {
    let pos = match index.refno_table_map.get(&refno) {
        Some(entry) => entry.pos,
        None => return Ok(None),
    };
    anyhow::ensure!(
        pos >= 4 && pos <= index.bytes.len(),
        "元素 {} 的索引位置 {pos} 越界（文件 {} 字节）——索引损坏",
        refno.to_pe_key(),
        index.bytes.len()
    );
    let db_info = aios_core::get_default_pdms_db_info();
    let ele = parse_pdms_db::parse::parse_ele_data_with_info(&index.bytes[pos - 4..], &db_info)
        .await
        .map_err(|error| {
            anyhow::anyhow!("元素 {} 部分解析失败: {error:#}", refno.to_pe_key())
        })?;
    let att = ele.whole_attmap.merge();
    let children = dedupe_members(refno, &ele.children);
    Ok(Some(AncestorElement {
        refno,
        owner: ele.owner,
        att,
        children,
    }))
}

/// 把闭包装载进暂存（StagingOnly，不进 journal）。窗口外是无操作。
///
/// 渲染与 `ensure_cata_refnos_parsed` 同一套函数（`att.pe().gen_sur_json` /
/// `att.gen_sur_json_exclude` / `att.gen_sur_json_uda`），保证与解析层落库形状
/// 同构（R1 的对策）。链边只写「子 → owner」一条（owner 成员块里该子的真实槽位），
/// 守卫盖住边的两套身份（见下）——**不做** OwnerReplace 整块替换：那会把本窗口
/// 解析已写的成员块回退成文件态之外的形状。
///
/// **名词表行用 `UPSERT … MERGE` 补齐，不用 `INSERT IGNORE`**（2026-08-08 实机
/// 7997@194 复盘）：Modified 元素的窗口主数据落库是
/// `UPSERT {noun}:{id} MERGE {只含本会话改动的属性}`——持久层上它合并进完整旧行，
/// 而在空白暂存库里它**从无到有创建出残行**（只有改动属性，无 TYPE/NAME/未变属性）。
/// `INSERT IGNORE` 会原样保留这条残行，窗口内的 ancestor/transform 读取把缺失的
/// ORI/POS 当默认值——正是 W1 要治的静默错模型，只是从祖先挪到了目标自己。
/// MERGE 补齐是安全的：预载属性表与窗口语句出自**同一份文件字节**（逐元素 sesno
/// 封口保证快照不越过窗口终点），重叠字段的值恒等，被本会话删除的属性不在解析
/// 属性表里、MERGE 不会碰它写下的 null——补的只有缺失字段，不回退任何窗口新态。
pub(crate) async fn apply_ancestor_preload(
    closure: &AncestorClosure,
    dbnum: u32,
) -> anyhow::Result<usize> {
    if super::active_staging_writes().is_none() || closure.elements.is_empty() {
        return Ok(0);
    }
    let by_refno: HashMap<RefU64, &AncestorElement> = closure
        .elements
        .iter()
        .map(|element| (element.refno, element))
        .collect();

    let mut pe_jsons = Vec::with_capacity(closure.elements.len());
    let mut att_statements = Vec::with_capacity(closure.elements.len());
    let mut uda_jsons = Vec::new();
    let mut edge_statements = Vec::new();
    for element in &closure.elements {
        let att = &element.att;
        let noun = att.get_type_str();
        anyhow::ensure!(
            !noun.is_empty() && noun != "unset",
            "祖先 {} 解析不出类型（TYPE 缺失），名词表行无从落库——预载不完整，整批失败",
            element.refno.to_pe_key()
        );
        let pe_key = element.refno.to_pe_key();
        pe_jsons.push(att.pe(dbnum as i32).gen_sur_json(Some(pe_key.clone())));
        // MERGE 目标不许带 id 字段；键形制与 pe.refno 链接同源（to_table_key），
        // 保证补的就是链接指向的那一条。
        let att_json = att.gen_sur_json_exclude(&["id"], None).ok_or_else(|| {
            anyhow::anyhow!("祖先 {} 的名词表行渲染失败", element.refno.to_pe_key())
        })?;
        att_statements.push(format!(
            "UPSERT {} MERGE {att_json};",
            element.refno.to_table_key(noun)
        ));
        if let Some(json) = att.gen_sur_json_uda(&[]) {
            uda_jsons.push(aios_core::helper::normalize_sql_string(&json));
        }

        if is_valid_ref0(element.owner.get_0()) {
            let owner_element = by_refno.get(&element.owner).ok_or_else(|| {
                anyhow::anyhow!(
                    "祖先闭包不含 {} 的 owner {}——walk 不变量被破坏",
                    element.refno.to_pe_key(),
                    element.owner.to_pe_key()
                )
            })?;
            let slot = owner_element
                .children
                .iter()
                .position(|child| *child == element.refno)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "owner {} 的成员表不含 {}——文件所有权数据不一致，拒绝伪造槽位",
                        element.owner.to_pe_key(),
                        element.refno.to_pe_key()
                    )
                })?;
            let owner_key = element.owner.to_pe_key();
            let edge_id = format!("pe_owner:[{owner_key}, {slot}]");
            // 幂等守卫必须盖住这条边的**两套身份**（docs/2026-08-06_pe-owner-uniqueness-fix-audit.md）：
            // 记录 id 是 `[owner, 槽位]`，唯一索引 `unique_pe_owner` 是 `(in, out)`。房间/
            // 产物预载从持久层拷来的旧边可能停在**旧槽位**（成员表此后增删过），只查 id
            // 会带着文件态的新槽位撞唯一索引——2026-08-08 实机 7997@194 的整批 fail-closed
            // 即此。逻辑边已在（任意槽位）或目标槽位被别的成员占着（陈旧持久态）都跳过
            // 不写：链的正确性押在 pe 行的 owner 字段上（[`validate_ancestor_preload`] 验的
            // 正是它），这条边只是窗口内的一致性补充，且预载绝不改写窗口前旧态。
            // `(in, out)` 探针走 `unique_pe_owner` 索引（同审计文档实测），不扫全表。
            edge_statements.push(format!(
                "IF !record::exists({edge_id}) AND array::len((SELECT VALUE id FROM pe_owner \
                 WHERE in = {pe_key} AND out = {owner_key} LIMIT 1)) == 0 \
                 {{ RELATE {pe_key}->{edge_id}->{owner_key}; }};"
            ));
        }
    }

    let mut written = 0usize;
    for chunk in pe_jsons.chunks(INSERT_CHUNK) {
        crate::surreal_retry::execute_generation_preload(
            &format!("INSERT IGNORE INTO pe [{}];", chunk.join(",")),
            "preload ancestor pe",
        )
        .await?;
        written += chunk.len();
    }
    for chunk in att_statements.chunks(INSERT_CHUNK) {
        crate::surreal_retry::execute_generation_preload(
            &chunk.join("\n"),
            "preload ancestor attributes",
        )
        .await?;
        written += chunk.len();
    }
    for chunk in uda_jsons.chunks(INSERT_CHUNK) {
        crate::surreal_retry::execute_generation_preload(
            &format!("INSERT IGNORE INTO ATT_UDA [{}];", chunk.join(",")),
            "preload ancestor UDA",
        )
        .await?;
        written += chunk.len();
    }
    for chunk in edge_statements.chunks(INSERT_CHUNK) {
        crate::surreal_retry::execute_generation_preload(
            &chunk.join("\n"),
            "preload ancestor pe_owner",
        )
        .await?;
        written += chunk.len();
    }
    println!(
        "暂存祖先预载: seeds={} elements={} written={written}（pe={} 名词行={} uda={} 链边={}）",
        closure.seed_hops.len(),
        closure.elements.len(),
        pe_jsons.len(),
        att_statements.len(),
        uda_jsons.len(),
        edge_statements.len()
    );
    Ok(written)
}

/// 装载后的完整性验证（D8 的后半）：闭包里每个元素在暂存里**行在、owner 对、
/// `refno` 链接可解引用**。链通到顶与链深预算已在 [`resolve_ancestor_closure`]
/// 里用 Rust 侧迭代上溯验过（刻意不经 `fn::ancestor`——不拿被测物验证被测物）；
/// 这里验的是「解析结果确实落进了暂存且没被写歪」。窗口外是无操作。
pub(crate) async fn validate_ancestor_preload(closure: &AncestorClosure) -> anyhow::Result<()> {
    if super::active_staging_writes().is_none() || closure.elements.is_empty() {
        return Ok(());
    }
    validate_ancestor_preload_on(&super::active_data_db(), closure).await
}

pub(crate) async fn validate_ancestor_preload_on(
    db: &Surreal<Any>,
    closure: &AncestorClosure,
) -> anyhow::Result<()> {
    const PROBE_CHUNK: usize = 200;
    for chunk in closure.elements.chunks(PROBE_CHUNK) {
        let probes = chunk
            .iter()
            .map(|element| {
                let pe_key = element.refno.to_pe_key();
                let owner_key = element.owner.to_pe_key();
                format!(
                    "[record::exists({pe_key}), {pe_key}.owner == {owner_key}, \
                     {pe_key}.refno.TYPE != NONE]"
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let mut response = db
            .query(format!("RETURN [{probes}];"))
            .await?
            .check()?;
        let flags: Vec<Vec<bool>> = response.take(0)?;
        anyhow::ensure!(
            flags.len() == chunk.len(),
            "祖先预载验证探针返回 {} 组结果，期望 {}",
            flags.len(),
            chunk.len()
        );
        for (element, checks) in chunk.iter().zip(&flags) {
            let pe_key = element.refno.to_pe_key();
            anyhow::ensure!(
                checks.first().copied().unwrap_or(false),
                "祖先预载验证失败：{pe_key} 不在暂存里（装载被跳过或被回滚）"
            );
            anyhow::ensure!(
                checks.get(1).copied().unwrap_or(false),
                "祖先预载验证失败：{pe_key} 在暂存里的 owner 与文件态不一致\
                 （期望 {}）——链在暂存里走不到顶",
                element.owner.to_pe_key()
            );
            anyhow::ensure!(
                checks.get(2).copied().unwrap_or(false),
                "祖先预载验证失败：{pe_key} 的 refno 链接解引用不到名词表行\
                 ——窗口内的 ancestor/transform 读取会把它当 (0,0,0)（静默错模型）"
            );
        }
    }
    Ok(())
}

/// 测试夹具（本模块单测与 parity 扩展共用）：内存 lookup + 标准四层链。
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use aios_core::NamedAttrValue;

    /// 与仓内其它夹具同一保留段（4000000001），序号取 78 万段避开 issue10 /
    /// room_fixture / staged_transform 系列——`GLOBAL_AABB_TREE` 是进程级共享。
    pub(crate) fn refu(n: u64) -> RefU64 {
        RefU64((4000000001u64 << 32) | n)
    }

    pub(crate) fn att_of(
        refno: RefU64,
        owner: RefU64,
        noun: &str,
        name: &str,
        pos: Option<[f32; 3]>,
        sesno: i32,
    ) -> NamedAttrMap {
        let mut att = NamedAttrMap::default();
        att.map
            .insert("TYPE".into(), NamedAttrValue::StringType(noun.into()));
        att.map
            .insert("NAME".into(), NamedAttrValue::StringType(name.into()));
        att.map
            .insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
        att.map
            .insert("OWNER".into(), NamedAttrValue::RefU64Type(owner));
        att.map
            .insert("SESNO".into(), NamedAttrValue::IntegerType(sesno));
        if let Some(pos) = pos {
            att.map
                .insert("POS".into(), NamedAttrValue::F32VecType(pos.to_vec()));
        }
        att
    }

    pub(crate) fn element(
        refno: RefU64,
        owner: RefU64,
        noun: &str,
        name: &str,
        pos: Option<[f32; 3]>,
        sesno: i32,
        children: &[RefU64],
    ) -> AncestorElement {
        AncestorElement {
            refno,
            owner,
            att: att_of(refno, owner, noun, name, pos, sesno),
            children: children.to_vec(),
        }
    }

    pub(crate) fn lookup_from(
        map: HashMap<RefU64, AncestorElement>,
    ) -> impl FnMut(RefU64) -> std::future::Ready<anyhow::Result<Option<AncestorElement>>> {
        move |refno| std::future::ready(Ok(map.get(&refno).cloned()))
    }

    /// 标准四层链（POS 位移可辨识，合成期望 [1000, 500, 7]）：
    /// WORL(base+1) ← SITE(base+2, POS z=7) ← ZONE(base+3, POS y=500) ←
    /// EQUI(base+4, POS x=1000)。返回 (lookup 表, WORL, EQUI)。
    pub(crate) fn world_chain(base: u64) -> (HashMap<RefU64, AncestorElement>, RefU64, RefU64) {
        let worl = refu(base + 1);
        let site = refu(base + 2);
        let zone = refu(base + 3);
        let equi = refu(base + 4);
        let map = HashMap::from([
            (
                worl,
                element(worl, RefU64(0), "WORL", "/*", None, 1, &[site]),
            ),
            (
                site,
                element(
                    site,
                    worl,
                    "SITE",
                    "/ZZAP-SITE",
                    Some([0.0, 0.0, 7.0]),
                    1,
                    &[zone],
                ),
            ),
            (
                zone,
                element(
                    zone,
                    site,
                    "ZONE",
                    "/ZZAP-ZONE",
                    Some([0.0, 500.0, 0.0]),
                    1,
                    &[equi],
                ),
            ),
            (
                equi,
                element(
                    equi,
                    zone,
                    "EQUI",
                    "/ZZAP-EQUI",
                    Some([1000.0, 0.0, 0.0]),
                    2,
                    &[],
                ),
            ),
        ]);
        (map, worl, equi)
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{element, lookup_from, refu, world_chain};
    use super::*;
    use crate::data_interface::staging::ResourceThresholds;
    use crate::data_interface::staging::lifecycle::create_window_on;
    use aios_core::NamedAttrValue;
    use aios_core::options::DbOption;
    use surrealdb::engine::any::connect;

    /// WORL(781001) ← SITE(781002) ← ZONE(781003) ← EQUI(781004)
    fn world_fixture() -> (HashMap<RefU64, AncestorElement>, RefU64, RefU64) {
        world_chain(781000)
    }

    /// W5.1 红先主用例：暂存窗口 Transform 目标的祖先 ZONE/SITE 带真 POS 且未被
    /// 本窗口解析触及——修复前（无祖先预载）世界变换丢这两段位移（静默零），
    /// 修复后等于绝对真值 [1000, 500, 7]。
    ///
    /// 同时钉 D3 的 StagingOnly：预载行绝不进 journal（journal 里只允许出现
    /// Transform 刷新自己的 Both 写）。
    #[tokio::test(flavor = "multi_thread")]
    async fn staged_transform_composes_ancestor_positions_after_parse_preload() {
        let (map, worl, equi_ref) = world_fixture();
        let equi = RefnoEnum::from(equi_ref);
        let equi_pe = equi.to_pe_key();
        let equi_inst = equi.to_inst_relate_key();

        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 7981, 3, 3, ResourceThresholds::default())
            .await
            .expect("create window");

        // 窗口解析写入的形态：只有目标自己的 pe + 名词表行 + 既有产物在暂存——
        // 祖先一个字都没有（pe+pe_owner 持久层拷贝退役后的真实起点）。
        window
            .staging_db()
            .query(format!(
                "UPSERT {equi_pe} CONTENT {{ noun: 'EQUI', deleted: false, owner: pe:4000000001_781003, refno: EQUI:⟨4000000001_781004⟩ }};\
                 UPSERT EQUI:⟨4000000001_781004⟩ CONTENT {{ TYPE: 'EQUI', NAME: '/ZZAP-EQUI', POS: [1000.0, 0.0, 0.0] }};\
                 CREATE trans:zzap_old SET d = {{ translation: [0.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0] }};\
                 CREATE aabb:zzap_geo SET d = {{ mins: [0.0, 0.0, 0.0], maxs: [100.0, 100.0, 100.0] }};\
                 CREATE inst_info:zzap_geo;\
                 CREATE inst_geo:zzap_geo SET meshed = true, visible = true, aabb = aabb:zzap_geo;\
                 RELATE inst_info:zzap_geo->geo_relate->inst_geo:zzap_geo \
                     SET trans = trans:zzap_old, geo_type = 'Pos', visible = true;\
                 RELATE {equi_pe}->{equi_inst}->inst_info:zzap_geo \
                     SET world_trans = trans:zzap_old, aabb = aabb:zzap_geo, solid = true, generic = 'EQUI';"
            ))
            .await
            .expect("plant staged fixture")
            .check()
            .expect("staged fixture applied");

        let closure = resolve_ancestor_closure(&[equi_ref], worl, 3, lookup_from(map))
            .await
            .expect("resolve ancestor closure");
        assert_eq!(closure.elements.len(), 4, "EQUI + ZONE + SITE + WORL");
        assert_eq!(closure.seed_hops.get(&equi_ref), Some(&3));

        window
            .scope(async {
                let written = apply_ancestor_preload(&closure, 7981).await?;
                anyhow::ensure!(written > 0, "预载必须真的写了东西");
                validate_ancestor_preload(&closure).await?;
                crate::data_interface::increment_manager::refresh_world_transform_products(
                    &DbOption::default(),
                    &[equi],
                )
                .await
            })
            .await
            .expect("暂存 Transform 必须全程只写暂存与 journal（SUL_DB 未连接，直写即错）");

        // 世界变换 = EQUI(1000,0,0) ∘ ZONE(0,500,0) ∘ SITE(0,0,7)，绝对值断言——
        // 不是「变了」，是「对了」。修复前 ZONE/SITE 名词行缺失，位移被静默当 0，
        // 算出的是 [1000, 0, 0]。
        let mut response = window
            .staging_db()
            .query(format!("RETURN {equi_inst}.world_trans.d.translation;"))
            .await
            .expect("read staged world trans")
            .check()
            .expect("valid staged world trans query");
        let translation: Vec<f64> = response.take(0).expect("take translation");
        assert_eq!(
            translation,
            vec![1000.0, 500.0, 7.0],
            "世界变换必须合成完整祖先链的位移"
        );

        // D3：预载行（pe / 名词表 / 链边）不进 journal。
        let journal = window.journal().await;
        for entry in &journal {
            assert!(
                !entry.sql.contains("INSERT IGNORE INTO pe [")
                    && !entry.sql.contains("INSERT IGNORE INTO ZONE")
                    && !entry.sql.contains("INSERT IGNORE INTO SITE")
                    && !entry.sql.contains("INSERT IGNORE INTO WORL")
                    && !entry.sql.contains("UPSERT ZONE:")
                    && !entry.sql.contains("UPSERT SITE:")
                    && !entry.sql.contains("UPSERT WORL:")
                    && !entry.sql.contains("RELATE pe:")
                    || entry.sql.contains("INSERT IGNORE INTO trans"),
                "祖先预载行不得进 journal（StagingOnly）: {}",
                entry.sql
            );
        }
        window.drop_database().await.expect("cleanup");
    }

    /// 红形态的负向对照：**不做**祖先预载时，同一夹具算出的世界变换把 ZONE/SITE
    /// 的位移静默当 (0,0,0)——这是 W1 修的那个洞的活体标本。哪天 rs-core 把
    /// 「祖先名词行缺失」改成响亮失败，本用例会翻红提醒重新审视预载的失败面。
    #[tokio::test(flavor = "multi_thread")]
    async fn without_ancestor_preload_the_offsets_are_silently_zero() {
        let equi_ref = refu(782004);
        let equi = RefnoEnum::from(equi_ref);
        let equi_pe = equi.to_pe_key();
        let equi_inst = equi.to_inst_relate_key();

        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 7980, 3, 3, ResourceThresholds::default())
            .await
            .expect("create window");

        // 旧 mutation 预载的形态：祖先 pe 行在（从持久层拷来），名词表行不在。
        window
            .staging_db()
            .query(format!(
                "UPSERT pe:4000000001_782001 CONTENT {{ noun: 'WORL', deleted: false, refno: WORL:⟨4000000001_782001⟩ }};\
                 UPSERT pe:4000000001_782002 CONTENT {{ noun: 'SITE', deleted: false, owner: pe:4000000001_782001, refno: SITE:⟨4000000001_782002⟩ }};\
                 UPSERT pe:4000000001_782003 CONTENT {{ noun: 'ZONE', deleted: false, owner: pe:4000000001_782002, refno: ZONE:⟨4000000001_782003⟩ }};\
                 UPSERT {equi_pe} CONTENT {{ noun: 'EQUI', deleted: false, owner: pe:4000000001_782003, refno: EQUI:⟨4000000001_782004⟩ }};\
                 UPSERT EQUI:⟨4000000001_782004⟩ CONTENT {{ TYPE: 'EQUI', NAME: '/ZZAQ-EQUI', POS: [1000.0, 0.0, 0.0] }};\
                 CREATE trans:zzaq_old SET d = {{ translation: [0.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0] }};\
                 CREATE aabb:zzaq_geo SET d = {{ mins: [0.0, 0.0, 0.0], maxs: [100.0, 100.0, 100.0] }};\
                 CREATE inst_info:zzaq_geo;\
                 CREATE inst_geo:zzaq_geo SET meshed = true, visible = true, aabb = aabb:zzaq_geo;\
                 RELATE inst_info:zzaq_geo->geo_relate->inst_geo:zzaq_geo \
                     SET trans = trans:zzaq_old, geo_type = 'Pos', visible = true;\
                 RELATE {equi_pe}->{equi_inst}->inst_info:zzaq_geo \
                     SET world_trans = trans:zzaq_old, aabb = aabb:zzaq_geo, solid = true, generic = 'EQUI';"
            ))
            .await
            .expect("plant staged fixture")
            .check()
            .expect("staged fixture applied");

        window
            .scope(
                crate::data_interface::increment_manager::refresh_world_transform_products(
                    &DbOption::default(),
                    &[equi],
                ),
            )
            .await
            .expect("refresh runs");

        let mut response = window
            .staging_db()
            .query(format!("RETURN {equi_inst}.world_trans.d.translation;"))
            .await
            .expect("read staged world trans")
            .check()
            .expect("valid query");
        let translation: Vec<f64> = response.take(0).expect("take translation");
        assert_eq!(
            translation,
            vec![1000.0, 0.0, 0.0],
            "祖先名词行缺失时位移被静默当零——W1 修的正是这个"
        );
        window.drop_database().await.expect("cleanup");
    }

    /// D8 断链：链上任何元素解析不到 → 整批失败，错误里带断点与种子。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_broken_chain_fails_closed_with_the_break_point() {
        let (mut map, worl, equi) = world_fixture();
        map.remove(&refu(781002)); // 抽掉 SITE
        let error = resolve_ancestor_closure(&[equi], worl, 3, lookup_from(map))
            .await
            .expect_err("断链必须失败");
        let message = error.to_string();
        assert!(message.contains("4000000001_781002"), "{message}");
        assert!(message.contains("4000000001_781004"), "{message}");
    }

    /// 事实基线 7 的封口：链上元素的会话号超过窗口终点（read-your-future）→ 拒绝。
    #[tokio::test(flavor = "multi_thread")]
    async fn an_ancestor_touched_after_the_window_end_is_refused() {
        let (mut map, worl, equi) = world_fixture();
        map.get_mut(&refu(781003)).unwrap().att.map.insert(
            "SESNO".into(),
            NamedAttrValue::IntegerType(4), // 窗口终点是 3
        );
        let error = resolve_ancestor_closure(&[equi], worl, 3, lookup_from(map))
            .await
            .expect_err("越窗会话必须拒绝");
        let message = error.to_string();
        assert!(message.contains("窗口终点"), "{message}");
        assert!(message.contains("4000000001_781003"), "{message}");
    }

    /// D9 探针：到顶恰好 9 跳压线合法；第 10 跳必须响亮失败并给修法。
    #[tokio::test(flavor = "multi_thread")]
    async fn the_nine_hop_budget_is_a_loud_probe_not_a_silent_truncation() {
        // 深链：worl=783000，783001 ← … ← 783009（9 跳）← 783010（10 跳）。
        let worl = refu(783000);
        let mut map = HashMap::from([(
            worl,
            element(worl, RefU64(0), "WORL", "/*", None, 1, &[refu(783001)]),
        )]);
        for n in 1..=10u64 {
            let refno = refu(783000 + n);
            let owner = refu(783000 + n - 1);
            map.insert(
                refno,
                element(
                    refno,
                    owner,
                    "STRU",
                    &format!("/DEEP-{n}"),
                    None,
                    1,
                    &[refu(783000 + n + 1)],
                ),
            );
        }

        let at_budget = refu(783009);
        let closure = resolve_ancestor_closure(&[at_budget], worl, 3, lookup_from(map.clone()))
            .await
            .expect("恰 9 跳压线必须通过");
        assert_eq!(closure.seed_hops.get(&at_budget), Some(&9));

        let over_budget = refu(783010);
        let error = resolve_ancestor_closure(&[over_budget], worl, 3, lookup_from(map))
            .await
            .expect_err("第 10 跳必须失败");
        let message = error.to_string();
        assert!(message.contains("fn::ancestor"), "{message}");
        assert!(message.contains("修法"), "{message}");
    }

    /// 链没停在本库 WORL（owner 数据异常）→ 拒绝。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_chain_ending_off_the_world_root_is_refused() {
        let (map, _worl, equi) = world_fixture();
        let error = resolve_ancestor_closure(&[equi], refu(999999), 3, lookup_from(map))
            .await
            .expect_err("终点不是 WORL 必须失败");
        assert!(error.to_string().contains("WORL"), "{error:#}");
    }

    /// 名词行渲染不出（TYPE 缺失）→ 装载阶段整批失败：缺名词行正是 W1 要修的
    /// 静默零，绝不能带着「pe 有、名词行无」的半套数据继续。
    #[tokio::test(flavor = "multi_thread")]
    async fn an_ancestor_without_a_type_is_refused_at_apply() {
        let (mut map, worl, equi) = world_fixture();
        map.get_mut(&refu(781003)).unwrap().att.map.remove("TYPE");
        let closure = resolve_ancestor_closure(&[equi], worl, 3, lookup_from(map))
            .await
            .expect("walk 本身不看 TYPE");

        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 7977, 3, 3, ResourceThresholds::default())
            .await
            .expect("create window");
        let error = window
            .scope(apply_ancestor_preload(&closure, 7977))
            .await
            .expect_err("TYPE 缺失必须失败");
        assert!(error.to_string().contains("TYPE"), "{error:#}");
        window.drop_database().await.expect("cleanup");
    }

    /// owner 成员表不含该子 → 装载阶段拒绝伪造槽位。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_child_missing_from_its_owner_member_block_is_refused_at_apply() {
        let (mut map, worl, equi) = world_fixture();
        map.get_mut(&refu(781003)).unwrap().children.clear(); // ZONE 的成员表抹掉 EQUI
        let closure = resolve_ancestor_closure(&[equi], worl, 3, lookup_from(map))
            .await
            .expect("walk 本身不看成员表");

        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 7979, 3, 3, ResourceThresholds::default())
            .await
            .expect("create window");
        let error = window
            .scope(apply_ancestor_preload(&closure, 7979))
            .await
            .expect_err("成员表不含子必须失败");
        assert!(error.to_string().contains("成员表不含"), "{error:#}");
        window.drop_database().await.expect("cleanup");
    }

    /// 实机 7997@194 复盘钉（2026-08-08）：房间/产物预载会把持久层的旧链边
    /// （**旧槽位**）拷进暂存，祖先预载随后按**文件**槽位渲染同一条逻辑边——守卫
    /// 必须盖住 `(in, out)` 这套身份。只查记录 id 的旧守卫会带着新槽位撞
    /// `unique_pe_owner`，整批 fail-closed（水位不动、窗口废弃，但这批模型工作
    /// 永远开不了工）。修后：逻辑边已在（任意槽位）→ 跳过不写、不重槽、不报错；
    /// 链上其余缺失的边照常补齐；owner 字段正确性验证不受影响。
    #[tokio::test(flavor = "multi_thread")]
    async fn an_edge_parked_at_an_old_slot_is_skipped_not_a_unique_index_hit() {
        let (map, worl, equi) = world_chain(786500);
        let closure = resolve_ancestor_closure(&[equi], worl, 3, lookup_from(map))
            .await
            .expect("resolve ancestor closure");

        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 7976, 3, 3, ResourceThresholds::default())
            .await
            .expect("create window");

        // 模拟房间/产物预载从持久层拷来的旧态：SITE→WORL 的逻辑边停在旧槽位 7
        //（文件态里它在槽位 0——WORL 成员表只有这一个 SITE）。
        let site_key = refu(786502).to_pe_key();
        let worl_key = refu(786501).to_pe_key();
        window
            .staging_db()
            .query(format!(
                "INSERT RELATION INTO pe_owner [{{ id: pe_owner:[{worl_key}, 7], \
                 in: {site_key}, out: {worl_key} }}];"
            ))
            .await
            .expect("plant stale edge")
            .check()
            .expect("stale edge planted");

        window
            .scope(async {
                apply_ancestor_preload(&closure, 7976).await?;
                validate_ancestor_preload(&closure).await
            })
            .await
            .expect("逻辑边已在（旧槽位）时预载必须跳过而不是撞 unique_pe_owner");

        // 旧边原样保留在槽位 7、文件槽位 0 没被重写；(site, worl) 只此一条；
        // 链上其余缺失的边（EQUI→ZONE、ZONE→SITE）照常补齐。
        let mut response = window
            .staging_db()
            .query(format!("RETURN record::exists(pe_owner:[{worl_key}, 7]);"))
            .query(format!("RETURN record::exists(pe_owner:[{worl_key}, 0]);"))
            .query(format!(
                "RETURN array::len((SELECT VALUE id FROM pe_owner \
                 WHERE in = {site_key} AND out = {worl_key}));"
            ))
            .query("RETURN array::len((SELECT VALUE id FROM pe_owner));")
            .await
            .expect("probe edges")
            .check()
            .expect("valid edge probe");
        let stale_kept: Option<bool> = response.take(0).expect("stale slot probe");
        let file_slot_written: Option<bool> = response.take(1).expect("file slot probe");
        let logical_edges: Option<i64> = response.take(2).expect("logical pair count");
        let total_edges: Option<i64> = response.take(3).expect("total edge count");
        assert_eq!(stale_kept, Some(true), "旧槽位边必须原样保留");
        assert_eq!(
            file_slot_written,
            Some(false),
            "文件槽位不得重写（预载不改写窗口前旧态）"
        );
        assert_eq!(logical_edges, Some(1), "(in, out) 这条逻辑边只许有一条");
        assert_eq!(
            total_edges,
            Some(3),
            "EQUI→ZONE、ZONE→SITE 两条缺边照常补齐 + 旧边一条 = 3"
        );
        window.drop_database().await.expect("cleanup");
    }

    /// 实机 7997@194 复盘钉（2026-08-08，第二层）：Modified 元素的窗口主数据落库
    /// 是 `UPSERT {noun}:{id} MERGE {只含本会话改动的属性}`——持久层上它合并进完整
    /// 旧行，在空白暂存库里它**从无到有创建出残行**（无 TYPE/NAME/未变属性）。
    /// 预载必须 MERGE 补齐缺失字段而不是 INSERT IGNORE 跳过：跳过的话
    /// `pe.refno.TYPE` 解引用为 NONE（完整性验证响亮失败、整批不开工），而未变的
    /// ORI/POS 会被窗口内读取当默认值——静默错模型从祖先挪到目标自己。
    /// 补齐同时不得动窗口写下的新值（同一份文件字节 + sesno 封口，值恒等）。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_partial_noun_row_from_the_window_merge_is_backfilled_not_ignored() {
        let (map, worl, equi) = world_chain(787500);
        let closure = resolve_ancestor_closure(&[equi], worl, 3, lookup_from(map))
            .await
            .expect("resolve ancestor closure");

        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 7974, 3, 3, ResourceThresholds::default())
            .await
            .expect("create window");

        // 窗口主数据先落了 Modified 残行：只有本会话的新 POS，没有 TYPE/NAME
        //（与 pdms_io `to_modify_surql` 的真实形状同构）。
        let equi_key = refu(787504).to_table_key("EQUI");
        window
            .staging_db()
            .query(format!(
                "UPSERT {equi_key} MERGE {{ POS: [1000.0, 0.0, 0.0] }};"
            ))
            .await
            .expect("plant partial noun row")
            .check()
            .expect("partial row planted");

        window
            .scope(async {
                apply_ancestor_preload(&closure, 7974).await?;
                validate_ancestor_preload(&closure).await
            })
            .await
            .expect("残行必须被 MERGE 补齐，验证必须通过");

        let mut response = window
            .staging_db()
            .query(format!("RETURN {equi_key}.TYPE;"))
            .query(format!("RETURN {equi_key}.NAME;"))
            .query(format!("RETURN {equi_key}.POS;"))
            .await
            .expect("probe backfilled row")
            .check()
            .expect("valid backfill probe");
        let noun_type: Option<String> = response.take(0).expect("TYPE");
        let name: Option<String> = response.take(1).expect("NAME");
        let pos: Vec<f64> = response.take(2).expect("POS");
        assert_eq!(noun_type.as_deref(), Some("EQUI"), "TYPE 必须被补齐");
        assert_eq!(name.as_deref(), Some("/ZZAP-EQUI"), "NAME 必须被补齐");
        assert_eq!(
            pos,
            vec![1000.0, 0.0, 0.0],
            "窗口写下的 POS 新值必须原样在场"
        );
        window.drop_database().await.expect("cleanup");
    }

    /// 验证是独立的后置探针：没装载就验证 → 必须点名缺行。
    #[tokio::test(flavor = "multi_thread")]
    async fn validation_fails_when_rows_never_landed() {
        let (map, worl, equi) = world_fixture();
        let closure = resolve_ancestor_closure(&[equi], worl, 3, lookup_from(map))
            .await
            .expect("resolve");

        let staging = connect("mem://").await.expect("mem boots");
        staging
            .use_ns("test")
            .use_db("validate_empty")
            .await
            .expect("use db");
        let error = validate_ancestor_preload_on(&staging, &closure)
            .await
            .expect_err("空暂存必须验不过");
        assert!(error.to_string().contains("不在暂存里"), "{error:#}");
    }

    /// D2 的种子口径（regen 同形用例）：Transform 目标 + Transform 子树模型节点 +
    /// RegenRoot + 新单元根都在；DeleteCleanup 与房间目标不在。
    #[test]
    fn ancestor_seeds_cover_all_model_work_but_never_deletes() {
        use crate::data_interface::model_update_plan::{ModelWorkAction, ModelWorkItem};

        let item = |action: ModelWorkAction, target: &str| ModelWorkItem {
            dbnum: 7997,
            db_type: "DESI".into(),
            source_end_sesno: 3,
            action,
            target_refno: target.into(),
            noun: "EQUI".into(),
        };
        let plan_items = vec![
            item(ModelWorkAction::Transform, "4000000001/784001"),
            item(ModelWorkAction::DeleteCleanup, "4000000001/784002"),
            item(ModelWorkAction::RegenRoot, "4000000001/784003"),
            item(ModelWorkAction::RoomRecalcElement, "4000000001/784004"),
        ];
        let new_units = vec![crate::data_interface::manual_update::UnitTask {
            dbnum: 7997,
            root_refno: "4000000001/784005".into(),
            noun: "BRAN".into(),
            source_end_sesno: 3,
            attempts: 0,
            revision: None,
            old_owner: None,
            new_owner: None,
        }];
        let transform_targets = vec![RefnoEnum::from("4000000001/784001")];
        let transform_models = vec![RefnoEnum::from("4000000001/784006")];

        let seeds = ancestor_seed_refnos(
            &plan_items,
            &new_units,
            &transform_targets,
            &transform_models,
        );

        let expect_in = ["784001", "784003", "784005", "784006"];
        for suffix in expect_in {
            assert!(
                seeds.iter().any(|seed| seed.to_pe_key().ends_with(suffix)),
                "种子必须含 {suffix}: {seeds:?}"
            );
        }
        assert!(
            !seeds.iter().any(|seed| seed.to_pe_key().ends_with("784002")),
            "删除目标已从文件消失，绝不进解析种子: {seeds:?}"
        );
        assert!(
            !seeds.iter().any(|seed| seed.to_pe_key().ends_with("784004")),
            "房间目标不进祖先种子: {seeds:?}"
        );
    }
}
