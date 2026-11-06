//! MBD 管道尺寸标注的宿主侧（计划 `.planning/2026-09-06-plant-mbd-pipe-dimension`，Phase 1）。
//!
//! 分层（plan D4）：`vendor/shared/plant-mbd`（junction → old/vendor/）是纯求解器——
//! 不读库、不 HTTP；本模块负责把一条
//! BRAN 从 E3D 库文件读成它要的**成员结点视图** [`plant_mbd::BranchQueryResult`]
//! （[`branch_query`]，数据来自 e3d-model 的 `route::walk_route` + `CataloguePoints`，
//! 即 PML `isobran.getbranmems` 的 `apos/adir/lpos/ldir/pos`），再调 `layout_branch`
//! 出冻结 V2 契约 [`plant_mbd::MbdV2PipeData`]（[`layout_pipe`]）。HTTP 端点在 `web_service`。
//!
//! 坐标：E3D 库文件里是 mm（源坐标系），契约以 `geometry_space = source_mm` +
//! [`SOURCE_TO_DESIGN_MM_TO_M`]（列主序对角缩放）交给前端换成设计坐标米。
//!
//! 全标注类型（plant3d-web#103 M7）：[`object`] 按 `[mbd_type_rules]` 把被选中的元素判成九种标注类型之一、
//! 取归属对象，再走 plant-mbd 统一入口；[`host`] 是各类型读取器共用的读法（属主链、可选属性、世界坐标、
//! 目录参数、SPAMAP 空间候选）。各类型的读取器：设备 [`equipment_query`]（M3），找墙候选 [`civil_faces`]
//! （设备先用，支架找墙与土建随后共用）。

pub mod branch_query;
pub mod civil_faces;
pub mod equipment_query;
pub mod host;
pub mod object;

use std::sync::Arc;

use e3d_io::db_element::DbSet;
use e3d_io::refno::RefNo;
use plant_mbd::{
    InMemoryDataSource, LayoutRequest, MbdV2PipeData, MbdV2SourceDatabase, MbdV2SourceVersion,
    layout_branch,
};

pub use branch_query::{BranchQueryError, branch_query, branch_query_with_diameters, wire_refno};
pub use civil_faces::{CivilFaceError, civil_candidate, locate_candidates};
pub use equipment_query::{EquipmentQueryError, query_equipment};
pub use object::{
    Classified, MbdTypeRules, ObjectQueryError, SubjectKind, classify, classify_with,
    equipment_view, layout_classified, layout_object_refno, support_parts,
};

/// `source_mm → design_m`：16 元素**列主序**（THREE `Matrix4.fromArray` 布局，平移在 12–14）。
/// 对角矩阵转置不变，顺序约定由前端 `fixtures/mbd-v2/source-to-design-translation.json` 钉死。
pub const SOURCE_TO_DESIGN_MM_TO_M: [f64; 16] = [
    0.001, 0.0, 0.0, 0.0, //
    0.0, 0.001, 0.0, 0.0, //
    0.0, 0.0, 0.001, 0.0, //
    0.0, 0.0, 0.0, 1.0,
];

/// 一条 BRAN 的管道尺寸标注：读库 → 成员结点视图 → 求解 → V2 契约。
///
/// `input_refno` 原样回填进 `MbdV2PipeData::input_refno`（前端 URL 里怎么写就怎么回），
/// `branch_refno` 归一化为 `w0_w1`（[`wire_refno`]）。
///
/// `meta.source_version` 由宿主填：读的是哪一库、哪一会话只有宿主知道，求解器是纯库，
/// 离线 CLI 照旧缺省。
pub fn layout_pipe(
    set: &Arc<DbSet>,
    refno: RefNo,
    input_refno: &str,
) -> Result<MbdV2PipeData, BranchQueryError> {
    let queried = branch_query_with_diameters(set, refno)?;
    let query = queried.result;
    let branch_refno = query.branch_refno.clone();
    let source = InMemoryDataSource {
        branches: vec![query],
    };
    let request = LayoutRequest::new(input_refno, branch_refno, SOURCE_TO_DESIGN_MM_TO_M);
    let mut data = layout_branch(&source, &request)
        .map_err(|message| BranchQueryError::Solver { refno, message })?;
    let version = queried.source_version;
    data.meta.source_version = Some(MbdV2SourceVersion {
        dbnum: version.dbnum,
        sesno: version.sesno,
        databases: version
            .databases
            .into_iter()
            .map(|database| MbdV2SourceDatabase {
                dbnum: database.dbnum,
                sesno: database.sesno,
                db_type: database.db_type,
            })
            .collect(),
    });
    Ok(data)
}
