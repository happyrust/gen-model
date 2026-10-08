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

pub mod branch_query;

use std::sync::Arc;

use e3d_io::db_element::DbSet;
use e3d_io::refno::RefNo;
use plant_mbd::{InMemoryDataSource, LayoutRequest, MbdV2PipeData, layout_branch};

pub use branch_query::{BranchQueryError, branch_query, branch_query_with_diameters, wire_refno};

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
pub fn layout_pipe(
    set: &Arc<DbSet>,
    refno: RefNo,
    input_refno: &str,
) -> Result<MbdV2PipeData, BranchQueryError> {
    let query = branch_query(set, refno)?;
    let branch_refno = query.branch_refno.clone();
    let source = InMemoryDataSource {
        branches: vec![query],
    };
    let request = LayoutRequest::new(input_refno, branch_refno, SOURCE_TO_DESIGN_MM_TO_M);
    layout_branch(&source, &request).map_err(|message| BranchQueryError::Solver { refno, message })
}
