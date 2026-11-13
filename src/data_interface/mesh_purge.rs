//! 清理模型「一并删除网格文件」那一格（2026-10-22）：范围的产物行删掉之后，这片范围引用过的
//! 内容寻址几何——库里的 `inst_geo` 行与盘上的 `.mesh`——也一起清掉，靠重新生成写回。
//!
//! 两个范围各一个收集口，同一份口径：右键「清理模型」是**子树**（[`scope_mesh_ids`]，
//! `DELETE /model/subtree`），库行「清理模型（整库）」是**整个库**（[`dbnum_mesh_ids`]，
//! `DELETE /dbnums/{dbnum}/model`）。两条删除例程（`on_demand_model::AiosDBManager::clear_model_scope`
//! 与 `fast_delete::wipe_dbnum_model_face`）按 [`ClearGeometry`] 决定要不要连几何一起清；HTTP 层只传
//! 开关，自己不收键、不删文件。右键「重新生成模型」（`force = true`）的先删一律连几何清——马上
//! 就要重新生成，写回的就是新的。
//!
//! **收集必须在删行之前**：行一删掉，库与投影都再也说不出这片范围引用过哪些网格。收集失败整趟中止
//! （一行不删）：宁可什么都不做，也不要在盘上留下「产物删了、几何还在」或反过来的一半。
//! **删几何排在删行之后**：反过来会先把模型还要用的几何删掉、而产物还在。
//!
//! 网格是**内容寻址**的：同一个键可能被这片范围之外的其他模型复用。这里**不做跨范围引用计数**
//! （2026-10-22 用户定）：删的是「本范围引用过的全部」，别处还在用的那份随这次没了，等那片范围
//! 自己重新生成时写回（`pdms_inst::render_inst_geo_upsert` + `e3d_mesh_store::ensure_mesh_file`）。
//! 这句话说的是**烘焙键**（一件一份的成品网格，库上没有 `inst_geo` 行、没有边）：丢的只是一个文件，
//! 重新生成就回来。下面三种按形状共用的几何不在这个口径里，删了丢的是别人的行。
//!
//! 头一种是**单位几何**（[`shared_unit_keys`]）：全项目所有隐含直管段共用 `inst_geo:⟨2⟩`
//! （`TUBI_GEO_HASH`）/ `⟨1⟩`（`BOXI_GEO_HASH`），单位球是 `3`；`1/2/3.mesh` 没有任何生成路径会写回
//! （`ensure_mesh_file` 只写 baked 键，`fill_basic_shapes` 只写回行），删了就是全库所有管段 404。
//! 这三枚行与文件一律不删，收到了也跳过。
//!
//! 同一个理由，**管身的单位几何不进删除集**：`tubi_relate.out` 指的是按段数共用的单位圆柱
//! （`unit_cylinder_identity(段数)`，实测一个键被几百到几千根管段引用，跨库），重生成写回的还是同一个键、
//! 同一份文件，删它只会让别的库的管段白白 404 一阵。
//!
//! **标准图元键也不进删除集**（GitHub #76，2026-11-11）。标准图元（`ProjectedShape::Canonical`：BOX、CYLI 一类）
//! 的网格键只由形状定——全库的 BOX 共用 `BoxV1` 那一行 `inst_geo`，圆柱按段数共用，而且用的就是管身那一套
//! 圆柱键。它们与上面两种是同一种东西，却经 `inst_relate.insts_flat.geo_hash` 进过删除集，后果比 404 重得多：
//! `geo_relate` / `tubi_relate` 是 `out` 指着 `inst_geo` 的**边**，SurrealDB 删节点时把它身上的边一起删掉——
//! 重生成一台带 BOX 的设备，全库别的 BOX 的 `geo_relate` 行就没了；带的是 CYLI，同键的管身 `tubi_relate` 行
//! 也跟着没（2026-10-27 增量夹具的 `data` / `geometry` 两例「恢复后多一条 `Pos`」就是基线被它删过）。
//! 所以收集不收标准图元的键：投影里只收 `ProjectedShape::Baked` 的非管身记录；库上 `inst_relate` 照旧收
//! `booled_id` 与 `insts_flat.geo_hash`，但模型面写的、带 `out`（挂着 `inst_info`）的行——那就是标准图元——
//! 的 `geo_hash` 不收。删行那一步另有一道闸（[`delete_inst_geo_rows`]）：还有边指着的 `inst_geo` 行不删，
//! 它的 `.mesh` 也留着——键是怎么进删除集的都一样（老生成链的行、脏行、将来别的收集口）。
//!
//! 不碰 `aabb` / `trans` / `vec3` 这几张内容寻址小表（一直按「泄漏有界、不回收」处理，见
//! `helper::render_cascade_delete` 的注释），也不碰 `meshes/` 下的非网格目录（房间输入缓存、
//! `model-input-cache` 之类）。唯一跟着键走进子目录的是显示用粗网格 `lod1-v1/{键}.mesh` 及其
//! `.none` 标记（ADR-096）：它借 L0 的名字，L0 没了它就是孤儿，随 L0 一并摘掉。

use std::collections::BTreeSet;
use std::path::Path;

use aios_core::RefnoEnum;
use aios_core::prim_geo::basic::{BOXI_GEO_HASH, TUBI_GEO_HASH};
use serde::Deserialize;
use surrealdb::{Surreal, engine::any::Any};

use crate::fast_model::model_memory_store::{
    ModelMemoryStore, ProjectedGeometry, ProjectedShape, ProjectionScope,
};

/// 清理 / 先删时要不要连几何（`inst_geo` 行 + `.mesh`）一起清。
///
/// 名义上 `pub`：它出现在 `clear_model_scope` / `wipe_dbnum_model_face` 这两个 `pub fn` 的签名里；模块本身
/// 是 `pub(crate)`，仓外拿不到它。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearGeometry {
    /// 只删产物行（`purge_meshes` 不勾时的老口径）。
    Keep,
    /// 产物行删完再清这片范围引用过的几何（`purge_meshes=true`、`force = true` 的先删）。
    Purge,
}

impl ClearGeometry {
    /// 请求上那一格→开关。
    pub(crate) fn from_flag(purge_meshes: bool) -> Self {
        if purge_meshes {
            Self::Purge
        } else {
            Self::Keep
        }
    }

    pub(crate) fn purges(self) -> bool {
        matches!(self, Self::Purge)
    }
}

/// 删完之后的账。失败的逐条点名（键 + 原因），不静默。名义上 `pub` 的理由同 [`ClearGeometry`]
/// （`clear_model_scope` 的返回值、`ModelFaceWipeResult::geometry`）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MeshPurgeReport {
    /// 真从盘上摘掉的 `.mesh`。
    pub removed: usize,
    /// 本来就不在盘上：重复清理、或已被别的路径删过。不是错。
    pub absent: usize,
    /// 删失败的 `(键, 原因)`。
    pub failed: Vec<(String, String)>,
    /// 发出 `DELETE` 的 `inst_geo` 行数（按键数计：对不存在的行 `DELETE` 是空操作）。
    /// 只写内存 / 读透的库没有权威行可删，恒为 0。
    pub inst_geo_removed: usize,
    /// 键集里撞上单位几何、按规矩跳过的个数（行与文件都没动）。
    pub shared_unit_skipped: usize,
    /// 键集里库上还有 `geo_relate` / `tubi_relate` 边指着的 `inst_geo` 行数：别处还在用，行与 `.mesh` 都留着
    /// （[`delete_inst_geo_rows`] 的那道闸）。只写内存 / 读透的库没有权威行可问，恒为 0。
    pub in_use_kept: usize,
    /// 随 L0 一起摘掉的显示用粗网格（`lod1-v1/{键}.mesh`）个数；`.none` 标记一并删但不计数。
    /// 粗网格借 L0 的名字，L0 没了它就是孤儿，不留。
    pub lod1_removed: usize,
}

impl MeshPurgeReport {
    /// 日志里那半句。勾了这一格就要说出来删了几个——包括「一个都没删到」。
    pub(crate) fn note(&self) -> String {
        let mut note = format!("一并删除网格文件 {} 个", self.removed);
        if self.inst_geo_removed > 0 {
            note.push_str(&format!("、几何行 {} 行", self.inst_geo_removed));
        }
        if self.lod1_removed > 0 {
            note.push_str(&format!("、粗网格 {} 个", self.lod1_removed));
        }
        if self.in_use_kept > 0 {
            note.push_str(&format!("、别处还在用留下 {} 个", self.in_use_kept));
        }
        if self.absent > 0 {
            note.push_str(&format!("（另 {} 个本来就不在盘上）", self.absent));
        }
        if !self.failed.is_empty() {
            note.push_str(&format!("，{} 个删失败", self.failed.len()));
        }
        note
    }
}

/// 单位球那一枚：`mesh_glb::repository_unit_meshes_convert_end_to_end` 点名的 `assets/meshes/3.mesh`；
/// aios-core 没有给它常量名（`1` / `2` 是 `BOXI_GEO_HASH` / `TUBI_GEO_HASH`）。
const UNIT_SPHERE_KEY: &str = "3";

/// 单位几何的键：全项目共用、没有生成路径会写回，行与文件一律不删。
pub(crate) fn shared_unit_keys() -> [String; 3] {
    [
        BOXI_GEO_HASH.to_string(),
        TUBI_GEO_HASH.to_string(),
        UNIT_SPHERE_KEY.to_string(),
    ]
}

pub(crate) fn is_shared_unit_key(key: &str) -> bool {
    shared_unit_keys().iter().any(|unit| unit == key)
}

/// 一片范围引用过的网格键：库上一份 + 进程内投影一份。
///
/// `include_durable` = 这一片走摄入形态（库是权威面）。「只写内存」/读透的库没有权威行可查
/// ——也不该在 `scope_memory_only` 作用域里回头去查那个库——只收投影那一份。
pub(crate) async fn scope_mesh_ids(
    refno: RefnoEnum,
    include_durable: bool,
) -> anyhow::Result<BTreeSet<String>> {
    let mut ids = projected_scope_mesh_ids(refno);
    if include_durable {
        ids.extend(
            durable_scope_mesh_ids(&crate::data_interface::staging::active_data_db(), refno)
                .await?,
        );
    }
    Ok(ids)
}

/// 一条投影记录交给删除集的键：只有**烘焙实体**（[`ProjectedShape::Baked`]）的非管身记录才给。
///
/// 管身的单位圆柱与标准图元（[`ProjectedShape::Canonical`]）的键按形状全库共用，不进删除集（模块文档）。
fn purgeable_mesh_id(record: ProjectedGeometry) -> Option<String> {
    (record.tube.is_none()
        && matches!(record.shape, ProjectedShape::Baked)
        && !record.mesh_id.is_empty())
    .then_some(record.mesh_id)
}

/// 进程内投影里这一片子树下的记录引用的键（管身与标准图元的记录不计，见模块文档）。
///
/// `by_ancestor` 对记录祖先链上**每一个**祖先都建了索引，所以给 ZONE / SITE 这样的非生成根
/// 也一样取得到（与 `/model/records` 同一个作用域口径，也是 [`super::on_demand_model`]
/// 那条投影删法用的同一份记录集）。
pub(crate) fn projected_scope_mesh_ids(refno: RefnoEnum) -> BTreeSet<String> {
    let root = crate::data_interface::generation_root::refno_to_e3d(refno);
    ModelMemoryStore::global()
        .records_of_root(ProjectionScope::Current, root)
        .into_iter()
        .filter_map(purgeable_mesh_id)
        .collect()
}

/// 库上这一片子树引用的键：`inst_relate` 的 `booled_id` 与 `insts_flat.geo_hash`（烘焙实体那一行不挂
/// `inst_info`，网格键直接写在这两格里）。模型面写的、带 `out` 的行是标准图元——它的 `geo_hash` 是按形状
/// 全库共用的那一行 `inst_geo`，不收；`tubi_relate` 的管身几何（`out`）同理不收（见模块文档）。
///
/// 一句里 `anc CONTAINS` 与 `in =` 是**裸的 OR**：子树行走 `idx_*_anc`、`anc` 缺失的历史行
/// 按 `idx_*_in` 兜底，两条路各自索引（`durable_scope_reads` 记过：OR 再挂一个 `AND` 谓词
/// 优化器就整条放弃索引，所以这里不挂谓词，缺值在 Rust 这侧滤）。
pub(crate) async fn durable_scope_mesh_ids(
    db: &Surreal<Any>,
    refno: RefnoEnum,
) -> anyhow::Result<BTreeSet<String>> {
    let packed = refno.refno().0 as u64;
    let owner = refno.to_pe_key();
    collect_durable_mesh_ids(db, &durable_scope_mesh_ids_sql(packed, &owner)).await
}

/// 整库那一格（库行「清理模型（整库）」）：这个库的模型行引用过的全部网格键。
///
/// 与 [`scope_mesh_ids`] 同一份口径，只是范围换成整个库——收集必须在删之前，理由相同。
/// `include_durable` 的判据也相同：库是权威面（`Database` 路由）才收库上那一份，其余路由
/// 的库权威面是进程内投影，回头查库只会捞到过期行。
pub(crate) async fn dbnum_mesh_ids(
    dbnum: u32,
    include_durable: bool,
) -> anyhow::Result<BTreeSet<String>> {
    let mut ids = projected_dbnum_mesh_ids(dbnum);
    if include_durable {
        ids.extend(
            durable_dbnum_mesh_ids(&crate::data_interface::staging::active_data_db(), dbnum)
                .await?,
        );
    }
    Ok(ids)
}

/// [`dbnum_mesh_ids`] 的库那一半：进程内投影照旧按 `dbnum` 收（那是本进程的记忆，与哪个
/// 库无关），库上那一份走传进来的 `db`。固定两边都收——升级清库删完行之后，谁也再说不
/// 出这个库引用过哪些网格，少收一个就是留下一个删不掉的文件。
pub(crate) async fn dbnum_mesh_ids_on(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<BTreeSet<String>> {
    let mut ids = projected_dbnum_mesh_ids(dbnum);
    ids.extend(durable_dbnum_mesh_ids(db, dbnum).await?);
    Ok(ids)
}

/// 进程内投影里这个库的记录引用的键（管身与标准图元的记录不计）。整库范围不按祖先链取（那个只对子树
/// 有意义），逐条看记录的 `dbnum`——它与库行的 `dbnum` 同一来源。
pub(crate) fn projected_dbnum_mesh_ids(dbnum: u32) -> BTreeSet<String> {
    ModelMemoryStore::global()
        .records(ProjectionScope::Current)
        .into_iter()
        .filter(|record| record.dbnum == dbnum)
        .filter_map(purgeable_mesh_id)
        .collect()
}

/// 库上这个库引用的键。与子树那一份逐字段同口径（只收烘焙键，标准图元与管身不收），
/// 谓词换成 `dbnum =`，整库取全量。
pub(crate) async fn durable_dbnum_mesh_ids(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<BTreeSet<String>> {
    collect_durable_mesh_ids(db, &durable_dbnum_mesh_ids_sql(dbnum)).await
}

/// 两句收集共用的解码：`inst_relate` 行 → 键集。
async fn collect_durable_mesh_ids(
    db: &Surreal<Any>,
    sql: &str,
) -> anyhow::Result<BTreeSet<String>> {
    let mut response = db.query(sql).await?.check()?;
    let insts: Vec<DurableInstMeshRow> = response.take(0)?;
    let mut ids = BTreeSet::new();
    for row in insts {
        // `booled_id` 与 `insts_flat` 在烘焙行上说的是同一件事，但脏行可能只对得上一个
        // （`fork_surreal_compat` 记过那种「booled_id 有值、insts_flat 还是正体」的行）：
        // 两边都收，宁可多删一个已经没人引用的键，也不要漏掉还画得出来的那个。
        if let Some(booled) = row.booled_id.filter(|value| !value.is_empty()) {
            ids.insert(booled);
        }
        // 模型面写的、带 `out`（挂着 `inst_info`）的行就是标准图元：它的 `geo_hash` 是按形状全库共用的那一行
        // `inst_geo`，不进删除集（模块文档）。老生成链的行不带 `model_sesno`，照旧收——它们的几何行有没有
        // 别人指着，由删行那道闸（`delete_inst_geo_rows`）说了算。
        if row.instanced && row.model_sesno.is_some() {
            continue;
        }
        for hash in row.hashes.into_iter().flatten().flatten() {
            if !hash.is_empty() {
                ids.insert(hash);
            }
        }
    }
    Ok(ids)
}

#[derive(Debug, Deserialize)]
struct DurableInstMeshRow {
    #[serde(default)]
    booled_id: Option<String>,
    /// `insts_flat.geo_hash`：一条边一份，行没有边时是 `None`、边为空时是空数组。
    #[serde(default)]
    hashes: Option<Vec<Option<String>>>,
    /// 这一行有没有 `out`（挂着 `inst_info`）。
    #[serde(default)]
    instanced: bool,
    /// 模型面（e3d-model 生成链）写的行才有这一格；老生成链的行没有。
    #[serde(default)]
    model_sesno: Option<i64>,
}

/// 收集那一句。抽出来只为让测试不必起库就能钉住：只查 `inst_relate`（管身单位几何不收）、只带
/// `anc CONTAINS` 与 `in =` 这一个裸 OR 谓词，不 `ORDER BY`（顺序对集合没有意义，排序只是白花钱）。
/// `instanced`（行上有没有 `out`）与 `model_sesno` 只是多取的两格，用来在解码时认出标准图元，不进谓词。
fn durable_scope_mesh_ids_sql(packed: u64, owner: &str) -> String {
    format!(
        "SELECT booled_id, insts_flat.geo_hash AS hashes, (out != NONE) AS instanced, model_sesno \
         FROM inst_relate WHERE anc CONTAINS {packed} OR in = {owner};"
    )
}

/// 整库那一档的收集句：与子树那份同一形状，谓词换成 `dbnum =`。
/// 也抽出来给测试钉：整库范围不按 Ref0 区间圈、不挂别的谓词、不排序。
fn durable_dbnum_mesh_ids_sql(dbnum: u32) -> String {
    format!(
        "SELECT booled_id, insts_flat.geo_hash AS hashes, (out != NONE) AS instanced, model_sesno \
         FROM inst_relate WHERE dbnum = {dbnum};"
    )
}

/// 清掉这些键对应的几何：`durable` 时先删库里的 `inst_geo:⟨k⟩` 行，再删盘上的 `.mesh`
/// （目录按全仓唯一解析口 `options::meshes_dir()` 算）。单位几何一律跳过。
///
/// 调用方必须在**产物行删完之后**调它（见模块文档）。删文件走 `spawn_blocking`：整库那一档
/// 可能几万个文件，不占 tokio worker。
pub(crate) async fn purge_geometry(
    db: &Surreal<Any>,
    ids: BTreeSet<String>,
    durable: bool,
) -> anyhow::Result<MeshPurgeReport> {
    purge_geometry_in(db, ids, durable, crate::options::meshes_dir()).await
}

/// [`purge_geometry`] 的显式目录版（测试对一次性临时目录执行时用，与 [`remove_mesh_files_in`] 同一个理由）。
///
/// 行先、文件后；行被 [`delete_inst_geo_rows`] 留下的键（别处还有边指着）文件也留——行在、文件没了，
/// 就是别处那些实例 404。
async fn purge_geometry_in(
    db: &Surreal<Any>,
    ids: BTreeSet<String>,
    durable: bool,
    dir: std::path::PathBuf,
) -> anyhow::Result<MeshPurgeReport> {
    let (inst_geo_removed, in_use) = if durable {
        let rows = delete_inst_geo_rows(db, &ids).await?;
        (rows.removed, rows.in_use)
    } else {
        (0, BTreeSet::new())
    };
    let files: BTreeSet<String> = ids.difference(&in_use).cloned().collect();
    let mut report = tokio::task::spawn_blocking(move || remove_mesh_files_in(&dir, &files))
        .await
        .map_err(|error| anyhow::anyhow!("mesh purge task failed: {error}"))?;
    report.inst_geo_removed = inst_geo_removed;
    report.in_use_kept = in_use.len();
    Ok(report)
}

/// 一句里最多删多少行 `inst_geo`：整库那一档的键集可能上万，不把它们塞进一句。
const INST_GEO_DELETE_BATCH: usize = 500;

/// [`delete_inst_geo_rows`] 的账。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct InstGeoRowsPurge {
    /// 发出删除且没被那道闸留下的键数（按键数计：对不存在的行 `DELETE` 是空操作）。
    pub removed: usize,
    /// 库上还有 `geo_relate` / `tubi_relate` 边指着、因此留下的行的键：别处还在用，`.mesh` 也不许删。
    pub in_use: BTreeSet<String>,
}

/// 删 `inst_geo:⟨k⟩` 行：键先过字符集门、再去掉单位几何，分批按记录 id 点删。
///
/// **还有边指着的行不删**（GitHub #76）。`geo_relate` / `tubi_relate` 是 `out` 指着 `inst_geo` 的边，SurrealDB 删一个
/// 节点会把它身上的边一起删掉；调用方是在本范围的产物行删完之后才来的，这时还挂在这一行上的边只能是别的范围
/// （别的根、别的库）的——删了这一行，就是把人家的 `geo_relate` / `tubi_relate` 行一起删了，而且没有任何报错。
/// 留下的键在返回值里点名，文件那一步照着跳过。
pub(crate) async fn delete_inst_geo_rows(
    db: &Surreal<Any>,
    ids: &BTreeSet<String>,
) -> anyhow::Result<InstGeoRowsPurge> {
    let keys = ids
        .iter()
        .filter(|key| crate::fast_model::e3d_mesh_store::is_valid_mesh_key(key.as_str()))
        .filter(|key| !is_shared_unit_key(key.as_str()))
        .collect::<Vec<_>>();
    let mut purge = InstGeoRowsPurge::default();
    for chunk in keys.chunks(INST_GEO_DELETE_BATCH) {
        let mut response = db.query(render_inst_geo_delete(chunk)).await?.check()?;
        let kept: Vec<String> = response.take(INST_GEO_DELETE_SURVIVORS)?;
        purge.removed += chunk.len().saturating_sub(kept.len());
        purge.in_use.extend(kept);
    }
    Ok(purge)
}

/// [`render_inst_geo_delete`] 里回「删完还在的行」那一句的语句序号（`LET` 0、`DELETE` 1、`RETURN` 2）。
const INST_GEO_DELETE_SURVIVORS: usize = 2;

/// 一批 `inst_geo` 行的删除句：按记录 id 点名（不扫表），只删**没有边指着**的行，再回这一批里删完还在的行的键。
/// 谓词问的是记录自己身上的入边（`<-geo_relate` / `<-tubi_relate`，各是这一个节点的图键前缀），不是表上的谓词。
/// 键已过 `is_valid_mesh_key`（只有字母数字下划线），拼进 `⟨⟩` 里不会被当成别的语法。
fn render_inst_geo_delete(keys: &[&String]) -> String {
    let rows = keys
        .iter()
        .map(|key| format!("inst_geo:⟨{key}⟩"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "LET $purge_geos = [{rows}];\n\
         DELETE $purge_geos WHERE array::len(<-geo_relate) = 0 AND array::len(<-tubi_relate) = 0 RETURN NONE;\n\
         RETURN (SELECT VALUE record::id(id) FROM $purge_geos);"
    )
}

/// `/health` 的 `capabilities.mesh_purge_keeps_shared`（spec §4.1，GitHub #142）：这个二进制清几何时不带走别的范围
/// ——别的根、别的库——共用的东西。
///
/// 它不是开关，是一句声明，立在本模块的两道闸上：标准图元与管身的键不进删除集（[`purgeable_mesh_id`]，库上那一侧是
/// `collect_durable_mesh_ids` 里对带 `out` 的模型面行的判别）；还有边指着的 `inst_geo` 行不删、它的 `.mesh` 也留着
/// （[`render_inst_geo_delete`] 的谓词）。所以放在闸的旁边、恒为 `true`——撤掉哪一道闸，这一格都得跟着撤
/// （`the_keeps_shared_declaration_stands_on_both_gates`）。
///
/// 客户端拿它决定发「强制重新生成」、带 `purge_meshes` 的清理之前要不要先警告：旧后端没有这个键 = 没修，在那上面做这些事
/// 会把别的范围挂在同一行几何上的 `geo_relate` / `tubi_relate` 行一起删掉。
pub(crate) const KEEPS_SHARED: bool = true;

/// 删掉这些键对应的 `.mesh`（显式目录版：测试对一次性临时目录执行时用，与写侧的
/// `meshes_dir_for(option)` 同一个理由）。
///
/// 键先过 [`crate::fast_model::e3d_mesh_store::is_valid_mesh_key`] 那道字符集门再拼路径——
/// 那是唯一一道目录穿越的门，与 `/api/v1/meshes/{geo_hash}` 同一道。单位几何跳过（进
/// `shared_unit_skipped`）。文件不在算删过（进 `absent`）：重复清理不该报错。
pub(crate) fn remove_mesh_files_in(dir: &Path, ids: &BTreeSet<String>) -> MeshPurgeReport {
    let mut report = MeshPurgeReport::default();
    for mesh_id in ids {
        if !crate::fast_model::e3d_mesh_store::is_valid_mesh_key(mesh_id) {
            report
                .failed
                .push((mesh_id.clone(), "网格键字符集非法，未拼路径".to_string()));
            continue;
        }
        if is_shared_unit_key(mesh_id) {
            report.shared_unit_skipped += 1;
            continue;
        }
        match std::fs::remove_file(dir.join(format!("{mesh_id}.mesh"))) {
            Ok(()) => report.removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => report.absent += 1,
            Err(error) => report.failed.push((mesh_id.clone(), error.to_string())),
        }
        // 粗网格与它的「不值得」标记随 L0 走（ADR-096）：文件名借的是 L0 的键，L0 没了就是孤儿。
        // 不在盘上不算事；删失败只记日志——它是显示增强，不该让清理回执为它报红。
        for path in [
            crate::fast_model::mesh_lod1::lod1_mesh_path(dir, mesh_id),
            crate::fast_model::mesh_lod1::lod1_marker_path(dir, mesh_id),
        ] {
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    if path.extension().is_some_and(|ext| ext == "mesh") {
                        report.lod1_removed += 1;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => log::warn!("[mesh-lod1] 清理 {} 失败: {error}", path.display()),
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 收集那一句的形状：只查 `inst_relate`（管身单位几何不收）、谓词只有裸 OR（`anc CONTAINS` / `in =`）、
    /// 不排序。加了 `AND` 谓词就会把索引让掉——这是 `durable_scope_reads` 那条纪律的同一件事。
    #[test]
    fn the_collection_sql_keeps_bare_index_predicates() {
        let sql = durable_scope_mesh_ids_sql(104724187644825, "pe:24383_66457");
        assert_eq!(
            sql.matches("anc CONTAINS 104724187644825").count(),
            1,
            "{sql}"
        );
        assert_eq!(sql.matches("in = pe:24383_66457").count(), 1, "{sql}");
        assert_eq!(sql.matches("inst_relate").count(), 1, "{sql}");
        assert!(
            !sql.contains("tubi_relate") && !sql.contains("record::id(out)"),
            "管身单位几何全项目共用，不收：{sql}"
        );
        assert!(
            sql.contains("insts_flat.geo_hash AS hashes")
                && sql.contains("booled_id")
                && sql.contains("(out != NONE) AS instanced"),
            "烘焙键的两格都要取，带 `out` 的行（标准图元）靠 `instanced` 在解码时滤掉：{sql}"
        );
        assert!(!sql.contains("ORDER BY"), "顺序对集合没有意义：{sql}");
        assert!(
            !sql.contains(" AND "),
            "裸 OR 才走索引并集，挂上 AND 就退回整表：{sql}"
        );
    }

    /// 真起一台 `mem://`：成品键（`booled_id` / `insts_flat.geo_hash`）收得到，管身键
    /// （`tubi_relate.out` 的 id）不收，空值 / 缺值不混进键集，`in =` 那条兜底也生效。
    #[tokio::test]
    async fn the_durable_scan_collects_inst_keys_and_skips_tubes_and_empty_values() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem 引擎应能启动（kv-mem feature）");
        db.use_ns("mesh_purge").use_db("scope").await.unwrap();
        db.query(
            "CREATE inst_relate:24383_1 SET anc = [104724187644825], \
               booled_id = 'e3d_baked_v2_aa', insts_flat = [{ geo_hash: 'e3d_baked_v2_aa' }];\
             CREATE inst_relate:24383_2 SET anc = [104724187644825], \
               insts_flat = [{ geo_hash: '11' }, { geo_hash: '22' }];\
             CREATE inst_relate:24383_3 SET anc = [104724187644825], \
               booled_id = '', insts_flat = [];\
             CREATE inst_relate:24383_4 SET anc = [104724187644825];\
             CREATE inst_relate:24383_9 SET in = pe:24383_66457, \
               insts_flat = [{ geo_hash: '33' }];\
             CREATE inst_relate:24383_77 SET anc = [104724187644824], \
               insts_flat = [{ geo_hash: 'outside' }];\
             CREATE tubi_relate:[pe:24383_101226, 0] SET anc = [104724187644825], \
               out = inst_geo:⟨17559853280313317419⟩;\
             CREATE tubi_relate:[pe:24383_101227, 0] SET anc = [104724187644824], \
               out = inst_geo:⟨999⟩;",
        )
        .await
        .expect("种子应执行")
        .check()
        .expect("种子应成功");

        let refno = RefnoEnum::from(aios_core::RefU64(104724187644825));
        let ids = durable_scope_mesh_ids(&db, refno).await.unwrap();
        assert_eq!(
            ids,
            ["11", "22", "33", "e3d_baked_v2_aa"]
                .into_iter()
                .map(str::to_owned)
                .collect::<BTreeSet<_>>(),
            "{ids:?}"
        );
        assert!(!ids.contains("outside"), "范围外的行不得进键集：{ids:?}");
        assert!(
            !ids.contains("17559853280313317419") && !ids.contains("999"),
            "管身单位几何不进键集：{ids:?}"
        );
        assert!(!ids.contains(""), "空值不得进键集：{ids:?}");
    }

    /// 删盘上那一格：删掉算 `removed`、不在算 `absent`（重复清理不报错）、字符集非法的键
    /// 连路径都不拼（`failed` 点名），并且**只删 `.mesh`**——目录里别的东西不许碰。
    #[test]
    fn the_purge_removes_mesh_files_and_leaves_everything_else_alone() {
        let dir = tempfile::tempdir().unwrap();
        let ids: BTreeSet<String> = ["e3d_baked_v2_aa", "17", "missing", "../escape", ""]
            .into_iter()
            .map(str::to_owned)
            .collect();
        for key in ["e3d_baked_v2_aa", "17"] {
            std::fs::write(dir.path().join(format!("{key}.mesh")), b"mesh").unwrap();
        }
        std::fs::write(dir.path().join("room-input-cache"), b"keep").unwrap();

        let report = remove_mesh_files_in(dir.path(), &ids);
        assert_eq!(report.removed, 2, "{report:?}");
        assert_eq!(report.absent, 1, "{report:?}");
        assert_eq!(report.failed.len(), 2, "{report:?}");
        assert_eq!(report.shared_unit_skipped, 0, "{report:?}");
        assert!(
            report.failed.iter().any(|(key, _)| key == "../escape"),
            "{report:?}"
        );
        assert!(!dir.path().join("e3d_baked_v2_aa.mesh").exists());
        assert!(!dir.path().join("17.mesh").exists());
        assert!(
            dir.path().join("room-input-cache").exists(),
            "别的东西不许碰"
        );
        assert_eq!(
            report.note(),
            "一并删除网格文件 2 个（另 1 个本来就不在盘上），2 个删失败"
        );
    }

    /// 粗网格随 L0 走（ADR-096）：`lod1-v1/{键}.mesh` 与 `.none` 标记一并删、只计 `.mesh`；别的键的
    /// 粗网格不碰；`note()` 把它说出来。
    #[test]
    fn the_purge_takes_the_lod1_copy_and_its_marker_along_with_the_l0_mesh() {
        use crate::fast_model::mesh_lod1::{lod1_dir, lod1_marker_path, lod1_mesh_path};
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lod1_dir(dir.path())).unwrap();
        for key in ["e3d_baked_v2_aa", "e3d_baked_v2_bb", "e3d_baked_v2_keep"] {
            std::fs::write(dir.path().join(format!("{key}.mesh")), b"mesh").unwrap();
        }
        std::fs::write(lod1_mesh_path(dir.path(), "e3d_baked_v2_aa"), b"coarse").unwrap();
        std::fs::write(lod1_marker_path(dir.path(), "e3d_baked_v2_bb"), b"").unwrap();
        std::fs::write(lod1_mesh_path(dir.path(), "e3d_baked_v2_keep"), b"coarse").unwrap();
        let ids: BTreeSet<String> = ["e3d_baked_v2_aa", "e3d_baked_v2_bb"]
            .into_iter()
            .map(str::to_owned)
            .collect();

        let report = remove_mesh_files_in(dir.path(), &ids);
        assert_eq!(report.removed, 2, "{report:?}");
        assert_eq!(report.lod1_removed, 1, "只计 .mesh，标记不计: {report:?}");
        assert!(report.failed.is_empty(), "{report:?}");
        assert!(!lod1_mesh_path(dir.path(), "e3d_baked_v2_aa").exists());
        assert!(!lod1_marker_path(dir.path(), "e3d_baked_v2_bb").exists());
        assert!(
            lod1_mesh_path(dir.path(), "e3d_baked_v2_keep").exists(),
            "别的键的粗网格不碰"
        );
        assert_eq!(report.note(), "一并删除网格文件 2 个、粗网格 1 个");
    }

    /// 单位几何 `1` / `2` / `3`（`BOXI_GEO_HASH` / `TUBI_GEO_HASH` / 单位球）收到了也不删：全项目所有
    /// 管段共用那一份，而且没有生成路径会把文件写回来；行和文件都不许动。
    #[test]
    fn the_shared_unit_meshes_are_never_removed_from_disk() {
        let units = shared_unit_keys();
        assert_eq!(
            units.iter().map(String::as_str).collect::<BTreeSet<&str>>(),
            ["1", "2", "3"].into_iter().collect::<BTreeSet<&str>>(),
            "单位盒 / 单位圆柱 / 单位球就是 mesh_glb 点名的 assets/meshes/1|2|3.mesh"
        );
        let dir = tempfile::tempdir().unwrap();
        for key in ["1", "2", "3", "e3d_baked_v2_aa"] {
            std::fs::write(dir.path().join(format!("{key}.mesh")), b"mesh").unwrap();
        }
        let ids: BTreeSet<String> = ["1", "2", "3", "e3d_baked_v2_aa"]
            .into_iter()
            .map(str::to_owned)
            .collect();

        let report = remove_mesh_files_in(dir.path(), &ids);
        assert_eq!(report.removed, 1, "{report:?}");
        assert_eq!(report.shared_unit_skipped, 3, "{report:?}");
        assert!(report.failed.is_empty(), "{report:?}");
        for key in ["1", "2", "3"] {
            assert!(
                dir.path().join(format!("{key}.mesh")).exists(),
                "单位几何 {key}.mesh 不许删"
            );
        }
        assert!(!dir.path().join("e3d_baked_v2_aa.mesh").exists());
    }

    /// `inst_geo` 行的删除句按记录 id 点名、不扫表（唯一的谓词问的是记录自己身上的入边）；单位几何与字符集
    /// 非法的键连句子都不进。真起一台 `mem://` 核：范围内的行没了、范围外的和单位行都在。
    #[tokio::test]
    async fn the_inst_geo_delete_skips_unit_rows_and_leaves_other_rows_alone() {
        let aa = "e3d_baked_v2_aa".to_string();
        let bb = "e3d_baked_v2_bb".to_string();
        let sql = render_inst_geo_delete(&[&aa, &bb]);
        assert!(
            sql.contains("LET $purge_geos = [inst_geo:⟨e3d_baked_v2_aa⟩,inst_geo:⟨e3d_baked_v2_bb⟩];"),
            "按记录 id 点名：{sql}"
        );
        assert_eq!(sql.matches("DELETE ").count(), 1, "{sql}");
        assert!(
            sql.contains("DELETE $purge_geos WHERE array::len(<-geo_relate) = 0 AND array::len(<-tubi_relate) = 0"),
            "只删没有边指着的行：{sql}"
        );
        assert!(
            !sql.contains("FROM inst_geo") && !sql.contains("DELETE inst_geo "),
            "不扫表：{sql}"
        );
        assert_eq!(
            sql.lines().nth(INST_GEO_DELETE_SURVIVORS),
            Some("RETURN (SELECT VALUE record::id(id) FROM $purge_geos);"),
            "第三句回删完还在的行：{sql}"
        );

        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem 引擎应能启动（kv-mem feature）");
        db.use_ns("mesh_purge").use_db("inst_geo").await.unwrap();
        db.query(
            "CREATE inst_geo:⟨1⟩ SET meshed = true;\
             CREATE inst_geo:⟨2⟩ SET meshed = true;\
             CREATE inst_geo:⟨e3d_baked_v2_aa⟩ SET meshed = true;\
             CREATE inst_geo:⟨e3d_baked_v2_bb⟩ SET meshed = true;",
        )
        .await
        .expect("种子应执行")
        .check()
        .expect("种子应成功");

        let ids: BTreeSet<String> = ["1", "2", "e3d_baked_v2_aa", "../escape"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let purge = delete_inst_geo_rows(&db, &ids).await.unwrap();
        assert_eq!(purge.removed, 1, "只有 e3d_baked_v2_aa 该发删除");
        assert!(purge.in_use.is_empty(), "没有边指着：{purge:?}");

        let mut response = db
            .query("SELECT VALUE record::id(id) FROM inst_geo;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let remaining: Vec<String> = response.take(0).unwrap();
        assert_eq!(
            remaining.into_iter().collect::<BTreeSet<_>>(),
            ["1", "2", "e3d_baked_v2_bb"]
                .into_iter()
                .map(str::to_owned)
                .collect::<BTreeSet<_>>(),
            "单位行与范围外的行都得在"
        );
    }

    /// 收不动就整趟中止，这一格不许「尽力而为」：半成品比不删更难收拾。两个范围入口
    /// （子树 / 整库）同一条纪律。
    #[test]
    fn the_purge_is_all_or_nothing_on_the_collection_side() {
        let source = include_str!("mesh_purge.rs")
            .split_once("#[cfg(test)]")
            .expect("测试在生产代码之后")
            .0;
        let collect = source
            .split_once("pub(crate) async fn scope_mesh_ids(")
            .expect("收集入口")
            .1
            .split_once("\n}\n")
            .expect("函数结束")
            .0;
        assert!(
            collect.contains("durable_scope_mesh_ids(") && collect.contains("?"),
            "库上那份收不动要上抛：{collect}"
        );
        let dbnum = source
            .split_once("pub(crate) async fn dbnum_mesh_ids(")
            .expect("整库收集入口")
            .1
            .split_once("\n}\n")
            .expect("函数结束")
            .0;
        assert!(
            dbnum.contains("durable_dbnum_mesh_ids(") && dbnum.contains("?"),
            "整库那一档同样收不动要上抛：{dbnum}"
        );
        // 删几何那一步是行先、文件后：行删不动就不碰盘。入口只是把目录解析出来交给显式目录版。
        let entry = source
            .split_once("pub(crate) async fn purge_geometry(")
            .expect("删几何入口")
            .1
            .split_once("\n}\n")
            .expect("函数结束")
            .0;
        assert!(
            entry.contains("purge_geometry_in(db, ids, durable, crate::options::meshes_dir())"),
            "入口交给显式目录版：{entry}"
        );
        let purge = source
            .split_once("async fn purge_geometry_in(")
            .expect("删几何（显式目录版）")
            .1
            .split_once("\n}\n")
            .expect("函数结束")
            .0;
        let rows = purge.find("delete_inst_geo_rows(").expect("先删行");
        let files = purge.find("remove_mesh_files_in(").expect("再删文件");
        assert!(rows < files, "几何行在前、文件在后：{purge}");
        assert!(
            purge.contains("spawn_blocking"),
            "删文件不占 tokio worker：{purge}"
        );
        assert!(
            purge.contains("ids.difference(&in_use)"),
            "行被留下的键文件也留：{purge}"
        );
    }

    use e3d_io::refno::RefNo;
    use e3d_model::elmodl::GeometryId;
    use e3d_model::primitive_instance::PrimitiveMeshKey;
    use glam::DMat4;

    use crate::fast_model::model_db_adapter::{WriteMode, render_records};
    use crate::fast_model::model_memory_store::{ProjectedTube, packed_refno};

    /// 标准图元 BOX 的键：全库的 BOX 共用 `BoxV1` 这一行（真库 `inst_geo:⟨12296242654610097853⟩`，
    /// 2026-11-11 时 7999 有 261 只、8000 有 46 只 BOX 挂在它上面）。
    const BOX_KEY: &str = "12296242654610097853";
    /// 圆柱按段数共用的键：设计 CYLI 与隐式管身用的是同一套（真库 7999 这个键上挂着 1264 根管身 + 112 只 CYLI）。
    const CYLINDER_KEY: &str = "11649145690927941167";
    const BAKED_KEY: &str = "e3d_baked_v2_aa";

    fn canonical(key: PrimitiveMeshKey) -> ProjectedShape {
        ProjectedShape::Canonical {
            key,
            local_transform: DMat4::IDENTITY,
            local_bounds: [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]],
        }
    }

    fn element(
        root: RefNo,
        refno: RefNo,
        noun: &str,
        mesh_id: &str,
        shape: ProjectedShape,
    ) -> ProjectedGeometry {
        ProjectedGeometry {
            record_id: format!("{}_{}", refno.word0, refno.word1),
            geometry_id: GeometryId::Element {
                refno: format!("{}/{}", refno.word0, refno.word1),
            },
            source_refno: refno,
            dbnum: 7998,
            source_sesno: 542,
            noun: noun.into(),
            mesh_id: mesh_id.into(),
            world_transform: DMat4::IDENTITY,
            world_bounds: [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]],
            ancestors: vec![packed_refno(refno), packed_refno(root)],
            shape,
            tube: None,
        }
    }

    /// 一根隐式管身：挂在分支上，网格是按段数共用的那只单位圆柱。
    fn tube(branch: RefNo, record_id: &str) -> ProjectedGeometry {
        ProjectedGeometry {
            record_id: record_id.into(),
            geometry_id: GeometryId::ImpliedTube {
                container_refno: format!("{}/{}", branch.word0, branch.word1),
                route_ordinal: 0,
                from_refno: "24381/145767".into(),
                to_refno: "24381/145770".into(),
            },
            source_refno: branch,
            dbnum: 7997,
            source_sesno: 385,
            noun: "TUBI".into(),
            mesh_id: CYLINDER_KEY.into(),
            world_transform: DMat4::IDENTITY,
            world_bounds: [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]],
            ancestors: vec![packed_refno(branch)],
            shape: canonical(PrimitiveMeshKey::CylinderV3 { segments: 32 }),
            tube: Some(ProjectedTube {
                route_ordinal: 0,
                from: RefNo::new(24381, 145767),
                to: RefNo::new(24381, 145770),
                invalid: false,
            }),
        }
    }

    const ROOT_A: RefNo = RefNo::new(24382, 1078);
    const ROOT_B: RefNo = RefNo::new(24382, 1074);
    const BRANCH: RefNo = RefNo::new(24381, 145764);

    /// 三片范围共用标准图元的库（行由生产渲染器 `render_records` 写）：
    /// 设备 A（`24382/1078`）名下一只 BOX、一只 CYLI、一件烘焙实体；设备 B（`24382/1074`）名下一只 BOX；
    /// 分支 `24381/145764` 名下一根管身。两只 BOX 共用 `BoxV1` 那一行，CYLI 与管身共用同一只单位圆柱。
    async fn shared_primitive_database(label: &str) -> Surreal<Any> {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem 引擎应能启动（kv-mem feature）");
        db.use_ns("mesh_purge").use_db(label).await.unwrap();
        let records = vec![
            element(
                ROOT_A,
                RefNo::new(24382, 1079),
                "BOX",
                BOX_KEY,
                canonical(PrimitiveMeshKey::BoxV1),
            ),
            element(
                ROOT_A,
                RefNo::new(24382, 1080),
                "CYLI",
                CYLINDER_KEY,
                canonical(PrimitiveMeshKey::CylinderV3 { segments: 32 }),
            ),
            element(
                ROOT_A,
                RefNo::new(24382, 1081),
                "BOX",
                BAKED_KEY,
                ProjectedShape::Baked,
            ),
            element(
                ROOT_B,
                RefNo::new(24382, 1075),
                "BOX",
                BOX_KEY,
                canonical(PrimitiveMeshKey::BoxV1),
            ),
            tube(BRANCH, "a1b2c3d4"),
        ];
        db.query(render_records(&records, WriteMode::Initial).expect("render"))
            .await
            .expect("种子应执行")
            .check()
            .expect("种子应成功");
        db
    }

    /// 范围 A 的产物行删完之后的样子（生产上是 `delete_model_subtree` 做的；清几何排在它后面）。
    async fn delete_scope_a_rows(db: &Surreal<Any>) {
        db.query(
            "DELETE (SELECT VALUE id FROM geo_relate \
               WHERE in IN [inst_info:⟨24382_1079⟩, inst_info:⟨24382_1080⟩]);\
             DELETE inst_info:⟨24382_1079⟩, inst_info:⟨24382_1080⟩;\
             DELETE inst_relate:⟨24382_1079⟩, inst_relate:⟨24382_1080⟩, inst_relate:⟨24382_1081⟩;",
        )
        .await
        .expect("删范围 A 的产物行")
        .check()
        .expect("删范围 A 的产物行应成功");
    }

    /// 别的范围还剩什么：B 那只 BOX 的 `geo_relate`、分支那根管身的 `tubi_relate`、两行共用的 `inst_geo`。
    async fn other_scopes_rows(db: &Surreal<Any>) -> (usize, usize, BTreeSet<String>) {
        let mut response = db
            .query(
                "SELECT VALUE record::id(geom_refno) FROM geo_relate WHERE in = inst_info:⟨24382_1075⟩;\
                 SELECT VALUE record::id(out) FROM tubi_relate WHERE in = pe:⟨24381_145764⟩;\
                 SELECT VALUE record::id(id) FROM inst_geo;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        let geo: Vec<String> = response.take(0).unwrap();
        let tubes: Vec<String> = response.take(1).unwrap();
        let meshes: Vec<String> = response.take(2).unwrap();
        (geo.len(), tubes.len(), meshes.into_iter().collect())
    }

    /// 这两道闸防的是什么（引擎行为，钉在这里当根因的记录）：SurrealDB 删一个节点，会把它身上的边一起删掉。
    /// 2026-10-22 到 2026-11-11 之间清几何发的就是这种裸 `DELETE inst_geo:⟨k⟩;`，键里带着共用的标准图元——
    /// 别的范围的 `geo_relate` / `tubi_relate` 行跟着没了，没有任何报错。哪天引擎不再连带删边，这一条会红，
    /// 那时再回头看闸还要不要。
    #[tokio::test]
    async fn deleting_an_inst_geo_row_takes_every_edge_that_points_at_it() {
        let db = shared_primitive_database("edge_cascade").await;
        db.query(format!(
            "DELETE inst_geo:⟨{BOX_KEY}⟩;\nDELETE inst_geo:⟨{CYLINDER_KEY}⟩;"
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
        assert_eq!(
            other_scopes_rows(&db).await,
            (0, 0, BTreeSet::new()),
            "裸删共用几何行：设备 B 的 geo_relate 与分支的 tubi_relate 一起被带走"
        );
    }

    /// GitHub #76：清一片范围的几何，不许把别的范围挂在共用标准图元上的行带走。
    ///
    /// `geo_relate` / `tubi_relate` 是 `out` 指着 `inst_geo` 的边，SurrealDB 删节点会把它身上的边一起删掉。
    /// 2026-10-22 起「重新生成模型」的先删把范围内 `insts_flat.geo_hash` 上的键全收进删除集，于是重生成设备 A
    /// 之后，设备 B 那只 BOX 的 `Pos` 行没了（增量夹具「恢复后多一条 `Pos`」的基线就是这么来的），同键的管身行也没了。
    /// 这里照生产的先后走一遍：收键 → 删范围 A 的产物行 → 清几何行。
    #[tokio::test]
    async fn purging_one_scope_leaves_the_rows_other_scopes_hang_on_shared_primitive_meshes() {
        let db = shared_primitive_database("scope_purge").await;
        assert_eq!(
            other_scopes_rows(&db).await,
            (
                1,
                1,
                [BOX_KEY, CYLINDER_KEY]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            ),
            "种子：B 的 BOX 一条 geo_relate、分支一根管身、两行共用几何（烘焙实体不建 inst_geo 行）"
        );

        let keys = durable_scope_mesh_ids(&db, crate::data_interface::generation_root::refno_from_e3d(ROOT_A))
            .await
            .unwrap();
        assert_eq!(
            keys,
            BTreeSet::from([BAKED_KEY.to_string()]),
            "删除集只收烘焙键，标准图元的共用键不进：{keys:?}"
        );
        delete_scope_a_rows(&db).await;
        let purge = delete_inst_geo_rows(&db, &keys).await.unwrap();
        assert_eq!(purge.removed, 1, "{purge:?}");
        assert!(purge.in_use.is_empty(), "{purge:?}");

        assert_eq!(
            other_scopes_rows(&db).await,
            (
                1,
                1,
                [BOX_KEY, CYLINDER_KEY]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            ),
            "设备 B 的 geo_relate、分支的 tubi_relate 与它们共用的 inst_geo 行都得还在"
        );
    }

    /// 第二道闸：就算共用键进了删除集（老行、脏行、将来别的收集口），`inst_geo` 行只要还有边指着就不删，
    /// 它的 `.mesh` 也留着；没有边指着的行与文件照删。回执里点名留下了几个。
    #[tokio::test]
    async fn an_inst_geo_row_with_edges_still_pointing_at_it_is_kept_with_its_mesh_file() {
        let db = shared_primitive_database("in_use_guard").await;
        // 一行没人指着的几何（比如管身消失后留下的烘焙管身网格）：该删。
        db.query("CREATE inst_geo:⟨e3d_baked_v2_orphan⟩ SET meshed = true;")
            .await
            .unwrap()
            .check()
            .unwrap();
        delete_scope_a_rows(&db).await;

        let dir = tempfile::tempdir().unwrap();
        let ids: BTreeSet<String> = [BOX_KEY, CYLINDER_KEY, BAKED_KEY, "e3d_baked_v2_orphan"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        for key in &ids {
            std::fs::write(dir.path().join(format!("{key}.mesh")), b"mesh").unwrap();
        }

        let report = purge_geometry_in(&db, ids, true, dir.path().to_path_buf())
            .await
            .unwrap();
        assert_eq!(report.in_use_kept, 2, "BOX 与圆柱那两行别处还在用：{report:?}");
        assert_eq!(report.inst_geo_removed, 2, "烘焙键与孤儿行照删：{report:?}");
        assert_eq!(report.removed, 2, "{report:?}");
        assert!(report.failed.is_empty(), "{report:?}");
        assert_eq!(
            report.note(),
            "一并删除网格文件 2 个、几何行 2 行、别处还在用留下 2 个"
        );
        for kept in [BOX_KEY, CYLINDER_KEY] {
            assert!(
                dir.path().join(format!("{kept}.mesh")).exists(),
                "{kept}.mesh 别处还在用，不许删"
            );
        }
        for gone in [BAKED_KEY, "e3d_baked_v2_orphan"] {
            assert!(!dir.path().join(format!("{gone}.mesh")).exists(), "{gone}");
        }
        assert_eq!(
            other_scopes_rows(&db).await,
            (
                1,
                1,
                [BOX_KEY, CYLINDER_KEY]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            ),
            "别的范围的 geo_relate / tubi_relate 行一条都不许少，孤儿几何行没了"
        );
    }

    /// 整库那一档同一份口径：标准图元的共用键不进删除集（别的库的 BOX、管身挂在同一行上）。
    #[tokio::test]
    async fn the_dbnum_scan_leaves_shared_primitive_keys_out_of_the_delete_set() {
        let db = shared_primitive_database("dbnum_purge").await;
        let ids = durable_dbnum_mesh_ids(&db, 7998).await.unwrap();
        assert_eq!(
            ids,
            BTreeSet::from([BAKED_KEY.to_string()]),
            "7998 的两只 BOX、一只 CYLI 都是标准图元，只有烘焙那一件给键：{ids:?}"
        );
    }

    /// 收集认标准图元靠两格：带 `out`（挂着 `inst_info`）**且**带 `model_sesno`（模型面写的）。老生成链的行
    /// 带 `out` 却没有 `model_sesno`，照旧给键——它的几何行还有没有别人指着，交给删行那道闸。
    #[tokio::test]
    async fn only_model_face_instanced_rows_keep_their_key_out_of_the_delete_set() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem 引擎应能启动（kv-mem feature）");
        db.use_ns("mesh_purge").use_db("legacy").await.unwrap();
        db.query(format!(
            "CREATE inst_relate:24383_1 SET anc = [104724187644825], out = inst_info:legacy_1, \
               insts_flat = [{{ geo_hash: '4711' }}];\
             CREATE inst_relate:24383_2 SET anc = [104724187644825], out = inst_info:⟨24383_2⟩, \
               model_sesno = 42, booled = false, insts_flat = [{{ geo_hash: '{BOX_KEY}' }}];\
             CREATE inst_relate:24383_3 SET anc = [104724187644825], model_sesno = 42, booled = true, \
               booled_id = '{BAKED_KEY}', insts_flat = [{{ geo_hash: '{BAKED_KEY}' }}];"
        ))
        .await
        .expect("种子应执行")
        .check()
        .expect("种子应成功");
        let refno = RefnoEnum::from(aios_core::RefU64(104724187644825));
        let ids = durable_scope_mesh_ids(&db, refno).await.unwrap();
        assert_eq!(
            ids,
            ["4711", BAKED_KEY]
                .into_iter()
                .map(str::to_owned)
                .collect::<BTreeSet<_>>(),
            "老行与烘焙行给键，模型面的标准图元行不给：{ids:?}"
        );
    }

    /// 投影那一半同一份口径：只有烘焙实体的非管身记录给键。
    #[test]
    fn only_baked_non_tube_records_hand_their_mesh_to_the_delete_set() {
        let baked = element(
            ROOT_A,
            RefNo::new(24382, 1081),
            "BOX",
            BAKED_KEY,
            ProjectedShape::Baked,
        );
        assert_eq!(purgeable_mesh_id(baked.clone()), Some(BAKED_KEY.to_string()));
        let shared_box = element(
            ROOT_A,
            RefNo::new(24382, 1079),
            "BOX",
            BOX_KEY,
            canonical(PrimitiveMeshKey::BoxV1),
        );
        assert_eq!(purgeable_mesh_id(shared_box), None, "标准图元的键全库共用");
        assert_eq!(purgeable_mesh_id(tube(BRANCH, "a1b2c3d4")), None, "管身单位圆柱");
        let mut baked_tube = tube(BRANCH, "e5f60718");
        baked_tube.shape = ProjectedShape::Baked;
        baked_tube.mesh_id = "e3d_baked_v2_tube".into();
        assert_eq!(purgeable_mesh_id(baked_tube), None, "管身记录一律不给");
        let mut unnamed = baked;
        unnamed.mesh_id.clear();
        assert_eq!(purgeable_mesh_id(unnamed), None, "空键不进键集");
    }

    /// `capabilities.mesh_purge_keeps_shared` 那句声明（[`KEEPS_SHARED`]，GitHub #142）与两道闸同在：
    /// ① 标准图元、管身的投影记录不交键，库上收集的解码把带 `out` 的模型面行滤掉；② 删行那一句只删没有边指着的行、
    /// 行被留下的键文件也留。两道闸各自的行为由上面几条 `mem://` 用例钉，这里钉的是「声明不许比闸活得久」——
    /// 撤了哪一道闸，这一条就红，声明得跟着撤。
    #[test]
    fn the_keeps_shared_declaration_stands_on_both_gates() {
        assert!(KEEPS_SHARED, "带着这两道闸的二进制恒声明 true");

        // ① 收集口：共用的键不进删除集。
        let shared_box = element(
            ROOT_B,
            RefNo::new(24382, 1075),
            "BOX",
            BOX_KEY,
            canonical(PrimitiveMeshKey::BoxV1),
        );
        assert_eq!(purgeable_mesh_id(shared_box), None, "标准图元的键全库共用");
        assert_eq!(purgeable_mesh_id(tube(BRANCH, "a1b2c3d4")), None, "管身单位圆柱");
        let source = include_str!("mesh_purge.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("生产代码在测试模块之前");
        assert!(
            source.contains("if row.instanced && row.model_sesno.is_some() {"),
            "库上那一侧：模型面写的、带 `out` 的行（标准图元）不交 `geo_hash`"
        );

        // ② 删行：只删没有边指着的行；留下的键文件也留。
        let key = BAKED_KEY.to_string();
        let sql = render_inst_geo_delete(&[&key]);
        assert!(
            sql.contains(
                "DELETE $purge_geos WHERE array::len(<-geo_relate) = 0 AND array::len(<-tubi_relate) = 0"
            ),
            "{sql}"
        );
        assert!(
            source.contains("ids.difference(&in_use)"),
            "行被留下的键 `.mesh` 也留"
        );
    }

    /// 整库那一档的收集句：与子树同一形状，谓词是 `dbnum =`（不是 Ref0 区间、不挂别的
    /// 谓词、不排序）。范围语义换了，收集口径（只收烘焙键，标准图元与管身不收）不许换。
    #[test]
    fn the_dbnum_collection_sql_asks_by_dbnum_only() {
        let sql = durable_dbnum_mesh_ids_sql(7999);
        assert_eq!(sql.matches("WHERE dbnum = 7999").count(), 1, "{sql}");
        assert_eq!(sql.matches("inst_relate").count(), 1, "{sql}");
        assert!(
            !sql.contains("tubi_relate") && !sql.contains("record::id(out)"),
            "管身单位几何全项目共用，不收：{sql}"
        );
        assert!(
            sql.contains("insts_flat.geo_hash AS hashes")
                && sql.contains("booled_id")
                && sql.contains("(out != NONE) AS instanced"),
            "烘焙键的两格都要取，带 `out` 的行（标准图元）靠 `instanced` 在解码时滤掉：{sql}"
        );
        assert!(!sql.contains("ORDER BY"), "顺序对集合没有意义：{sql}");
        assert!(
            !sql.contains(" AND ") && !sql.contains("anc CONTAINS"),
            "整库范围按 dbnum 取全量，不按 Ref0 区间、不挂别的谓词：{sql}"
        );
    }

    /// 整库收集真跑一台 `mem://`：只收这个库的行（别的库一行都不许混进来），
    /// 没有 `out` 的行上的键收得到（`booled_id` / `insts_flat.geo_hash`），管身键不收，空值不混进键集。
    #[tokio::test]
    async fn the_dbnum_scan_keeps_only_this_database() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem 引擎应能启动（kv-mem feature）");
        db.use_ns("mesh_purge").use_db("dbnum").await.unwrap();
        db.query(
            "CREATE inst_relate:24383_1 SET dbnum = 7999, booled_id = 'e3d_baked_v2_aa';\
             CREATE inst_relate:24383_2 SET dbnum = 7999, \
               insts_flat = [{ geo_hash: '11' }, { geo_hash: '22' }];\
             CREATE inst_relate:24383_3 SET dbnum = 7999, booled_id = '', insts_flat = [];\
             CREATE inst_relate:24383_4 SET dbnum = 8000, booled_id = 'e3d_baked_v2_other';\
             CREATE inst_relate:24383_5 SET dbnum = 7999;\
             CREATE tubi_relate:[pe:24383_101226, 0] SET dbnum = 7999, \
               out = inst_geo:⟨17559853280313317419⟩;\
             CREATE tubi_relate:[pe:24383_101227, 0] SET dbnum = 8000, \
               out = inst_geo:⟨999⟩;",
        )
        .await
        .expect("种子应执行")
        .check()
        .expect("种子应成功");

        let ids = durable_dbnum_mesh_ids(&db, 7999).await.unwrap();
        assert_eq!(
            ids,
            ["11", "22", "e3d_baked_v2_aa"]
                .into_iter()
                .map(str::to_owned)
                .collect::<BTreeSet<_>>(),
            "{ids:?}"
        );
        assert!(
            !ids.contains("e3d_baked_v2_other") && !ids.contains("999"),
            "别的库的行不得进键集：{ids:?}"
        );
        assert!(
            !ids.contains("17559853280313317419"),
            "管身单位几何不进键集：{ids:?}"
        );
        assert!(!ids.contains(""), "空值不得进键集：{ids:?}");
    }
}
