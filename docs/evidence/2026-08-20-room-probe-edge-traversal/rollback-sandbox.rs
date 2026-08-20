//! 单节点「按需生成 + 房间归属」实机探针（2026-08-07）。
//!
//! 用法：`cargo run --bin node_gen_room_probe -- <refno>... [--force]`
//! （refno 用 `a/b` 形式；配置取当前目录 `DbOption.toml`，可用 `DB_OPTION_FILE` 覆盖。）
//!
//! 每个目标节点走生产同款链路：
//! 1. 幂等建立 AMS 1112/7997 基线；
//! 2. `dbnum_statuses`（只读）——目标库「文件最新会话 vs 权威水位」，确认解析是否齐平；
//! 3. 非容器节点 `ensure_model_generated`（生成根归一策略；CATA 依赖惰性按需解析，
//!    ref0 → db 文件由 `InMemoryCataLocator` 定位）；容器（WORL/SITE/ZONE）不进按需
//!    入口（那是给 viewer 的策略门），直接以容器为根走定向生成引擎——与整库全量
//!    「按 SITE 为根」同一条展开路径（`gen_all_geos_data` 定向分支）；
//! 4. `rebuild_tree_from_pointers` 后 `drain_rooms`：面板分支的成员候选取自空间树，
//!    顺序与批次 worker 房间轮的顺序钉一致（先空间树、再收房间）；
//! 5. 汇报房间归属：目标自身的归属边、生成根子树的归属分布、容器子树内在册房间的成员统计。

use std::collections::BTreeMap;

use aios_core::{RefnoEnum, SUL_DB};
use aios_database::data_interface::cata_closure::{CataDbLocator, InMemoryCataLocator};
use aios_database::data_interface::generation_root::is_coarse_hierarchy_noun;
use aios_database::data_interface::model_update_pending::drain_rooms;
use aios_database::data_interface::tidb_manager::AiosDBManager;
use aios_database::fast_model::room_model::match_room_name_hd;

const CHUNK: usize = 300;
const BASELINE_DBS: [(u32, &str); 2] = [(1112, "ams1112_0001"), (7997, "ams7997_0001")];

#[derive(Debug, serde::Deserialize)]
struct PeInfo {
    noun: String,
    name: Option<String>,
    dbnum: Option<u32>,
}

#[derive(Debug, serde::Deserialize)]
struct RoomCountRow {
    room_num: Option<String>,
    c: usize,
}

#[derive(Debug, serde::Deserialize)]
struct CountRow {
    count: usize,
}

async fn baseline_counts(dbnum: u32) -> anyhow::Result<(usize, usize)> {
    let mut response = SUL_DB
        .query(format!(
            "SELECT count() AS count FROM pe WHERE dbnum = {dbnum} GROUP ALL; \
             SELECT math::sum(count) AS count FROM dbnum_info_table \
             WHERE dbnum = {dbnum} GROUP ALL;"
        ))
        .await?
        .check()?;
    let pe = response
        .take::<Vec<CountRow>>(0)?
        .first()
        .map(|row| row.count)
        .unwrap_or_default();
    let info = response
        .take::<Vec<CountRow>>(1)?
        .first()
        .map(|row| row.count)
        .unwrap_or_default();
    Ok((pe, info))
}

async fn pe_info(refno: RefnoEnum) -> anyhow::Result<Option<PeInfo>> {
    let mut response = SUL_DB
        .query(format!(
            "SELECT noun, name, dbnum FROM {};",
            refno.to_pe_key()
        ))
        .await?
        .check()?;
    Ok(response.take::<Vec<PeInfo>>(0)?.into_iter().next())
}

fn pe_keys(chunk: &[RefnoEnum]) -> String {
    chunk
        .iter()
        .map(|refno| refno.to_pe_key())
        .collect::<Vec<_>>()
        .join(",")
}

/// 子树里已写入 inst_relate 的元素数（与 on_demand_model 的 written 口径一致）。
async fn written_instances(scope: &[RefnoEnum]) -> anyhow::Result<usize> {
    let mut total = 0usize;
    for chunk in scope.chunks(CHUNK) {
        if chunk.is_empty() {
            continue;
        }
        let inst_keys = aios_core::get_inst_relate_keys(chunk);
        let mut response = SUL_DB
            .query(format!(
                "RETURN array::len(SELECT VALUE id FROM {inst_keys});"
            ))
            .await?
            .check()?;
        let count: Option<usize> = response.take(0)?;
        total += count.unwrap_or_default();
    }
    Ok(total)
}

/// 子树成员边（`room_relate` out 侧）按房间号统计。
async fn member_edges_by_room(scope: &[RefnoEnum]) -> anyhow::Result<BTreeMap<String, usize>> {
    let mut merged: BTreeMap<String, usize> = BTreeMap::new();
    for chunk in scope.chunks(CHUNK) {
        if chunk.is_empty() {
            continue;
        }
        let keys = pe_keys(chunk);
        let rows: Vec<RoomCountRow> = SUL_DB
            .query(format!(
                "SELECT room_num, count() AS c FROM room_relate WHERE out IN [{keys}] GROUP BY room_num;"
            ))
            .await?
            .check()?
            .take(0)?;
        for row in rows {
            *merged.entry(row.room_num.unwrap_or_default()).or_default() += row.c;
        }
    }
    Ok(merged)
}

/// 子树内在册房间（FRMW，名字含房间关键词且末段过命名校验）。
async fn rooms_in_subtree(scope: &[RefnoEnum]) -> anyhow::Result<Vec<(String, RefnoEnum)>> {
    #[derive(Debug, serde::Deserialize)]
    struct Row {
        id: RefnoEnum,
        name: Option<String>,
    }
    let keywords = aios_core::get_db_option().get_room_key_word();
    let mut rooms = Vec::new();
    for chunk in scope.chunks(CHUNK) {
        if chunk.is_empty() {
            continue;
        }
        let keys = pe_keys(chunk);
        let rows: Vec<Row> = SUL_DB
            .query(format!(
                "SELECT id, name FROM [{keys}] WHERE noun = 'FRMW' AND name != NONE;"
            ))
            .await?
            .check()?
            .take(0)?;
        for row in rows {
            let Some(name) = row.name else { continue };
            if !keywords.iter().any(|word| name.contains(word.as_str())) {
                continue;
            }
            let tail = name.rsplit('-').next().unwrap_or_default();
            if match_room_name_hd(tail) {
                rooms.push((name, row.id));
            }
        }
    }
    rooms.sort();
    Ok(rooms)
}

/// 一间房现有的成员边数（经 `room_panel_relate` 找到面板，再数面板的 `room_relate` 出边）。
async fn room_member_edges(room: RefnoEnum) -> anyhow::Result<(usize, usize)> {
    let mut response = SUL_DB
        .query(format!(
            "SELECT VALUE out FROM room_panel_relate WHERE in = {};",
            room.to_pe_key()
        ))
        .await?
        .check()?;
    let panels: Vec<RefnoEnum> = response.take(0)?;
    if panels.is_empty() {
        return Ok((0, 0));
    }
    let keys = pe_keys(&panels);
    let mut response = SUL_DB
        .query(format!(
            "RETURN array::len(SELECT VALUE id FROM room_relate WHERE in IN [{keys}]);"
        ))
        .await?
        .check()?;
    let edges: Option<usize> = response.take(0)?;
    Ok((panels.len(), edges.unwrap_or_default()))
}

async fn room_queue_depth() -> anyhow::Result<usize> {
    let mut response = SUL_DB
        .query(
            "RETURN array::len(SELECT VALUE id FROM pending_model_work \
             WHERE action IN ['room_recalc_element', 'room_recalc_panel']);",
        )
        .await?
        .check()?;
    let count: Option<usize> = response.take(0)?;
    Ok(count.unwrap_or_default())
}

/// 目标自身的归属边：`(面板, 房间号)` 列表。
async fn rooms_of_element(refno: RefnoEnum) -> anyhow::Result<Vec<(String, String)>> {
    #[derive(Debug, serde::Deserialize)]
    struct Row {
        panel_name: Option<String>,
        room_num: Option<String>,
    }
    let mut response = SUL_DB
        .query(format!(
            "SELECT in.name AS panel_name, room_num FROM room_relate WHERE out = {};",
            refno.to_pe_key()
        ))
        .await?
        .check()?;
    let rows: Vec<Row> = response.take(0)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.panel_name.unwrap_or_default(),
                row.room_num.unwrap_or_default(),
            )
        })
        .collect())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut force = false;
    let mut targets: Vec<String> = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--force" => force = true,
            other => targets.push(other.to_string()),
        }
    }
    anyhow::ensure!(
        !targets.is_empty(),
        "usage: node_gen_room_probe <refno>... [--force]"
    );

    aios_core::init_test_surreal().await?;
    let manager = AiosDBManager::init_form_config().await?;
    let project = aios_core::get_db_option().project_name.clone();

    anyhow::ensure!(
        aios_database::data_interface::cata_closure::cata_closure_enabled(),
        "node_gen_room_probe requires AIOS_CATA_CLOSURE_MODE=on"
    );
    println!("CATA-MODE|on");
    for (dbnum, file_name) in BASELINE_DBS {
        let (pe, mut info) = baseline_counts(dbnum).await?;
        if pe > 0 {
            let rebuilt = pe != info;
            if rebuilt {
                aios_database::versioned_db::database::rebuild_dbnum_info_from_pe(
                    dbnum, file_name, "DESI",
                )
                .await?;
                info = baseline_counts(dbnum).await?.1;
            }
            anyhow::ensure!(
                pe == info,
                "dbnum={dbnum} existing baseline incomplete: PE={pe} dbnum_info={info}"
            );
            println!("BASELINE|dbnum={dbnum}|pe={pe}|reused=true|stats_rebuilt={rebuilt}");
            continue;
        }
        match manager
            .initialize_project_dbnum_baseline(&project, dbnum)
            .await
        {
            Ok(parsed) => println!("BASELINE|dbnum={dbnum}|pe={parsed}|reused=false"),
            Err(error) => {
                let (pe, info) = baseline_counts(dbnum).await?;
                if pe > 0 && pe == info && format!("{error:#}").contains("finalize baseline") {
                    println!("BASELINE|dbnum={dbnum}|pe={pe}|reused=false|watermark_pending=true");
                } else {
                    return Err(error);
                }
            }
        }
    }

    // 目标节点身份。
    let mut resolved: Vec<(RefnoEnum, PeInfo)> = Vec::new();
    for target in &targets {
        let refno = RefnoEnum::from(target.as_str());
        match pe_info(refno).await? {
            Some(info) => {
                println!(
                    "TARGET|{}|noun={}|name={}|dbnum={}",
                    refno.to_pdms_str(),
                    info.noun,
                    info.name.as_deref().unwrap_or(""),
                    info.dbnum.map_or_else(|| "?".into(), |d| d.to_string()),
                );
                resolved.push((refno, info));
            }
            None => println!("TARGET-MISSING|{target}"),
        }
    }

    let locator_started = std::time::Instant::now();
    let locator = InMemoryCataLocator::build_for_project(&project).await?;
    println!(
        "LOCATOR|dbnums={}|ref0s={}|elapsed_ms={}",
        locator.dbnum_count(),
        locator.ref0_count(),
        locator_started.elapsed().as_millis()
    );
    for (refno, _) in &resolved {
        let ref0 = refno.refno().get_0();
        let dbnum = locator.dbnum_of_ref0(ref0);
        let db_type = dbnum.and_then(|value| locator.db_type_of(value));
        let file = dbnum.and_then(|value| locator.file_of(value));
        println!(
            "LOCATE|ref0={ref0}|dbnum={}|db_type={}|file={}",
            dbnum.map_or_else(|| "?".into(), |value| value.to_string()),
            db_type.as_deref().unwrap_or("?"),
            file.as_ref()
                .map(|(_, path)| path.display().to_string())
                .unwrap_or_else(|| "?".into())
        );
    }

    // 0) 登记状态（只读扫描）：目标库水位 vs 文件最新会话。
    let mut interesting: Vec<u32> = resolved.iter().filter_map(|(_, info)| info.dbnum).collect();
    interesting.extend(BASELINE_DBS.map(|(dbnum, _)| dbnum));
    interesting.sort_unstable();
    interesting.dedup();
    match manager.dbnum_statuses(&project, None).await {
        Ok(report) => {
            for row in &report.dbnums {
                if interesting.contains(&row.dbnum) {
                    println!(
                        "DBNUM|{}|{}|file_latest={}|applied={}|initialized={}|blocked={}|excluded={}|anomaly={}",
                        row.dbnum,
                        row.db_type,
                        row.file_latest_sesno,
                        row.applied_sesno,
                        row.initialized,
                        row.blocked,
                        row.excluded,
                        row.anomaly
                            .as_ref()
                            .map_or_else(|| "none".into(), |a| format!("{a:?}")),
                    );
                }
            }
            for warning in &report.warnings {
                println!("DBNUM-WARN|{warning}");
            }
        }
        Err(error) => println!("DBNUM-STATUS-FAILED|{error:#}"),
    }

    // 1) 生成：非容器节点走按需生成；容器直接作为定向生成根（与整库全量同一条展开路径）。
    let mut report_scopes: Vec<(RefnoEnum, String, Vec<RefnoEnum>)> = Vec::new();
    for (refno, info) in &resolved {
        if is_coarse_hierarchy_noun(&info.noun) {
            let scope = aios_core::query_deep_children_refnos(*refno).await?;
            let written = written_instances(&scope).await?;
            println!(
                "COVERAGE|{}|subtree={}|with_inst_relate={}",
                refno.to_pdms_str(),
                scope.len(),
                written,
            );
            if written == 0 || force {
                let mut option = aios_core::get_db_option().clone();
                option.gen_model = true;
                option.gen_mesh = true;
                option.debug_refno_types = vec!["CATA".into(), "LOOP".into(), "PRIM".into()];
                option.debug_root_refnos = Some(vec![refno.to_pdms_str()]);
                let cata = aios_database::data_interface::cata_closure::preload_cata_for_roots(
                    &project,
                    &[refno.refno()],
                    None,
                )
                .await?;
                println!(
                    "CATA-PRELOAD|root={}|parsed={}|missing={}",
                    refno.to_pdms_str(),
                    cata.parsed,
                    cata.missing
                );
                println!("GEN-CONTAINER|{}|start", refno.to_pdms_str());
                match aios_database::fast_model::gen_all_geos_data(&option).await {
                    Ok(_) => {
                        let after = written_instances(&scope).await?;
                        println!(
                            "GEN-CONTAINER|{}|done|with_inst_relate={after}",
                            refno.to_pdms_str(),
                        );
                    }
                    Err(error) => {
                        println!("GEN-CONTAINER-FAILED|{}|{error:#}", refno.to_pdms_str())
                    }
                }
            } else {
                println!(
                    "GEN-CONTAINER|{}|skipped（已有 {written} 条实例，--force 可重生成）",
                    refno.to_pdms_str(),
                );
            }
            report_scopes.push((*refno, info.noun.clone(), scope));
            continue;
        }
        match manager.ensure_model_generated(*refno, force).await {
            Ok(result) => {
                println!("ENSURE|{}", serde_json::to_string(&result)?);
                let root = RefnoEnum::from(result.generation_root.as_str());
                let scope = aios_core::query_deep_children_refnos(root).await?;
                report_scopes.push((*refno, info.noun.clone(), scope));
            }
            Err(error) => {
                println!("ENSURE-FAILED|{}|{error:#}", refno.to_pdms_str());
                report_scopes.push((*refno, info.noun.clone(), vec![*refno]));
            }
        }
    }

    // 2) 房间任务：生成阶段按「包围盒确实变了」入队，这里用生产同款消费者收干净。
    // 面板分支的成员候选取自空间树——先从库指针整树重建（批次 worker 房间轮同款顺序钉）。
    if let Err(error) = aios_database::fast_model::aabb_tree::rebuild_tree_from_pointers().await {
        println!("SPATIAL-REBUILD-FAILED|{error:#}");
    }
    let queued = room_queue_depth().await.unwrap_or_default();
    println!("ROOM-QUEUE|before_drain={queued}");
    let consumed = drain_rooms(aios_core::get_db_option()).await?.done;
    let remaining = room_queue_depth().await.unwrap_or_default();
    println!("ROOM-DRAIN|consumed={consumed}|remaining={remaining}");

    // 3) 房间归属汇报。
    for (refno, noun, scope) in &report_scopes {
        if is_coarse_hierarchy_noun(noun) {
            let rooms = rooms_in_subtree(scope).await?;
            let mut total_edges = 0usize;
            let mut lines: Vec<(String, usize, usize)> = Vec::new();
            for (name, room) in &rooms {
                let (panels, edges) = room_member_edges(*room).await?;
                total_edges += edges;
                lines.push((name.clone(), panels, edges));
            }
            println!(
                "ROOMS|{}|rooms={}|member_edges_total={}",
                refno.to_pdms_str(),
                rooms.len(),
                total_edges,
            );
            lines.sort_by(|a, b| b.2.cmp(&a.2));
            for (name, panels, edges) in lines.iter().take(12) {
                println!("ROOM|{name}|panels={panels}|member_edges={edges}");
            }
            continue;
        }
        // 非容器：目标自身归属 + 生成根子树的归属分布。
        let own = rooms_of_element(*refno).await?;
        if own.is_empty() {
            println!("ELEMENT-ROOMS|{}|<none>", refno.to_pdms_str());
        } else {
            for (panel, room_num) in &own {
                println!(
                    "ELEMENT-ROOMS|{}|panel={panel}|room={room_num}",
                    refno.to_pdms_str()
                );
            }
        }
        let by_room = member_edges_by_room(scope).await?;
        let summary = by_room
            .iter()
            .map(|(room, count)| format!("{room}:{count}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "SUBTREE-ROOMS|{}|scope={}|{}",
            refno.to_pdms_str(),
            scope.len(),
            if summary.is_empty() {
                "<no edges>".to_string()
            } else {
                summary
            },
        );
    }

    Ok(())
}
