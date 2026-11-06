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
//!
//! 唯一的例外是**单位几何**（[`shared_unit_keys`]）：全项目所有隐含直管段共用 `inst_geo:⟨2⟩`
//! （`TUBI_GEO_HASH`）/ `⟨1⟩`（`BOXI_GEO_HASH`），单位球是 `3`；`1/2/3.mesh` 没有任何生成路径会写回
//! （`ensure_mesh_file` 只写 baked 键，`fill_basic_shapes` 只写回行），删了就是全库所有管段 404。
//! 这三枚行与文件一律不删，收到了也跳过。
//!
//! 同一个理由，**管身的单位几何不进删除集**：`tubi_relate.out` 指的是按段数共用的单位圆柱
//! （`unit_cylinder_identity(段数)`，实测一个键被几百到几千根管段引用，跨库），重生成写回的还是同一个键、
//! 同一份文件，删它只会让别的库的管段白白 404 一阵。收集只看 `inst_relate`（成品键 + 标准图元键）和
//! 投影里非管身的记录。
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

use crate::fast_model::model_memory_store::{ModelMemoryStore, ProjectionScope};

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
            durable_scope_mesh_ids(&crate::data_interface::staging::active_data_db(), refno).await?,
        );
    }
    Ok(ids)
}

/// 进程内投影里这一片子树下的记录引用的键（管身记录不计，见模块文档）。
///
/// `by_ancestor` 对记录祖先链上**每一个**祖先都建了索引，所以给 ZONE / SITE 这样的非生成根
/// 也一样取得到（与 `/model/records` 同一个作用域口径，也是 [`super::on_demand_model`]
/// 那条投影删法用的同一份记录集）。
pub(crate) fn projected_scope_mesh_ids(refno: RefnoEnum) -> BTreeSet<String> {
    let root = crate::data_interface::generation_root::refno_to_e3d(refno);
    ModelMemoryStore::global()
        .records_of_root(ProjectionScope::Current, root)
        .into_iter()
        .filter(|record| record.tube.is_none())
        .map(|record| record.mesh_id)
        .filter(|mesh_id| !mesh_id.is_empty())
        .collect()
}

/// 库上这一片子树引用的键：`inst_relate` 的成品键（`booled_id` 与 `insts_flat` 的
/// `geo_hash`——前者是布尔后的整件网格，后者是标准图元那份）。`tubi_relate` 的管身几何
/// （`out`）是全项目按段数共用的单位圆柱，不收（见模块文档）。
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
            durable_dbnum_mesh_ids(&crate::data_interface::staging::active_data_db(), dbnum).await?,
        );
    }
    Ok(ids)
}

/// 进程内投影里这个库的记录引用的键（管身记录不计）。整库范围不按祖先链取（那个只对子树
/// 有意义），逐条看记录的 `dbnum`——它与库行的 `dbnum` 同一来源。
pub(crate) fn projected_dbnum_mesh_ids(dbnum: u32) -> BTreeSet<String> {
    ModelMemoryStore::global()
        .records(ProjectionScope::Current)
        .into_iter()
        .filter(|record| record.dbnum == dbnum && record.tube.is_none())
        .map(|record| record.mesh_id)
        .filter(|mesh_id| !mesh_id.is_empty())
        .collect()
}

/// 库上这个库引用的键。与子树那一份逐字段同口径（成品键 + 标准图元键，管身不收），
/// 谓词换成 `dbnum =`，整库取全量。
pub(crate) async fn durable_dbnum_mesh_ids(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<BTreeSet<String>> {
    collect_durable_mesh_ids(db, &durable_dbnum_mesh_ids_sql(dbnum)).await
}

/// 两句收集共用的解码：`inst_relate` 行 → 键集。
async fn collect_durable_mesh_ids(db: &Surreal<Any>, sql: &str) -> anyhow::Result<BTreeSet<String>> {
    let mut response = db.query(sql).await?.check()?;
    let insts: Vec<DurableInstMeshRow> = response.take(0)?;
    let mut ids = BTreeSet::new();
    for row in insts {
        // `booled_id` 与 `insts_flat` 在成品行上说的是同一件事，但脏行可能只对得上一个
        // （`fork_surreal_compat` 记过那种「booled_id 有值、insts_flat 还是正体」的行）：
        // 两边都收，宁可多删一个已经没人引用的键，也不要漏掉还画得出来的那个。
        if let Some(booled) = row.booled_id.filter(|value| !value.is_empty()) {
            ids.insert(booled);
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
}

/// 收集那一句。抽出来只为让测试不必起库就能钉住：只查 `inst_relate`（管身单位几何不收）、只带
/// `anc CONTAINS` 与 `in =` 这一个裸 OR 谓词，不 `ORDER BY`（顺序对集合没有意义，排序只是白花钱）。
fn durable_scope_mesh_ids_sql(packed: u64, owner: &str) -> String {
    format!(
        "SELECT booled_id, insts_flat.geo_hash AS hashes FROM inst_relate \
         WHERE anc CONTAINS {packed} OR in = {owner};"
    )
}

/// 整库那一档的收集句：与子树那份同一形状，谓词换成 `dbnum =`。
/// 也抽出来给测试钉：整库范围不按 Ref0 区间圈、不挂别的谓词、不排序。
fn durable_dbnum_mesh_ids_sql(dbnum: u32) -> String {
    format!(
        "SELECT booled_id, insts_flat.geo_hash AS hashes FROM inst_relate WHERE dbnum = {dbnum};"
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
    let inst_geo_removed = if durable {
        delete_inst_geo_rows(db, &ids).await?
    } else {
        0
    };
    let dir = crate::options::meshes_dir();
    let mut report = tokio::task::spawn_blocking(move || remove_mesh_files_in(&dir, &ids))
        .await
        .map_err(|error| anyhow::anyhow!("mesh purge task failed: {error}"))?;
    report.inst_geo_removed = inst_geo_removed;
    Ok(report)
}

/// 一句里最多删多少行 `inst_geo`：整库那一档的键集可能上万，不把它们塞进一句。
const INST_GEO_DELETE_BATCH: usize = 500;

/// 删 `inst_geo:⟨k⟩` 行：键先过字符集门、再去掉单位几何，分批成一句多条 `DELETE`。
/// 返回发出删除的键数。
pub(crate) async fn delete_inst_geo_rows(
    db: &Surreal<Any>,
    ids: &BTreeSet<String>,
) -> anyhow::Result<usize> {
    let keys = ids
        .iter()
        .filter(|key| crate::fast_model::e3d_mesh_store::is_valid_mesh_key(key.as_str()))
        .filter(|key| !is_shared_unit_key(key.as_str()))
        .collect::<Vec<_>>();
    for chunk in keys.chunks(INST_GEO_DELETE_BATCH) {
        db.query(render_inst_geo_delete(chunk)).await?.check()?;
    }
    Ok(keys.len())
}

/// 一批 `inst_geo` 行的删除句：逐键 `DELETE inst_geo:⟨k⟩;`（按记录 id 直删，不扫表、不挂谓词）。
/// 键已过 `is_valid_mesh_key`（只有字母数字下划线），拼进 `⟨⟩` 里不会被当成别的语法。
fn render_inst_geo_delete(keys: &[&String]) -> String {
    keys.iter()
        .map(|key| format!("DELETE inst_geo:⟨{key}⟩;"))
        .collect::<Vec<_>>()
        .join("\n")
}

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
        assert_eq!(sql.matches("anc CONTAINS 104724187644825").count(), 1, "{sql}");
        assert_eq!(sql.matches("in = pe:24383_66457").count(), 1, "{sql}");
        assert_eq!(sql.matches("inst_relate").count(), 1, "{sql}");
        assert!(
            !sql.contains("tubi_relate") && !sql.contains("record::id(out)"),
            "管身单位几何全项目共用，不收：{sql}"
        );
        assert!(
            sql.contains("insts_flat.geo_hash AS hashes") && sql.contains("booled_id"),
            "成品键与标准图元键都要收：{sql}"
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
        assert!(dir.path().join("room-input-cache").exists(), "别的东西不许碰");
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
        assert!(lod1_mesh_path(dir.path(), "e3d_baked_v2_keep").exists(), "别的键的粗网格不碰");
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

    /// `inst_geo` 行的删除句按记录 id 直删、不扫表；单位几何与字符集非法的键连句子都不进。
    /// 真起一台 `mem://` 核：范围内的行没了、范围外的和单位行都在。
    #[tokio::test]
    async fn the_inst_geo_delete_skips_unit_rows_and_leaves_other_rows_alone() {
        let aa = "e3d_baked_v2_aa".to_string();
        let bb = "e3d_baked_v2_bb".to_string();
        let sql = render_inst_geo_delete(&[&aa, &bb]);
        assert_eq!(sql.matches("DELETE inst_geo:⟨").count(), 2, "{sql}");
        assert!(sql.contains("DELETE inst_geo:⟨e3d_baked_v2_aa⟩;"), "{sql}");
        assert!(!sql.contains("WHERE"), "按 id 直删、不挂谓词：{sql}");

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
        let issued = delete_inst_geo_rows(&db, &ids).await.unwrap();
        assert_eq!(issued, 1, "只有 e3d_baked_v2_aa 该发删除");

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
        // 删几何那一步是行先、文件后：行删不动就不碰盘。
        let purge = source
            .split_once("pub(crate) async fn purge_geometry(")
            .expect("删几何入口")
            .1
            .split_once("\n}\n")
            .expect("函数结束")
            .0;
        let rows = purge.find("delete_inst_geo_rows(").expect("先删行");
        let files = purge.find("remove_mesh_files_in(").expect("再删文件");
        assert!(rows < files, "几何行在前、文件在后：{purge}");
        assert!(purge.contains("spawn_blocking"), "删文件不占 tokio worker：{purge}");
    }

    /// 整库那一档的收集句：与子树同一形状，谓词是 `dbnum =`（不是 Ref0 区间、不挂别的
    /// 谓词、不排序）。范围语义换了，收集口径（成品键 / 标准图元键，管身不收）不许换。
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
            sql.contains("insts_flat.geo_hash AS hashes") && sql.contains("booled_id"),
            "成品键与标准图元键都要收：{sql}"
        );
        assert!(!sql.contains("ORDER BY"), "顺序对集合没有意义：{sql}");
        assert!(
            !sql.contains(" AND ") && !sql.contains("anc CONTAINS"),
            "整库范围按 dbnum 取全量，不按 Ref0 区间、不挂别的谓词：{sql}"
        );
    }

    /// 整库收集真跑一台 `mem://`：只收这个库的行（别的库一行都不许混进来），
    /// 成品键 / 标准图元键 / 管身键三种都收得到，空值不混进键集。
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
