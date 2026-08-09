//! 房间计算的最小合成夹具（ADR-010 §9）。
//!
//! AvevaMarineSample 的 7997 模型可用于真实全量基线和无损单构件增量对拍；需要移动几何
//! 的破坏性场景仍放在一次性数据库的合成夹具中，避免改动共享项目数据。
//!
//! 夹具铺的是 `cal_room_refnos` 真正会走的整条链路，而不是简化替身：
//!
//! - 层级按本库实际形状 `FRMW → CWALL → PANE`，FRMW 同时写 `pe` 与类型表两处
//!   （`build_room_panels_relate_common` 查的是 `from FRMW`）；
//! - 每个 PANE 有真实的 `.mesh` 文件，因为判定要把它反序列化成 `TriMesh` 做点包含；
//! - 每个构件有 `inst_relate.aabb`（进 R 树）与 `inst_geo.pts`（第二轮点检查要用）。
//!
//! 布局：两个 1000 见方的房间在 x 上重叠 100，5 个构件覆盖三种归属情形。
//!
//! ```text
//!   x=0        900 1000            1900
//!   ├── A ──────┼───┤
//!               ├───┼──── B ─────────┤
//!    C1   C2      C5      C3    C4
//! ```
//!
//! `C5` 骑在重叠区上：它的 AABB 八个顶点对 A、B 都只有部分在内，因此两边都会落到
//! 第二轮的逐点兜底，是多归属与排序的用例。

use std::path::{Path, PathBuf};

use aios_core::SUL_DB;
use aios_core::shape::pdms_shape::PlantMesh;
use glam::Vec3;
use parry3d::bounding_volume::Aabb;

/// 夹具占用的库号。沿用仓库既有的保留段约定（见 `model_refresh.rs` 的 live 用例）。
pub const FIXTURE_DBNUM: u64 = 4000000001;

const ROOM_NAME: &str = "/ZZ-R-K100";
const ROOM_NUM: &str = "K100";

fn refno(seq: u64) -> String {
    format!("{FIXTURE_DBNUM}_{seq}")
}

/// 一个闭合且朝外的盒子。`TriMeshFlags::ORIENTED` 下 `contains_point` 依赖法线朝向，
/// 绕序错了会把内外判反。
/// 供 `room_predicate` 的单测复用同一个盒子构造，避免两处各造一份形状。
pub fn box_mesh_for_test(min: Vec3, max: Vec3) -> PlantMesh {
    box_mesh(min, max)
}

fn box_mesh(min: Vec3, max: Vec3) -> PlantMesh {
    let vertices = vec![
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ];
    #[rustfmt::skip]
    let indices = vec![
        0, 2, 1,  0, 3, 2, // -z
        4, 5, 6,  4, 6, 7, // +z
        0, 1, 5,  0, 5, 4, // -y
        3, 7, 6,  3, 6, 2, // +y
        0, 4, 7,  0, 7, 3, // -x
        1, 2, 6,  1, 6, 5, // +x
    ];
    let mut mesh = PlantMesh {
        indices,
        vertices,
        normals: Vec::new(),
        wire_vertices: Vec::new(),
        aabb: None,
    };
    mesh.aabb = mesh.cal_aabb();
    mesh
}

fn aabb_json(aabb: &Aabb) -> String {
    format!(
        "{{mins: [{}, {}, {}], maxs: [{}, {}, {}]}}",
        aabb.mins.x, aabb.mins.y, aabb.mins.z, aabb.maxs.x, aabb.maxs.y, aabb.maxs.z
    )
}

const IDENTITY_TRANS: &str =
    "{translation: [0.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0]}";

/// 夹具里的一个几何体：一个 pe + 一条 inst_relate + 一个 inst_geo，盒形。
struct Body {
    seq: u64,
    noun: &'static str,
    owner_seq: u64,
    min: Vec3,
    max: Vec3,
    /// 只有 PANE 需要落 `.mesh` 文件——判定时只有面板会被反序列化成 TriMesh。
    write_mesh: bool,
}

impl Body {
    fn geo_hash(&self) -> String {
        format!("zzfx_{}", self.seq)
    }
}

fn bodies() -> Vec<Body> {
    let pane = |seq, min, max| Body {
        seq,
        noun: "PANE",
        owner_seq: 2,
        min,
        max,
        write_mesh: true,
    };
    let part = |seq, cx: f32, half: f32| Body {
        seq,
        noun: "BOX",
        owner_seq: 2,
        min: Vec3::new(cx - half, 500.0 - half, 500.0 - half),
        max: Vec3::new(cx + half, 500.0 + half, 500.0 + half),
        write_mesh: false,
    };
    vec![
        pane(
            10,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1000.0, 1000.0, 1000.0),
        ),
        pane(
            11,
            Vec3::new(900.0, 0.0, 0.0),
            Vec3::new(1900.0, 1000.0, 1000.0),
        ),
        part(20, 200.0, 50.0),  // 完全在 A 内
        part(21, 500.0, 50.0),  // 完全在 A 内
        part(22, 1500.0, 50.0), // 完全在 B 内
        part(23, 1700.0, 50.0), // 完全在 B 内
        part(24, 950.0, 100.0), // 骑在 A/B 重叠区，走第二轮逐点判定
    ]
}

/// 面板 A / B 的 refno，供断言使用。
pub fn panel_refnos() -> (String, String) {
    (refno(10), refno(11))
}

/// 完全在 A 内、完全在 B 内、以及跨界的构件 refno。
pub fn part_refnos() -> (Vec<String>, Vec<String>, String) {
    (
        vec![refno(20), refno(21)],
        vec![refno(22), refno(23)],
        refno(24),
    )
}

/// 建夹具：写 `.mesh` 文件 + 灌库。幂等——先调 [`drop_room_fixture`] 清干净再建。
pub async fn create_room_fixture(mesh_dir: &Path) -> anyhow::Result<()> {
    drop_room_fixture(mesh_dir).await?;
    std::fs::create_dir_all(mesh_dir)?;

    let bodies = bodies();
    let mut sql = String::new();

    // 房间节点：pe 供图遍历，类型表供 `build_room_panels_relate_common` 的 `from FRMW`。
    // `pe.owner` 与 `inst_relate.generic` 都是 `GeomInstQuery` 的**非 Option** 字段
    // （`owner: RefnoEnum` / `generic: String`）。缺任何一个，`query_insts` 就会以
    // 「expected a string, found None」失败，而 `cal_room_refnos` 又把这个错误
    // `unwrap_or_default()` 吞成空 Vec，整间房静悄悄算成 0 个成员。
    // `pe.name` 同样是下游非 Option 字段：计划层（`resolve_unit_rollup`）加载
    // OWNER 图时 SELECT id, owner, noun, name——缺 name 会以「expected a string,
    // found None」整批失败。房间路径自身不读它，但 D12 的触发用例要把夹具窗口
    // 喂给 `build_model_update_plan`。
    sql.push_str(&format!(
        "CREATE pe:{r} SET noun = 'FRMW', deleted = false, owner = pe:{r}, name = '{ROOM_NAME}';\
         CREATE FRMW:{r} SET NAME = '{ROOM_NAME}', REFNO = pe:{r};\
         CREATE pe:{w} SET noun = 'CWALL', deleted = false, owner = pe:{r}, name = '';\
         RELATE pe:{w}->pe_owner->pe:{r};",
        r = refno(1),
        w = refno(2),
    ));
    sql.push_str(&format!(
        "INSERT IGNORE INTO trans {{id: trans:zzfx_id, d: {IDENTITY_TRANS}}};"
    ));

    for body in &bodies {
        let aabb = Aabb::new(body.min.into(), body.max.into());
        let geo_hash = body.geo_hash();
        let seq_refno = refno(body.seq);

        if body.write_mesh {
            let mesh = box_mesh(body.min, body.max);
            mesh.ser_to_file(&mesh_dir.join(format!("{geo_hash}.mesh")))?;
        }

        // 第二轮点检查读的是 inst_geo.pts → vec3.d，用盒子的 8 个角点。
        let mut pt_ids = Vec::new();
        for (i, v) in box_mesh(body.min, body.max).vertices.iter().enumerate() {
            let id = format!("zzfx_{}_{i}", body.seq);
            sql.push_str(&format!(
                "INSERT IGNORE INTO vec3 {{id: vec3:{id}, d: [{}, {}, {}]}};",
                v.x, v.y, v.z
            ));
            pt_ids.push(format!("vec3:{id}"));
        }

        sql.push_str(&format!(
            "INSERT IGNORE INTO aabb {{id: aabb:{geo_hash}, d: {}}};\
             CREATE pe:{seq_refno} SET noun = '{noun}', deleted = false, owner = pe:{owner}, \
                 name = '';\
             RELATE pe:{seq_refno}->pe_owner->pe:{owner};\
             CREATE inst_info:{geo_hash};\
             CREATE inst_geo:{geo_hash} SET meshed = true, visible = true, \
                 aabb = aabb:{geo_hash}, pts = [{pts}];\
             RELATE inst_info:{geo_hash}->geo_relate->inst_geo:{geo_hash} \
                 SET trans = trans:zzfx_id, geo_type = 'Pos', visible = true, \
                     geom_refno = pe:{seq_refno};\
             RELATE pe:{seq_refno}->inst_relate:{seq_refno}->inst_info:{geo_hash} \
                 SET world_trans = trans:zzfx_id, aabb = aabb:{geo_hash}, solid = true, \
                     generic = 'UNKOWN';",
            aabb_json(&aabb),
            noun = body.noun,
            owner = refno(body.owner_seq),
            pts = pt_ids.join(", "),
        ));
    }

    SUL_DB.query(sql).await?.check()?;
    Ok(())
}

/// 把一个构件搬到新位置。
///
/// 只动几何侧（`aabb` 记录、`inst_geo.pts`，面板还要重写 `.mesh`），不碰
/// `inst_relate.aabb`——后者由 `update_inst_relate_aabbs_by_refnos` 从 `geo_relate`
/// 重算，走的正是生成流程那条路。测试若直接改 `inst_relate.aabb`，就绕过了「包围盒
/// 真的变了」的触发源，等于没测。
pub async fn move_fixture_body(
    mesh_dir: &Path,
    seq: u64,
    min: Vec3,
    max: Vec3,
) -> anyhow::Result<()> {
    let geo_hash = format!("zzfx_{seq}");
    // 面板的归属判定读的是 `.mesh` 里的三角网而不是包围盒，不重写它，面板就还停在原处。
    if bodies()
        .iter()
        .any(|body| body.seq == seq && body.write_mesh)
    {
        box_mesh(min, max).ser_to_file(&mesh_dir.join(format!("{geo_hash}.mesh")))?;
    }
    let mut sql = format!(
        "UPDATE aabb:{geo_hash} SET d = {};",
        aabb_json(&Aabb::new(min.into(), max.into()))
    );
    for (i, v) in box_mesh(min, max).vertices.iter().enumerate() {
        sql.push_str(&format!(
            "UPDATE vec3:zzfx_{seq}_{i} SET d = [{}, {}, {}];",
            v.x, v.y, v.z
        ));
    }
    SUL_DB.query(sql).await?.check()?;
    Ok(())
}

/// 拆夹具：删库里的记录与 `.mesh` 文件。对不存在的记录是安全的。
pub async fn drop_room_fixture(mesh_dir: &Path) -> anyhow::Result<()> {
    let mut ids = vec![
        format!("pe:{}", refno(1)),
        format!("FRMW:{}", refno(1)),
        format!("pe:{}", refno(2)),
        format!("trans:zzfx_id"),
    ];
    for body in bodies() {
        let geo_hash = body.geo_hash();
        let seq_refno = refno(body.seq);
        ids.push(format!("pe:{seq_refno}"));
        ids.push(format!("inst_relate:{seq_refno}"));
        ids.push(format!("inst_info:{geo_hash}"));
        ids.push(format!("inst_geo:{geo_hash}"));
        ids.push(format!("geo_relate:{geo_hash}"));
        ids.push(format!("aabb:{geo_hash}"));
        for i in 0..8 {
            ids.push(format!("vec3:zzfx_{}_{i}", body.seq));
        }
        let path: PathBuf = mesh_dir.join(format!("{geo_hash}.mesh"));
        if path.exists() {
            std::fs::remove_file(path)?;
        }
    }

    // pe_owner / geo_relate / inst_relate / room_relate 的边按端点删，id 不完全可预测。
    let pes = (0..25)
        .map(|seq| format!("pe:{}", refno(seq)))
        .collect::<Vec<_>>()
        .join(", ");
    SUL_DB
        .query(format!(
            "DELETE pe_owner WHERE in IN [{pes}] OR out IN [{pes}];\
             DELETE room_relate WHERE in IN [{pes}] OR out IN [{pes}];\
             DELETE room_panel_relate WHERE in IN [{pes}] OR out IN [{pes}];\
             DELETE inst_relate WHERE in IN [{pes}];\
             DELETE {};",
            ids.join(", ")
        ))
        .await?
        .check()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::aabb_tree::rebuild_tree_from_pointers;
    use crate::fast_model::occ_generate::update_inst_relate_aabbs_by_refnos;
    use crate::fast_model::room_model::build_room_relations;
    use aios_core::room::room::load_aabb_tree;
    use aios_core::{RefnoEnum, get_db_option};
    use std::collections::HashSet;
    use surrealdb::opt::{Config, auth::Root};

    /// 盒子的绕序 / 闭合性必须撑得住 `contains_point`——`cal_room_refnos` 的两轮判定
    /// 全押在它上面。这条不连库，先把几何本身钉死，免得端到端失败时分不清是数据问题
    /// 还是网格问题。
    #[test]
    fn box_mesh_supports_point_containment() {
        use parry3d::math::{Isometry, Point};
        use parry3d::query::PointQuery;
        use parry3d::shape::TriMeshFlags;

        let mesh = box_mesh(Vec3::ZERO, Vec3::splat(1000.0));
        let tri = mesh
            .get_tri_mesh_with_flag(
                glam::Mat4::IDENTITY,
                TriMeshFlags::ORIENTED | TriMeshFlags::MERGE_DUPLICATE_VERTICES,
            )
            .expect("box mesh -> trimesh");
        assert!(
            tri.contains_point(&Isometry::identity(), &Point::new(500.0, 500.0, 500.0)),
            "盒心应判为在内"
        );
        assert!(
            !tri.contains_point(&Isometry::identity(), &Point::new(1500.0, 500.0, 500.0)),
            "盒外的点应判为在外"
        );
    }

    /// 排障用：把 `cal_room_refnos` 的三个输入逐个打出来（实例、树、判定结果）。
    /// 需要先跑过带 `AIOS_KEEP_FIXTURE=1` 的用例把夹具留在库里。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: diagnostic for an existing fixture"]
    async fn live_room_fixture_probe() {
        use crate::fast_model::room_model::cal_room_refnos;
        use aios_core::room::room::GLOBAL_AABB_TREE;

        connect_live().await;
        let db_option = get_db_option().clone();
        let mesh_dir = db_option.get_meshes_path();

        let panel: RefnoEnum = "4000000001_10".into();
        println!("panel refno parsed = {panel} / {:?}", panel.refno());

        let insts = aios_core::query_insts(&[panel], true).await;
        println!("query_insts -> {:?}", insts.as_ref().map(|v| v.len()));
        if let Ok(v) = &insts {
            for g in v {
                println!("  world_aabb={:?} insts={}", g.world_aabb, g.insts.len());
                for i in &g.insts {
                    let p = mesh_dir.join(format!("{}.mesh", i.geo_hash));
                    println!(
                        "    geo_hash={} mesh_exists={} des_ok={}",
                        i.geo_hash,
                        p.exists(),
                        PlantMesh::des_mesh_file(&p).is_ok()
                    );
                }
            }
        }

        load_aabb_tree().await.expect("load tree");
        println!("tree size = {}", GLOBAL_AABB_TREE.read().await.tree.size());
        let hit = GLOBAL_AABB_TREE
            .read()
            .await
            .locate_intersecting_bounds(&Aabb::new(Vec3::ZERO.into(), Vec3::splat(1000.0).into()))
            .map(|b| b.refno.to_string())
            .collect::<Vec<_>>();
        println!("tree hits in panel A bounds = {hit:?}");

        let within = cal_room_refnos(&mesh_dir, panel, &HashSet::new()).await;
        println!("cal_room_refnos -> {within:?}");
    }

    async fn connect_live() {
        let endpoint = std::env::var("AIOS_LIVE_WS").expect("set AIOS_LIVE_WS");
        let ns = std::env::var("AIOS_LIVE_NS").unwrap_or_else(|_| "1516".into());
        let db = std::env::var("AIOS_LIVE_DB").unwrap_or_else(|_| "AvevaMarineSample".into());
        // 这些 live 用例只能**逐个**运行，不能一把 `--ignored` 全跑：`SUL_DB` 是进程级
        // 全局，而每个用例各建一个 tokio 运行时；第一个用例结束时它的运行时被丢弃，
        // 连接的后台任务随之死掉，后面的用例拿到的是一条已关闭的连接
        // （表现为 AlreadyConnected 或 "sending into a closed channel"）。
        if let Err(error) = SUL_DB
            .connect((endpoint, Config::default().ast_payload()))
            .with_capacity(1000)
            .await
        {
            panic!(
                "connect: {error:?}\nlive 用例需逐个运行：\
                 cargo test --lib <用例名> -- --ignored --exact --nocapture"
            );
        }
        SUL_DB.use_ns(&ns).use_db(&db).await.expect("use ns/db");
        SUL_DB
            .signin(Root {
                username: "root",
                password: "root",
            })
            .await
            .expect("signin");
    }

    /// D12（ADR-010）：房间改名与 PANE 搬迁都不改任何 AABB，唯一触发在计划层。
    /// 本用例在真库上验证那两条规则的端到端前半程——窗口操作 →
    /// `build_model_update_plan` → `RoomRecalcPanel` 工作项，其中房间改名要经
    /// `panels_under_rooms` 的真库子 + 孙查询（FRMW → CWALL → PANE）拿到面板。
    /// 工作项入队之后的消费路径（整间分支、先清后写、对拍）已由本文件其余
    /// live 用例覆盖，这里不重复。
    ///
    /// 触发判定只看窗口 delta 里的名字（旧名 `/1RX-RM03-R301` 命中全局关键字
    /// `-RM`），与夹具房间的实际 NAME 解耦——D12 的语义本来就是「改出房间也要
    /// 重算」。
    ///
    /// ```text
    /// ./scripts/Start-Surreal8009.ps1 -Memory -Bind 127.0.0.1:8071
    /// AIOS_LIVE_WS=ws://localhost:8071 cargo test --lib \
    ///     live_room_structural_triggers_enqueue_panel_recalc -- --ignored --exact --nocapture
    /// ```
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: writes fixture records and .mesh files"]
    async fn live_room_structural_triggers_enqueue_panel_recalc() {
        use crate::data_interface::model_update_plan::{ModelWorkAction, build_model_update_plan};
        use aios_core::NamedAttrValue;
        use pdms_io::io::{EleOperationData, EleOperationDetail, ModifiedElement};
        use std::collections::BTreeMap;

        fn fixture_modified_op(
            seq: u64,
            noun: &str,
            attr: &str,
            old_value: NamedAttrValue,
            new_value: NamedAttrValue,
        ) -> EleOperationData {
            let mut modified_attrs = std::collections::HashMap::new();
            modified_attrs.insert(attr.to_string(), (old_value, new_value));
            EleOperationData::new(
                RefnoEnum::from(refno(seq).as_str()).refno(),
                7,
                EleOperationDetail::Modified(ModifiedElement {
                    current_data: Default::default(),
                    added_attrs: Default::default(),
                    deleted_attrs: Default::default(),
                    modified_attrs,
                    added_explicit_attrs: Default::default(),
                    deleted_explicit_attrs: Default::default(),
                    modified_explicit_attrs: Default::default(),
                    added_uda_attrs: Default::default(),
                    deleted_uda_attrs: Default::default(),
                    modified_uda_attrs: Default::default(),
                    noun: noun.to_string(),
                    children_changed: None,
                }),
            )
        }

        connect_live().await;
        let db_option = get_db_option().clone();
        let mesh_dir = db_option.get_meshes_path();
        create_room_fixture(&mesh_dir)
            .await
            .expect("create fixture");

        let dbnum = u32::try_from(FIXTURE_DBNUM).expect("fixture dbnum fits u32");
        let (pane_a, pane_b) = panel_refnos();
        let pane_a_target = RefnoEnum::from(pane_a.as_str()).to_pdms_str();
        let pane_b_target = RefnoEnum::from(pane_b.as_str()).to_pdms_str();

        // 场景 A：房间（FRMW，refno 1）改名，旧名命中关键字 → 名下两块 PANE 入队。
        let rename_ops = BTreeMap::from([(
            7u32,
            vec![fixture_modified_op(
                1,
                "FRMW",
                "NAME",
                NamedAttrValue::StringType("/1RX-RM03-R301".into()),
                NamedAttrValue::StringType("/1RX-FRAME-01".into()),
            )],
        )]);
        let rename_plan = build_model_update_plan(dbnum, 7, "DESI", &rename_ops)
            .await
            .expect("plan for room rename");
        let mut rename_targets: Vec<String> = rename_plan
            .work_items
            .iter()
            .filter(|item| item.action == ModelWorkAction::RoomRecalcPanel)
            .map(|item| item.target_refno.clone())
            .collect();
        rename_targets.sort();

        // 场景 B：PANE（refno 10）搬迁（OWNER 变更）→ 自身入队。
        let move_ops = BTreeMap::from([(
            8u32,
            vec![fixture_modified_op(
                10,
                "PANE",
                "OWNER",
                NamedAttrValue::RefU64Type(RefnoEnum::from(refno(2).as_str()).refno()),
                NamedAttrValue::RefU64Type(RefnoEnum::from(refno(1).as_str()).refno()),
            )],
        )]);
        let move_plan = build_model_update_plan(dbnum, 8, "DESI", &move_ops)
            .await
            .expect("plan for pane move");
        let move_targets: Vec<String> = move_plan
            .work_items
            .iter()
            .filter(|item| item.action == ModelWorkAction::RoomRecalcPanel)
            .map(|item| item.target_refno.clone())
            .collect();

        if std::env::var("AIOS_KEEP_FIXTURE").is_err() {
            drop_room_fixture(&mesh_dir).await.expect("drop fixture");
        }

        let mut want = vec![pane_a_target.clone(), pane_b_target];
        want.sort();
        assert_eq!(
            rename_targets, want,
            "房间改名必须为名下（子 + 孙）全部 PANE 排整间重算"
        );
        assert_eq!(
            move_targets,
            vec![pane_a_target],
            "PANE 搬迁必须为自身排整间重算"
        );
        // 改名与搬迁都是纯数据 / 结构变更：不得混进几何重生成工作项。
        assert!(
            rename_plan
                .work_items
                .iter()
                .all(|item| item.action == ModelWorkAction::RoomRecalcPanel),
            "{:?}",
            rename_plan.work_items
        );
    }

    /// H-1 的端到端回归：FRMW 在暂存窗口内改名「成为」合规房间时，提交后的
    /// RocksDB 房间轮必须从新拓扑解析出面板并算出归属。
    ///
    /// 走与生产数据批次（`execute_frozen_batch` 的 staged 路径）同一套窗口设施、
    /// 同一个顺序：窗口作用域内 `build_model_update_plan` → `stage_parsed_window`
    /// 解析改名 → 登记 finalize → `commit_registered_to` 写回持久层和 durable room
    /// pending → 释放窗口 → `drain_rooms_scoped` 从 RocksDB 计算本任务目标。
    ///
    /// 提交后两张房间关系表都要带着新房间号收敛，且不留任何 durable pending。
    ///
    /// 跨界构件 _24 骑在两块面板的重叠区上，第二轮逐点判定要读它的
    /// `inst_geo.pts`——结构触发预载只拷面板产物、不拷成员几何，暂存读不回落，
    /// 所以窗口内它两边都判不进。断言用 ⊇/⊆ 把它留成自由变量：现状不在不红，
    /// 将来预载扩到成员几何后出现也不红。
    ///
    /// 只能单独运行，见 [`connect_live`]：
    ///
    /// ```text
    /// ./scripts/Start-Surreal8009.ps1 -Memory -Bind 127.0.0.1:8071
    /// AIOS_LIVE_WS=ws://localhost:8071 cargo test --lib \
    ///     live_room_rename_into_compliance_recomputes_membership -- --ignored --exact --nocapture
    /// ```
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: writes fixture records, a watermark row and .mesh files"]
    async fn live_room_rename_into_compliance_recomputes_membership() {
        use crate::data_interface::increment_pipeline::IncrementPipeline;
        use crate::data_interface::model_update_pending::{RoomDrainScope, drain_rooms_scoped};
        use crate::data_interface::model_update_plan::{ModelWorkAction, build_model_update_plan};
        use crate::data_interface::staging::lifecycle::create_window_on;
        use crate::data_interface::staging::{
            ResourceThresholds, StagedFinalize, register_staged_finalize,
        };
        use crate::fast_model::room_model::load_room_panel_map;
        use aios_core::NamedAttrValue;
        use pdms_io::io::{EleOperationData, EleOperationDetail, ModifiedElement};
        use std::collections::BTreeMap;
        use surrealdb::engine::any::connect;

        // 两个名字都带 `-RM`：结构触发看的是**全局** DbOption 的关键字，映射加载
        // 看的是本用例传下去的 db_option——这里让两边用同一个词。旧名尾段 BAD9
        // 不满足 `^[A-Z]\d{3}$`，提交前不是合规房间；新名尾段 K200 合规。
        const OLD_NAME: &str = "/ZZ-RM-BAD9";
        const NEW_NAME: &str = "/ZZ-RM-K200";
        const NEW_ROOM_NUM: &str = "K200";

        fn rename_op(seq: u64, old_name: &str, new_name: &str, sesno: u32) -> EleOperationData {
            // NAME 放 `modified_explicit_attrs`：结构触发两个映射都认，而解析渲染
            // （`to_modify_surql`）只对显式 NAME 生成 `UPDATE pe SET name = …`。
            let mut modified_explicit_attrs = std::collections::HashMap::new();
            modified_explicit_attrs.insert(
                "NAME".to_string(),
                (
                    NamedAttrValue::StringType(old_name.into()),
                    NamedAttrValue::StringType(new_name.into()),
                ),
            );
            EleOperationData::new(
                RefnoEnum::from(refno(seq).as_str()).refno(),
                sesno,
                EleOperationDetail::Modified(ModifiedElement {
                    current_data: Default::default(),
                    added_attrs: Default::default(),
                    deleted_attrs: Default::default(),
                    modified_attrs: Default::default(),
                    added_explicit_attrs: Default::default(),
                    deleted_explicit_attrs: Default::default(),
                    modified_explicit_attrs,
                    added_uda_attrs: Default::default(),
                    deleted_uda_attrs: Default::default(),
                    modified_uda_attrs: Default::default(),
                    noun: "FRMW".to_string(),
                    children_changed: None,
                }),
            )
        }

        connect_live().await;
        let mut db_option = get_db_option().clone();
        db_option.room_key_word = Some(vec!["-RM".to_string()]);
        let mesh_dir = db_option.get_meshes_path();

        create_room_fixture(&mesh_dir)
            .await
            .expect("create fixture");
        let room = refno(1);
        // H-1 的前置态：命中关键字、但不合规——这间房在提交前不是房间。
        SUL_DB
            .query(format!(
                "UPDATE pe:{room} SET name = '{OLD_NAME}'; \
                 UPDATE FRMW:{room} SET NAME = '{OLD_NAME}';"
            ))
            .await
            .expect("rename fixture room to a non-compliant name")
            .check()
            .expect("valid initial rename");

        // 夹具几何进树（与其余 live 用例同一手法；刻意不 load_aabb_tree）。
        let fixture_refnos: Vec<RefnoEnum> = bodies()
            .iter()
            .map(|body| RefnoEnum::from(refno(body.seq).as_str()))
            .collect();
        update_inst_relate_aabbs_by_refnos(&fixture_refnos, true)
            .await
            .expect("push fixture aabbs into tree");

        let dbnum = u32::try_from(FIXTURE_DBNUM).expect("fixture dbnum fits u32");
        let end_sesno: i32 = 9;
        let (pane_a, pane_b) = panel_refnos();
        let pane_a_refno = RefnoEnum::from(pane_a.as_str());
        let pane_b_refno = RefnoEnum::from(pane_b.as_str());

        // 窗口起点：提交前合规房间映射。这间房不合规 → 不在映射里（正是 H-1 盲区）。
        let preloaded = load_room_panel_map(&db_option)
            .await
            .expect("load pre-window room map");
        assert!(
            preloaded.room_num_of(pane_a_refno).is_none(),
            "初始名不合规，提交前映射不该把它当房间: {:?}",
            preloaded.rooms
        );

        let instance = connect("mem://").await.expect("staging mem boots");
        let mut window = create_window_on(
            &instance,
            dbnum,
            end_sesno,
            end_sesno,
            ResourceThresholds::default(),
        )
        .await
        .expect("create staged window");
        // 与生产同序：先在窗口作用域内建计划——H-1 的结构触发预载在这里发生。
        let ops = BTreeMap::from([(
            end_sesno as u32,
            vec![rename_op(1, OLD_NAME, NEW_NAME, end_sesno as u32)],
        )]);
        let plan = window
            .scope(build_model_update_plan(dbnum, end_sesno, "DESI", &ops))
            .await
            .expect("plan inside the staged window");
        let mut planned: Vec<String> = plan
            .work_items
            .iter()
            .filter(|item| item.action == ModelWorkAction::RoomRecalcPanel)
            .map(|item| item.target_refno.clone())
            .collect();
        planned.sort();
        let mut want = vec![pane_a_refno.to_pdms_str(), pane_b_refno.to_pdms_str()];
        want.sort();
        assert_eq!(
            planned,
            want,
            "改名触发必须为名下两块 PANE 排整间任务（依赖全局 room_key_word 命中 `-RM`，\
             当前配置: {:?}）",
            aios_core::get_db_option().get_room_key_word()
        );
        assert_eq!(plan.work_items.len(), 2, "{:?}", plan.work_items);

        // 再解析入暂存：改名渲染为 `UPDATE pe SET name`，更新的正是预载拷进来的旧行。
        let staged = IncrementPipeline::stage_parsed_window(&mut window, &ops, dbnum)
            .await
            .expect("stage parsed rename");
        assert!(staged > 0, "改名会话必须进 journal");

        window
            .scope(register_staged_finalize(StagedFinalize {
                dbnum,
                start_sesno: end_sesno,
                end_sesno,
                plan: plan.clone(),
                window_statements: Vec::new(),
                cache_refnos: Vec::new(),
            }))
            .await
            .expect("register finalize");

        let room_scope = RoomDrainScope::from_plan(&plan);
        window
            .commit_registered_to(&SUL_DB)
            .await
            .expect("staged write-back");
        window.drop_database().await.expect("drop staging db");
        let report = drain_rooms_scoped(&db_option, &room_scope)
            .await
            .expect("post-commit scoped room drain");
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert_eq!(report.requested, 2);
        assert_eq!(report.done, 2);

        // ---- 提交后的持久层取证（先取数、后清理、再断言，与其余用例同一纪律）----
        let mut response = SUL_DB
            .query(format!("RETURN pe:{room}.name;"))
            .await
            .expect("query committed room name")
            .check()
            .expect("valid name query");
        let committed_name: Option<String> = response.take(0).expect("decode room name");

        let mut response = SUL_DB
            .query(
                "SELECT record::id(in) AS panel, record::id(out) AS part, room_num \
                 FROM room_relate ORDER BY panel, part;",
            )
            .await
            .expect("query room_relate")
            .check()
            .expect("valid room_relate query");
        let mut edges: Vec<Edge> = response.take(0).expect("decode room_relate");
        edges.sort();

        let mut response = SUL_DB
            .query(
                "SELECT record::id(in) AS panel, record::id(out) AS part, room_num \
                 FROM room_panel_relate ORDER BY panel, part;",
            )
            .await
            .expect("query room_panel_relate")
            .check()
            .expect("valid room_panel_relate query");
        // 复用 Edge 的形状：in=房间（panel 字段）、out=面板（part 字段）。
        let mut topology: Vec<Edge> = response.take(0).expect("decode room_panel_relate");
        topology.sort();

        let mut response = SUL_DB
            .query(
                "SELECT VALUE record::id(id) FROM model_update_pending \
                 WHERE action IN ['room_recalc_element', 'room_recalc_panel'];",
            )
            .await
            .expect("query pending rows")
            .check()
            .expect("valid pending query");
        let pending: Vec<String> = response.take(0).expect("decode pending rows");

        let mut response = SUL_DB
            .query(format!("RETURN dbnum_watermark:{dbnum}.applied_sesno;"))
            .await
            .expect("query watermark")
            .check()
            .expect("valid watermark query");
        let applied: Option<i32> = response.take(0).expect("decode watermark");

        // 收尾：水位行是本用例特有的落库，清掉；夹具与队列走公共清理。
        if std::env::var("AIOS_KEEP_FIXTURE").is_err() {
            SUL_DB
                .query(format!("DELETE dbnum_watermark:{dbnum};"))
                .await
                .expect("cleanup watermark")
                .check()
                .expect("valid watermark cleanup");
        }
        drop_fixture_and_queue(&mesh_dir).await;

        assert_eq!(
            committed_name.as_deref(),
            Some(NEW_NAME),
            "窗口解析的改名必须随 journal 写回持久层"
        );
        assert_eq!(applied, Some(end_sesno), "写回尾事务必须推进水位");
        assert!(
            pending.is_empty(),
            "提交后收敛的面板不得残留 durable pending: {pending:?}"
        );

        let (in_a, in_b, straddler) = part_refnos();
        assert!(
            edges.iter().all(|edge| edge.room_num == NEW_ROOM_NUM
                && (edge.panel == pane_a || edge.panel == pane_b)),
            "归属边只该属于这间房的两块面板、且带新房间号: {edges:#?}"
        );
        let a_parts: HashSet<String> = edges
            .iter()
            .filter(|edge| edge.panel == pane_a)
            .map(|edge| edge.part.clone())
            .collect();
        let b_parts: HashSet<String> = edges
            .iter()
            .filter(|edge| edge.panel == pane_b)
            .map(|edge| edge.part.clone())
            .collect();
        let a_min: HashSet<String> = in_a.iter().cloned().collect();
        let b_min: HashSet<String> = in_b.iter().cloned().collect();
        let mut a_max = a_min.clone();
        a_max.insert(straddler.clone());
        let mut b_max = b_min.clone();
        b_max.insert(straddler.clone());
        assert!(
            a_parts.is_superset(&a_min) && a_parts.is_subset(&a_max),
            "改名成为合规房间后，面板 A 必须当场算出完全在内的成员（H-1 修复前这里是空集）\
             \n实得: {a_parts:?}\n下界: {a_min:?}\n上界: {a_max:?}"
        );
        assert!(
            b_parts.is_superset(&b_min) && b_parts.is_subset(&b_max),
            "面板 B 同理\n实得: {b_parts:?}\n下界: {b_min:?}\n上界: {b_max:?}"
        );

        // 两张表同源一致：房间 → 面板拓扑恰是这两块面板，房间号一致。
        let mut want_topology = vec![
            Edge {
                panel: room.clone(),
                part: pane_a.clone(),
                room_num: NEW_ROOM_NUM.into(),
            },
            Edge {
                panel: room.clone(),
                part: pane_b.clone(),
                room_num: NEW_ROOM_NUM.into(),
            },
        ];
        want_topology.sort();
        assert_eq!(
            topology, want_topology,
            "room_panel_relate 必须与 room_relate 同一轮收敛出同一间房"
        );
    }

    #[derive(serde::Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    struct Edge {
        panel: String,
        part: String,
        room_num: String,
    }

    /// 夹具那间房当前的全部归属边，已排序，可直接相等比较。
    async fn room_edges() -> Vec<Edge> {
        let mut response = SUL_DB
            .query(
                "SELECT record::id(in) AS panel, record::id(out) AS part, room_num \
                 FROM room_relate WHERE room_num = 'K100' ORDER BY panel, part;",
            )
            .await
            .expect("query room_relate")
            .check()
            .expect("valid query");
        let mut edges: Vec<Edge> = response.take(0).expect("decode room_relate");
        edges.sort();
        edges
    }

    /// ADR-010 §9 的验收骨架：夹具 → `build_room_relations` → 逐边比对。
    ///
    /// 断言的是三种归属都算对：完全在内（走 AABB 八顶点快路径）、完全在外（不该出现）、
    /// 以及骑在两室重叠区上的那个（八顶点判不出来，必须落到第二轮逐点兜底，且两室都算）。
    ///
    /// **不要指向共享的工作库**——用一次性内存实例，夹具会写 pe / inst_* / room_* 多张表：
    ///
    /// ```text
    /// surreal start --user root --pass root --bind 127.0.0.1:8071 memory
    /// AIOS_LIVE_WS=ws://localhost:8071 cargo test live_room_fixture_parity -- --ignored --nocapture
    /// ```
    ///
    /// `assets/meshes` 下会多出两个 `zzfx_*.mesh`，用例结束时删除。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: writes fixture records and .mesh files"]
    async fn live_room_fixture_parity() {
        connect_live().await;

        let mut db_option = get_db_option().clone();
        // 只匹配夹具那一间，避免把库里 124 个真实房间一起卷进来。
        db_option.room_key_word = Some(vec!["ZZ-R-".to_string()]);
        let mesh_dir = db_option.get_meshes_path();

        create_room_fixture(&mesh_dir)
            .await
            .expect("create fixture");

        // 夹具构件要先进 R 树，cal_room_refnos 的候选集就是从树里捞的。
        // 刻意**不**调 `load_aabb_tree`：一次性实例上树应当只装夹具，
        // 免得把 `accel_tree.bin` 里几万条真实包围盒带进来干扰断言。
        let fixture_refnos: Vec<RefnoEnum> = bodies()
            .iter()
            .map(|b| RefnoEnum::from(refno(b.seq).as_str()))
            .collect();
        update_inst_relate_aabbs_by_refnos(&fixture_refnos, true)
            .await
            .expect("push fixture aabbs into tree");

        build_room_relations(&db_option)
            .await
            .expect("build room relations");

        let (pane_a, pane_b) = panel_refnos();
        let mut response = SUL_DB
            .query(
                "SELECT record::id(in) AS panel, record::id(out) AS part, room_num \
                 FROM room_relate WHERE room_num = 'K100' ORDER BY panel, part;",
            )
            .await
            .expect("query room_relate")
            .check()
            .expect("valid query");
        let mut got: Vec<Edge> = response.take(0).expect("decode room_relate");
        got.sort();

        // 排障时置 AIOS_KEEP_FIXTURE=1 把数据留在库里，便于用 SQL 逐段回溯。
        if std::env::var("AIOS_KEEP_FIXTURE").is_err() {
            drop_room_fixture(&mesh_dir).await.expect("drop fixture");
        }

        let (in_a, in_b, straddler) = part_refnos();
        let mut want: Vec<Edge> = Vec::new();
        for part in in_a.iter().chain(std::iter::once(&straddler)) {
            want.push(Edge {
                panel: pane_a.clone(),
                part: part.clone(),
                room_num: "K100".into(),
            });
        }
        for part in in_b.iter().chain(std::iter::once(&straddler)) {
            want.push(Edge {
                panel: pane_b.clone(),
                part: part.clone(),
                room_num: "K100".into(),
            });
        }
        want.sort();

        let got_set: HashSet<_> = got.iter().map(|e| (&e.panel, &e.part)).collect();
        let want_set: HashSet<_> = want.iter().map(|e| (&e.panel, &e.part)).collect();
        assert_eq!(
            got_set, want_set,
            "\n实得: {got:#?}\n期望: {want:#?}\n（跨界构件 {straddler} 应同时属于两室）"
        );
    }

    /// 建夹具 + 灌树 + 全量基线，返回夹具用的 `DbOption`（房间关键字已指向夹具那一间）。
    async fn fixture_baseline() -> aios_core::options::DbOption {
        let mut db_option = get_db_option().clone();
        // 只匹配夹具那一间，避免把库里的真实房间一起卷进来。
        db_option.room_key_word = Some(vec!["ZZ-R-".to_string()]);

        let mesh_dir = db_option.get_meshes_path();
        create_room_fixture(&mesh_dir)
            .await
            .expect("create fixture");
        let fixture_refnos: Vec<RefnoEnum> = bodies()
            .iter()
            .map(|body| RefnoEnum::from(refno(body.seq).as_str()))
            .collect();
        update_inst_relate_aabbs_by_refnos(&fixture_refnos, true)
            .await
            .expect("push fixture aabbs into tree");
        build_room_relations(&db_option)
            .await
            .expect("baseline full rebuild");
        assert_eq!(room_edges().await.len(), 6, "基线应有 6 条归属边");
        db_option
    }

    async fn drop_fixture_and_queue(mesh_dir: &Path) {
        if std::env::var("AIOS_KEEP_FIXTURE").is_ok() {
            return;
        }
        SUL_DB
            .query(
                "DELETE model_update_pending \
                 WHERE action IN ['room_recalc_element', 'room_recalc_panel'];",
            )
            .await
            .expect("cleanup queue")
            .check()
            .expect("cleanup queue statements");
        drop_room_fixture(mesh_dir).await.expect("drop fixture");
    }

    /// 整间分支的对拍：**搬动的是一块面板**，不是构件。
    ///
    /// 面板一动，它名下的成员整批换人，元素级根本表达不了——这正是 ADR-010 §2 要分出
    /// 两种任务的理由。这里把 B 房的面板往外挪，让原本骑在 A/B 重叠区上的跨界构件掉出
    /// B 房，然后只跑整间分支，再与全量重建逐边比较。
    ///
    /// 只能单独运行，见 [`connect_live`]。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: writes fixture records, queue rows and .mesh files"]
    async fn live_room_panel_move_parity() {
        use crate::data_interface::model_update_pending::enqueue_room_recalc;
        use crate::fast_model::room_model::{load_room_panel_map, recalc_panel_membership};

        connect_live().await;
        let db_option = fixture_baseline().await;
        let mesh_dir = db_option.get_meshes_path();

        // B 房的面板从 900..1900 挪到 1400..2400：跨界构件（850..1050）就此掉出 B 房，
        // 而 B 房原有的两个成员（1450..1550 / 1650..1750）仍在里面。
        let panel = RefnoEnum::from(refno(11).as_str());
        move_fixture_body(
            &mesh_dir,
            11,
            Vec3::new(1400.0, 0.0, 0.0),
            Vec3::new(2400.0, 1000.0, 1000.0),
        )
        .await
        .expect("move panel B");

        let changes = update_inst_relate_aabbs_by_refnos(&[panel], true)
            .await
            .expect("refresh moved panel aabb");
        assert_eq!(
            changes
                .iter()
                .map(|change| (change.refno, change.noun.as_str()))
                .collect::<Vec<_>>(),
            vec![(panel, "PANE")],
            "只有被搬走的那块面板的包围盒变了"
        );
        enqueue_room_recalc(&changes)
            .await
            .expect("enqueue room work");
        let mut response = SUL_DB
            .query(
                "SELECT VALUE record::id(id) FROM model_update_pending \
                 WHERE action = 'room_recalc_panel';",
            )
            .await
            .expect("query queued room work")
            .check()
            .expect("valid queue query");
        let queued: Vec<String> = response.take(0).expect("decode queued room work");
        assert_eq!(
            queued,
            vec!["room_recalc_panel_4000000001_11"],
            "PANE 必须走整间分支"
        );

        let rooms = load_room_panel_map(&db_option)
            .await
            .expect("load room panel map");
        recalc_panel_membership(&db_option, &rooms, panel)
            .await
            .expect("incremental whole-room recalc");
        let incremental = room_edges().await;

        build_room_relations(&db_option)
            .await
            .expect("full rebuild after move");
        let full = room_edges().await;

        drop_fixture_and_queue(&mesh_dir).await;

        assert_eq!(
            incremental, full,
            "\n增量: {incremental:#?}\n全量: {full:#?}"
        );
        // 搬家要看得见：跨界构件掉出 B 房，但仍留在 A 房。
        let (pane_a, pane_b) = panel_refnos();
        let (_, _, straddler) = part_refnos();
        assert!(
            !full
                .iter()
                .any(|edge| edge.panel == pane_b && edge.part == straddler),
            "跨界构件应已掉出 B 房: {full:#?}"
        );
        assert!(
            full.iter()
                .any(|edge| edge.panel == pane_a && edge.part == straddler),
            "它在 A 房的归属不该被牵连: {full:#?}"
        );
    }

    /// 同轮冲突规则（ADR-010 §8）：整间分支已经写过的构件，其元素任务被吸收跳过。
    ///
    /// 两条分支的删除范围不同——一个按面板删出边，一个按构件删入边。真正的风险是它们
    /// 互相踩：元素分支若在整间分支之后拿着过期的候选集跑一遍，会把刚写好的边删掉。
    /// 这里把一块面板的任务和它名下一个成员的任务塞进同一轮，跑完整的第三阶段，断言
    /// 边集**一条不变**、两行队列都被消费掉。
    ///
    /// 「一条不变」同时也是幂等性：在没有任何变更的数据上重算，结果必须与基线相同。
    ///
    /// 只能单独运行，见 [`connect_live`]。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: writes fixture records, queue rows and .mesh files"]
    async fn live_room_panel_task_absorbs_element_task_in_the_same_round() {
        use crate::data_interface::model_update_pending::{drain_rooms, enqueue_room_recalc};
        use crate::fast_model::occ_generate::AabbChange;

        connect_live().await;
        let db_option = fixture_baseline().await;
        let mesh_dir = db_option.get_meshes_path();
        let baseline = room_edges().await;

        // 面板 A 与它名下的一个成员，同一轮一起入队。
        let panel = RefnoEnum::from(refno(10).as_str());
        let member = RefnoEnum::from(refno(21).as_str());
        enqueue_room_recalc(&[
            AabbChange {
                refno: panel,
                noun: "PANE".into(),
            },
            AabbChange {
                refno: member,
                noun: "BOX".into(),
            },
        ])
        .await
        .expect("enqueue both room tasks");

        let done = drain_rooms(&db_option)
            .await
            .expect("drain room phase")
            .done;
        let after = room_edges().await;

        let mut response = SUL_DB
            .query(
                "SELECT VALUE record::id(id) FROM model_update_pending \
                 WHERE action IN ['room_recalc_element', 'room_recalc_panel'];",
            )
            .await
            .expect("query leftover room work")
            .check()
            .expect("valid leftover query");
        let leftover: Vec<String> = response.take(0).expect("decode leftover room work");

        drop_fixture_and_queue(&mesh_dir).await;

        assert_eq!(done, 2, "两行任务都要被这一轮消费掉");
        assert!(
            leftover.is_empty(),
            "被吸收的元素任务也必须删行，否则它会一直卡在队列里: {leftover:?}"
        );
        assert_eq!(
            after, baseline,
            "\n重算后: {after:#?}\n基线: {baseline:#?}\n（数据没变，两条分支同轮跑完结果必须一致）"
        );
    }

    /// 吸收的封闭性（ADR-010 §8，2026-07-28 修订）：构件从「本轮未重算」的面板搬进
    /// 「本轮已重算」的面板时，吸收必须让路——只有元素分支那条「删全部入边」能清掉
    /// 旧面板指向它的陈旧边。修订前这里的 B→22 会永久留在库里，材料表随之读到
    /// 两个房间号，`fn::room_code` 取哪个全看旧排序键。
    ///
    /// 场景：构件 _22（原本完全在 B 内）搬进 A 的内部，同轮 A 面板自身外扩了一点
    /// （成员不变但包围盒变了，走整间分支），B 完全不在本轮。同轮 drain 后：
    /// A 收下 _22，B 对 _22 的旧边被元素分支清掉，且整体与全量重建逐边一致。
    ///
    /// 同样只能单独运行，见 [`connect_live`]：
    ///
    /// ```text
    /// AIOS_LIVE_WS=ws://localhost:8071 cargo test --lib \
    ///     live_room_cross_panel_move_defeats_absorption -- --ignored --exact --nocapture
    /// ```
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: writes fixture records, queue rows and .mesh files"]
    async fn live_room_cross_panel_move_defeats_absorption() {
        use crate::data_interface::model_update_pending::{drain_rooms, enqueue_room_recalc};
        use crate::fast_model::room_model::build_room_relations;

        connect_live().await;
        let db_option = fixture_baseline().await;
        let mesh_dir = db_option.get_meshes_path();

        let panel_a = RefnoEnum::from(refno(10).as_str());
        let mover = RefnoEnum::from(refno(22).as_str());
        let (pane_a, pane_b) = panel_refnos();

        // _22 从 B 的内部（1450..1550）搬进 A 的内部；A 面板往 -x 外扩 50，
        // 原有成员照旧、包围盒确实变了。B 一动不动，不会出现在本轮任务里。
        move_fixture_body(
            &mesh_dir,
            22,
            Vec3::new(300.0, 450.0, 450.0),
            Vec3::new(400.0, 550.0, 550.0),
        )
        .await
        .expect("move part 22 into room A");
        move_fixture_body(
            &mesh_dir,
            10,
            Vec3::new(-50.0, 0.0, 0.0),
            Vec3::new(1000.0, 1000.0, 1000.0),
        )
        .await
        .expect("expand panel A");

        let changes = update_inst_relate_aabbs_by_refnos(&[panel_a, mover], true)
            .await
            .expect("refresh moved aabbs");
        assert_eq!(
            changes.len(),
            2,
            "面板与构件的包围盒都该判为变了: {changes:?}"
        );
        enqueue_room_recalc(&changes)
            .await
            .expect("enqueue both room tasks");

        let done = drain_rooms(&db_option)
            .await
            .expect("drain room phase")
            .done;
        let incremental = room_edges().await;

        build_room_relations(&db_option)
            .await
            .expect("full rebuild after cross-panel move");
        let full = room_edges().await;

        drop_fixture_and_queue(&mesh_dir).await;

        assert_eq!(done, 2, "两行任务都要被这一轮消费掉");
        assert!(
            !incremental
                .iter()
                .any(|edge| edge.panel == pane_b && edge.part == refno(22)),
            "B 对搬走构件的陈旧边必须被元素分支清掉（吸收让路）: {incremental:#?}"
        );
        assert!(
            incremental
                .iter()
                .any(|edge| edge.panel == pane_a && edge.part == refno(22)),
            "构件搬进 A 后必须收进 A 的成员: {incremental:#?}"
        );
        assert_eq!(
            incremental, full,
            "\n增量: {incremental:#?}\n全量: {full:#?}\n（增量收敛结果必须等于全量重建结果）"
        );
    }

    /// 删除路径：被删元素的房间归属必须当场清干净，而不是留成悬空边（ADR-010 §4）。
    ///
    /// 此前生产路径上**从来没有人删过** `room_relate`——全仓只有夹具清理里有一条删除
    /// 语句，于是房间归属只增不减。这条用例分两步：先删一个普通构件（只有入边），
    /// 再删一整块面板（还有出边和 `room_panel_relate`），顺带确认它们也从空间树上
    /// 摘掉了——留在树里的话，之后的重算会把一个已经不存在的构件算进某间房。
    ///
    /// 同样只能单独运行，见 [`connect_live`]：
    ///
    /// ```text
    /// AIOS_LIVE_WS=ws://localhost:8071 cargo test --lib \
    ///     live_room_delete_clears_membership -- --ignored --exact --nocapture
    /// ```
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: writes fixture records and .mesh files"]
    async fn live_room_delete_clears_membership() {
        use crate::data_interface::helper::delete_inst_relate_subtree;
        use aios_core::room::room::GLOBAL_AABB_TREE;

        connect_live().await;
        let db_option = fixture_baseline().await;
        let mesh_dir = db_option.get_meshes_path();

        // 第一步：删一个普通构件。它只有入边。
        let part = RefnoEnum::from(refno(20).as_str());
        delete_inst_relate_subtree(&[part], 300)
            .await
            .expect("delete a member part");
        let after_part = room_edges().await;
        assert!(
            after_part.iter().all(|edge| edge.part != refno(20)),
            "被删构件不该再有归属边: {after_part:#?}"
        );
        assert_eq!(after_part.len(), 5, "只该少掉它自己那一条: {after_part:#?}");
        assert!(
            !GLOBAL_AABB_TREE
                .read()
                .await
                .tree
                .iter()
                .any(|bbox| bbox.refno == part.refno()),
            "被删构件必须从空间树上摘掉"
        );

        // 第二步：删一整块面板。它还有出边和 room_panel_relate。
        let panel = RefnoEnum::from(refno(10).as_str());
        delete_inst_relate_subtree(&[panel], 300)
            .await
            .expect("delete a panel");
        let after_panel = room_edges().await;
        let (pane_a, pane_b) = panel_refnos();
        assert!(
            after_panel.iter().all(|edge| edge.panel != pane_a),
            "被删面板不该再收着任何成员: {after_panel:#?}"
        );
        assert!(
            after_panel.iter().any(|edge| edge.panel == pane_b),
            "另一块面板的归属不该被牵连: {after_panel:#?}"
        );

        let mut response = SUL_DB
            .query("SELECT VALUE record::id(out) FROM room_panel_relate;")
            .await
            .expect("query room_panel_relate")
            .check()
            .expect("valid room_panel_relate query");
        // 在 Rust 侧排序：`SELECT VALUE` 的 ORDER BY 字段必须出现在投影里，而这里投影的
        // 是 `record::id(out)` 而不是 `out` 本身。
        let mut panels: Vec<String> = response.take(0).expect("decode room_panel_relate");
        panels.sort();

        drop_fixture_and_queue(&mesh_dir).await;

        assert_eq!(
            panels,
            vec![pane_b],
            "房间-面板映射里也不该再留着被删的那块面板"
        );
    }

    /// ADR-010 §9 的验收硬标准：**增量收敛结果 == 全量重建结果**。
    ///
    /// 全量建基线 → 把一个构件从 A 房搬到 B 房 → 只跑增量 → 在同一份数据上再跑一遍
    /// 全量 → 逐边比较。两条路径共用 `room_predicate` 的判定与 `{panel}_{element}`
    /// 边 id，这条用例就是在守它们不许分叉：共享谓词一旦被某一侧偷偷改了口径、
    /// 或者两边的边 id 走形，这里立刻红。
    ///
    /// 搬家本身也要断言看得见——只比「增量 == 全量」的话，两边同时算错（比如都算成
    /// 空集）也会相等。
    ///
    /// 同样**不要指向共享的工作库**，用一次性内存实例：
    ///
    /// ```text
    /// surreal start --user root --pass root --bind 127.0.0.1:8071 memory
    /// AIOS_LIVE_WS=ws://localhost:8071 cargo test live_room_incremental_parity -- --ignored --nocapture
    /// ```
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: writes fixture records, queue rows and .mesh files"]
    async fn live_room_incremental_parity() {
        use crate::data_interface::model_update_pending::enqueue_room_recalc;
        use crate::fast_model::room_model::{
            ElementRoomHistory, load_panel_index, load_room_panel_map, recalc_element_membership,
        };

        connect_live().await;
        let db_option = fixture_baseline().await;
        let mesh_dir = db_option.get_meshes_path();

        // 把完全在 A 房内的 _20 搬进 B 房。
        let moved = RefnoEnum::from(refno(20).as_str());
        move_fixture_body(
            &mesh_dir,
            20,
            Vec3::new(1450.0, 450.0, 450.0),
            Vec3::new(1550.0, 550.0, 550.0),
        )
        .await
        .expect("move part across rooms");

        // 触发源（ADR §4）：刷新包围盒拿到变更集，再按它入队。
        let changes = update_inst_relate_aabbs_by_refnos(&[moved], true)
            .await
            .expect("refresh moved aabb");
        assert_eq!(
            changes
                .iter()
                .map(|change| (change.refno, change.noun.as_str()))
                .collect::<Vec<_>>(),
            vec![(moved, "BOX")],
            "只有被搬走的那个构件的包围盒变了"
        );
        enqueue_room_recalc(&changes)
            .await
            .expect("enqueue room work");
        let mut response = SUL_DB
            .query(
                "SELECT VALUE record::id(id) FROM model_update_pending \
                 WHERE action = 'room_recalc_element';",
            )
            .await
            .expect("query queued room work")
            .check()
            .expect("valid queue query");
        let queued: Vec<String> = response.take(0).expect("decode queued room work");
        assert_eq!(queued, vec!["room_recalc_element_4000000001_20"]);

        // 增量收敛：只重算这一个构件的归属，其余五条边不该被碰。
        let rooms = load_room_panel_map(&db_option)
            .await
            .expect("load room panel map");
        let panels = load_panel_index(&db_option, &rooms)
            .await
            .expect("load panel index");
        assert_eq!(
            panels.usable_panels(),
            2,
            "夹具的两块面板都要带着可用几何进候选索引，否则这条对拍测的是空集对空集"
        );
        let history = ElementRoomHistory::load(&[moved])
            .await
            .expect("load element room history");
        recalc_element_membership(&rooms, &panels, &history, moved)
            .await
            .expect("incremental recalc");
        let incremental = room_edges().await;

        // 同一份数据上再跑一遍全量。
        build_room_relations(&db_option)
            .await
            .expect("full rebuild after move");
        let full = room_edges().await;

        drop_fixture_and_queue(&mesh_dir).await;

        assert_eq!(
            incremental, full,
            "\n增量: {incremental:#?}\n全量: {full:#?}"
        );
        let (pane_a, pane_b) = panel_refnos();
        let part = refno(20);
        assert!(
            full.iter()
                .any(|edge| edge.part == part && edge.panel == pane_b),
            "搬过去的构件应属于 B 房: {full:#?}"
        );
        assert!(
            !full
                .iter()
                .any(|edge| edge.part == part && edge.panel == pane_a),
            "搬走之后 A 房不该再收着它: {full:#?}"
        );
    }

    /// issue #7 的原样复刻：**先手动删掉一个构件的房间边，再挪动它，增量必须把边建回来。**
    ///
    /// 报告人的步骤就是这两步（`delete from room_relate:⟨…⟩` ×2，然后把 Z 坐标改成
    /// 5821.67），结果是房间号查不到数据。这里用夹具把同一个序列钉成回归：构件在**同一
    /// 间房内**平移，正确答案因此是「边原样回来」，比搬家更能暴露「写回那一步没发生」。
    ///
    /// 与 [`live_room_incremental_parity`] 的分工：那条守的是「增量 == 全量」这条收敛
    /// 性质，走的是直调元素分支；这条守的是**队列消费路径**（`drain_rooms` → 本轮
    /// `PanelIndex` → 元素分支），也就是生产上真正跑的那条，且起点是「边已经不在了」。
    ///
    /// 只能单独运行，见 [`connect_live`]：
    ///
    /// ```text
    /// AIOS_LIVE_WS=ws://localhost:8071 cargo test --lib \
    ///     live_room_deleted_edges_come_back_after_a_move -- --ignored --nocapture
    /// ```
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: writes fixture records, queue rows and .mesh files"]
    async fn live_room_deleted_edges_come_back_after_a_move() {
        use crate::data_interface::model_update_pending::{drain_rooms, enqueue_room_recalc};
        use aios_core::room::room::GLOBAL_AABB_TREE;

        connect_live().await;
        let db_option = fixture_baseline().await;
        let mesh_dir = db_option.get_meshes_path();

        let moved = RefnoEnum::from(refno(20).as_str());
        let part = refno(20);
        let (pane_a, pane_b) = panel_refnos();
        let baseline = room_edges().await;
        let baseline_own: Vec<Edge> = baseline
            .iter()
            .filter(|edge| edge.part == part)
            .cloned()
            .collect();
        assert_eq!(
            baseline_own.len(),
            1,
            "_20 完全在 A 房内，基线应恰有一条边: {baseline:#?}"
        );

        // 隔离 issue #7 的主嫌：业务库里的面板几何仍完整，但空间树故意不放任何 PANE。
        // 修复前元素分支从树里找候选，这时必然捞空；现在候选必须来自 PanelIndex。
        let panel_refs = HashSet::from([
            RefnoEnum::from(pane_a.as_str()).refno(),
            RefnoEnum::from(pane_b.as_str()).refno(),
        ]);
        let removed = GLOBAL_AABB_TREE.write().await.remove_by_refnos(&panel_refs);
        assert!(removed > 0, "夹具基线应先把 PANE 放进空间树");
        assert!(
            GLOBAL_AABB_TREE
                .read()
                .await
                .tree
                .iter()
                .all(|bbox| bbox.noun != "PANE"),
            "隔离变量失败：空间树里仍有 PANE"
        );

        // 第一步（报告人做的）：手动删掉这个构件的房间边。
        SUL_DB
            .query(format!("DELETE room_relate WHERE out = pe:{part};"))
            .await
            .expect("delete the part's room edges")
            .check()
            .expect("valid delete");
        assert!(
            !room_edges().await.iter().any(|edge| edge.part == part),
            "手动删除之后这个构件不该还有房间边"
        );

        // 第二步（报告人做的）：挪它——**留在同一间房内**，Z 抬高 170。
        move_fixture_body(
            &mesh_dir,
            20,
            Vec3::new(150.0, 450.0, 620.0),
            Vec3::new(250.0, 550.0, 720.0),
        )
        .await
        .expect("move part within the same room");

        let changes = update_inst_relate_aabbs_by_refnos(&[moved], true)
            .await
            .expect("refresh moved aabb");
        assert_eq!(
            changes
                .iter()
                .map(|change| (change.refno, change.noun.as_str()))
                .collect::<Vec<_>>(),
            vec![(moved, "BOX")],
            "包围盒确实变了，否则触发源不会点火"
        );
        enqueue_room_recalc(&changes)
            .await
            .expect("enqueue room work");

        // 走生产上真正的消费路径，而不是直调元素分支。
        let done = drain_rooms(&db_option).await.expect("drain room work").done;
        assert_eq!(done, 1, "那条元素任务必须被消费掉");

        let incremental = room_edges().await;
        rebuild_tree_from_pointers()
            .await
            .expect("restore complete spatial tree before full parity rebuild");
        build_room_relations(&db_option)
            .await
            .expect("full rebuild after move");
        let full = room_edges().await;

        drop_fixture_and_queue(&mesh_dir).await;

        let restored: Vec<Edge> = incremental
            .iter()
            .filter(|edge| edge.part == part)
            .cloned()
            .collect();
        assert_eq!(
            restored, baseline_own,
            "删掉的边必须被增量原样建回来（issue #7）\n增量: {incremental:#?}"
        );
        assert_eq!(
            restored[0].panel, pane_a,
            "构件没出 A 房，边就该回到 A 房: {restored:#?}"
        );
        assert_eq!(
            incremental, full,
            "\n增量: {incremental:#?}\n全量: {full:#?}"
        );
    }

    /// 造 / 重建一条「插入时自带 aabb 指针、geo 侧无从重算」的行——形状对齐生产上的
    /// 隐含直管段（TUBI/BOXI）：`inst_relate` 挂在 BRAN 名下、out 指向共享单位几何、
    /// `aabb` 在写入时就指向现成记录，`->geo_relate` 里没有可用的 `aabb`/`pts`。
    /// `recreate` 语义 = 生产上的「重生成」：先删行（`save_instance_data(replace)` 的
    /// 删除集现在包含隐含直管段），再按新盒子重插。
    async fn upsert_tubi_like_row(seq: u64, min: Vec3, max: Vec3) {
        let seq_refno = refno(seq);
        let sql = format!(
            "DELETE inst_relate:{seq_refno};\
             UPSERT pe:{seq_refno} SET noun = 'BRAN', deleted = false, owner = pe:{owner};\
             UPSERT inst_info:zzfx_tubi_unit;\
             UPSERT aabb:zzfx_tubi_{seq} SET d = {aabb};\
             RELATE pe:{seq_refno}->inst_relate:{seq_refno}->inst_info:zzfx_tubi_unit \
                 SET world_trans = trans:zzfx_id, aabb = aabb:zzfx_tubi_{seq}, \
                     solid = true, generic = 'PIPE';",
            owner = refno(2),
            aabb = aabb_json(&Aabb::new(min.into(), max.into())),
        );
        SUL_DB
            .query(sql)
            .await
            .expect("upsert tubi-like row")
            .check()
            .expect("valid tubi-like row statements");
    }

    async fn drop_tubi_like_row(seq: u64) {
        let seq_refno = refno(seq);
        SUL_DB
            .query(format!(
                "DELETE room_relate WHERE out = pe:{seq_refno};\
                 DELETE inst_relate:{seq_refno};\
                 DELETE pe:{seq_refno}, inst_info:zzfx_tubi_unit, aabb:zzfx_tubi_{seq};"
            ))
            .await
            .expect("drop tubi-like row")
            .check()
            .expect("valid tubi-like cleanup");
    }

    /// 隐含直管段类行（aabb 在插入时写死、geo 侧无从重算）的房间链路。
    ///
    /// 此前这类行被刷新层整体跳过：`replace=false` 时被 `and aabb=none` 过滤，
    /// `replace=true` 时因 `geo_aabbs` 为空被 `continue`——从未进过空间树，也就从未
    /// 参与过房间归属；重生成后树上（若有）也只剩旧位置。本用例钉住修复后的三段语义：
    ///
    /// 1. **回填**：树上首次见到 → 算变 → 元素分支把它算进 A 房；
    /// 2. **幂等**：删行重插同一个盒子（未动的重生成）→ 树上旧值相等 → 不算变——
    ///    这同时守着「重生成不再把根下全员白排一遍房间任务」的差异语义；
    /// 3. **搬家**：删行重插指向 B 房的新盒 → 算变 → 收敛到 B 房，且与全量重建逐边一致
    ///    （树上有它了，全量路径同样看得见它）。
    ///
    /// 只能单独运行，见 [`connect_live`]：
    ///
    /// ```text
    /// AIOS_LIVE_WS=ws://localhost:8071 cargo test --lib \
    ///     live_room_tubi_row_enters_tree_and_tracks_regen -- --ignored --exact --nocapture
    /// ```
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: writes fixture records, queue rows and .mesh files"]
    async fn live_room_tubi_row_enters_tree_and_tracks_regen() {
        use crate::data_interface::model_update_pending::{drain_rooms, enqueue_room_recalc};
        use crate::fast_model::room_model::build_room_relations;

        connect_live().await;
        let db_option = fixture_baseline().await;
        let mesh_dir = db_option.get_meshes_path();

        let tube = RefnoEnum::from(refno(30).as_str());
        let (pane_a, pane_b) = panel_refnos();
        let box_in_a = (
            Vec3::new(100.0, 100.0, 100.0),
            Vec3::new(300.0, 200.0, 200.0),
        );
        let box_in_b = (
            Vec3::new(1500.0, 100.0, 100.0),
            Vec3::new(1700.0, 200.0, 200.0),
        );

        // 1. 回填：首次刷新（regen 路径口径 replace=true），树上没有它 → 算变。
        upsert_tubi_like_row(30, box_in_a.0, box_in_a.1).await;
        let changes = update_inst_relate_aabbs_by_refnos(&[tube], true)
            .await
            .expect("first refresh of the tubi-like row");
        assert_eq!(
            changes
                .iter()
                .map(|change| (change.refno, change.noun.as_str()))
                .collect::<Vec<_>>(),
            vec![(tube, "BRAN")],
            "树上首次见到必须算变，且以 BRAN 身份走元素分支"
        );
        enqueue_room_recalc(&changes)
            .await
            .expect("enqueue backfill");
        drain_rooms(&db_option).await.expect("drain backfill");
        let after_backfill = room_edges().await;
        assert!(
            after_backfill
                .iter()
                .any(|edge| edge.panel == pane_a && edge.part == refno(30)),
            "回填后 A 房应收下管段: {after_backfill:#?}"
        );

        // 2. 幂等：未动的重生成（删行重插同一个盒子）不该再算变。
        upsert_tubi_like_row(30, box_in_a.0, box_in_a.1).await;
        let changes = update_inst_relate_aabbs_by_refnos(&[tube], true)
            .await
            .expect("noop-regen refresh");
        assert!(
            changes.is_empty(),
            "盒子没动的重生成不该再排房间任务: {changes:?}"
        );

        // 3. 搬家：重生成后管段挪进 B 房。
        upsert_tubi_like_row(30, box_in_b.0, box_in_b.1).await;
        let changes = update_inst_relate_aabbs_by_refnos(&[tube], true)
            .await
            .expect("move-regen refresh");
        assert_eq!(changes.len(), 1, "搬家必须算变: {changes:?}");
        enqueue_room_recalc(&changes).await.expect("enqueue move");
        drain_rooms(&db_option).await.expect("drain move");
        let incremental = room_edges().await;

        build_room_relations(&db_option)
            .await
            .expect("full rebuild after tubi move");
        let full = room_edges().await;

        drop_tubi_like_row(30).await;
        drop_fixture_and_queue(&mesh_dir).await;

        assert_eq!(
            incremental, full,
            "\n增量: {incremental:#?}\n全量: {full:#?}\n（管段进树之后全量路径必须同样看得见它）"
        );
        assert!(
            !incremental
                .iter()
                .any(|edge| edge.panel == pane_a && edge.part == refno(30)),
            "搬走之后 A 房不该再收着管段: {incremental:#?}"
        );
        assert!(
            incremental
                .iter()
                .any(|edge| edge.panel == pane_b && edge.part == refno(30)),
            "管段应已归入 B 房: {incremental:#?}"
        );
    }
}
