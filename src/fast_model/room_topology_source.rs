//! Immutable, source-local room topology. Only `capture` reads E3D; composition
//! combines exact source tokens and never consults the current model projection.
//!
//! 分片**不进库**（ADR-072 D4 / d-441 ⑤）：一片 = 一个 DESI 文件在一个冻结令牌下的房间树
//! 节点表，落成本地磁盘工件（`model_artifact` 信封：校验和 + 键逐字节比对），键按 d-89 只由
//! **本文件的冻结令牌**寻址、不含整组文件的 composite——邻库前进不作废没变文件的分片。
//! 启动参照（`room_resident::ResidentReference`）按令牌读回这些分片合成「旧」视图；找不到
//! 就是 `ReferenceUnreadable`，走显式全量（多算不漏算）。此前这一层是两张表
//! （头 + 512 KiB 分块）加发布串行闸写入，随 publish 阶段一起退役。
use super::room_topology::{RoomGroup, RoomHierarchy, collect_room_groups};
use crate::data_interface::{
    digest::digest,
    frozen_source::FrozenSource,
    generation_root::{SubtreeElement, refno_from_e3d, subtree_element_from_set},
};
use aios_core::RefnoEnum;
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: u16 = 1;
/// 磁盘工件目录名（在 `meshes_path` 之下，与 `room-input-cache` / `room-result-cache` 并排）。
const ARTIFACT_DIR: &str = "room-topology-source";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TopologyNode {
    pub refno: RefnoEnum,
    pub noun: String,
    pub name: String,
    pub members: Vec<RefnoEnum>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RoomTopologySource {
    pub version: u16,
    pub dbnum: u32,
    pub source_sesno: u32,
    pub source_token: String,
    pub roots: Vec<RefnoEnum>,
    pub nodes: Vec<TopologyNode>,
}

/// 分片身份摘要。**过渡件**：库里两张分片表退役后它已没有生产读者，只剩
/// `side_effect_pending` 退役前的一处引用；随 T4 删该文件一起删。
pub(crate) fn source_key(token: &str) -> String {
    digest(format!("room-topology-source-v{SCHEMA_VERSION}\n{token}").as_bytes())
}

/// 磁盘工件键：按**本文件**的冻结令牌原文寻址（d-89），不含 composite、不另算摘要——
/// 工件信封自己把键哈希成文件名、装载时逐字节比对（`model_artifact::path` / `load_value`）。
pub(crate) fn artifact_key(token: &str) -> String {
    format!("room-topology-source-v{SCHEMA_VERSION}:{token}")
}

/// 缺省工件目录：`meshes_path/room-topology-source`。
pub(crate) fn artifact_dir() -> PathBuf {
    aios_core::get_db_option()
        .get_meshes_path()
        .join(ARTIFACT_DIR)
}

impl RoomTopologySource {
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.version == SCHEMA_VERSION,
            "unsupported room topology source version"
        );
        let source = FrozenSource::from_commit_token_json(&self.source_token)?;
        ensure!(
            source.db_type().eq_ignore_ascii_case("DESI")
                && source.dbnum() as u32 == self.dbnum
                && source.target_sesno() == self.source_sesno,
            "room topology source token mismatch"
        );
        let mut own = std::collections::HashSet::new();
        for node in &self.nodes {
            ensure!(
                own.insert(node.refno),
                "duplicate node in room topology source"
            );
        }
        ensure!(
            self.roots.iter().all(|root| own.contains(root)),
            "room topology source root is missing"
        );
        Ok(())
    }
}

/// Physical source membership comes from its own index. In particular, do not
/// recursively copy foreign members into this shard: their version is selected
/// later by the published composite manifest.
pub(crate) async fn capture(source: &FrozenSource) -> anyhow::Result<RoomTopologySource> {
    let started = std::time::Instant::now();
    let source = source.reopen_verified()?;
    ensure!(
        source.db_type().eq_ignore_ascii_case("DESI"),
        "room topology requires a DESI source"
    );
    let service = super::e3d_model_service::E3dModelService::from_frozen_sources(
        std::slice::from_ref(&source),
        true,
    )
    .await?;
    let set = service.build_set(0, None)?;
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<RoomTopologySource> {
        let index =
            super::e3d_model_service::scan_index(source.path(), Some(source.target_sesno()))?;
        let mut nodes = Vec::with_capacity(index.owners.len());
        for &(word0, word1) in index.owners.keys() {
            let refno = refno_from_e3d(e3d_io::refno::RefNo::new(word0, word1));
            let element = subtree_element_from_set(&set, refno)?;
            let is_room = matches!(
                element.noun.trim().to_ascii_uppercase().as_str(),
                "FRMW" | "SBFR"
            );
            nodes.push(TopologyNode {
                refno,
                noun: element.noun,
                name: if is_room { element.name } else { String::new() },
                members: element.members,
            });
        }
        source.reopen_verified()?;
        Ok(RoomTopologySource {
            version: SCHEMA_VERSION,
            dbnum: source.dbnum() as u32,
            source_sesno: source.target_sesno(),
            source_token: source.commit_token_json()?,
            roots: index.roots.into_iter().map(refno_from_e3d).collect(),
            nodes,
        })
    })
    .await
    .context("capture room topology source task")??;
    result.validate()?;
    println!(
        "[room-topology] frozen dbnum={} nodes={} elapsed_ms={}",
        result.dbnum,
        result.nodes.len(),
        started.elapsed().as_millis()
    );
    Ok(result)
}

pub(crate) fn context_fingerprint() -> String {
    digest(
        format!(
            "room-context-v1|{:?}|{:?}",
            RoomHierarchy::configured(),
            aios_core::get_db_option().get_room_key_word()
        )
        .as_bytes(),
    )
}

/// 这个令牌的分片在不在磁盘上（在、且校验通过、且令牌逐字相等）。
///
/// 信封校验不过（内容被改 / 键不对）按「不在」算——参照那一侧据此判 `ReferenceUnreadable`
/// 走全量，与此前库里「头不完整」同一去向。
pub(crate) fn exists_in(dir: &Path, token: &str) -> bool {
    load_from(dir, token).is_ok()
}

pub(crate) fn exists(token: &str) -> bool {
    exists_in(&artifact_dir(), token)
}

/// 读回一片：信封校验 + 结构自检 + 令牌逐字相等，任一不过整片 `Err`（原则 2：判不了就不用）。
pub(crate) fn load_from(dir: &Path, token: &str) -> anyhow::Result<RoomTopologySource> {
    let source: RoomTopologySource =
        super::model_artifact::load_value(dir, &artifact_key(token)).with_context(|| {
            format!(
                "missing exact room topology source artifact in {}; retain the reference without reopening E3D",
                dir.display()
            )
        })?;
    source.validate()?;
    ensure!(
        source.source_token == token,
        "room topology payload token mismatch"
    );
    Ok(source)
}

pub(crate) fn load(token: &str) -> anyhow::Result<RoomTopologySource> {
    load_from(&artifact_dir(), token)
}

/// 落一片：`model_artifact::store_value`（临时文件 + fsync + 原子改名）。已有同令牌的分片时
/// 只核对不重写——分片是不可变的，内容不同说明同一令牌下捕获出了两份不同的拓扑，那是
/// 上游的 bug，不能静默盖掉。
pub(crate) fn persist_in(dir: &Path, source: &RoomTopologySource) -> anyhow::Result<()> {
    source.validate()?;
    let payload = serde_json::to_vec(source)?;
    if let Some(existing) =
        super::model_artifact::load_value::<RoomTopologySource>(dir, &artifact_key(&source.source_token))
    {
        ensure!(
            serde_json::to_vec(&existing)? == payload,
            "immutable room topology source conflicts with the retained artifact"
        );
        return Ok(());
    }
    super::model_artifact::store_value(dir, &artifact_key(&source.source_token), source)
}

pub(crate) fn persist(source: &RoomTopologySource) -> anyhow::Result<()> {
    persist_in(&artifact_dir(), source)
}

/// 把一个冻结源的分片备好：磁盘上已有就只校验（不重开 E3D），没有才 `capture` 再落盘。
/// 只在结构触发命中 / 本库首次（参照缺这一片）时才该走到 `capture` 那一支——调用方管这道门。
/// 不写库、不写参照。
pub(crate) async fn prepare_producer_source(source: &FrozenSource) -> anyhow::Result<()> {
    prepare_producer_source_in(&artifact_dir(), source).await
}

pub(crate) async fn prepare_producer_source_in(
    dir: &Path,
    source: &FrozenSource,
) -> anyhow::Result<()> {
    if !source.db_type().eq_ignore_ascii_case("DESI") {
        return Ok(());
    }
    let token = source.commit_token_json()?;
    if exists_in(dir, &token) {
        return Ok(());
    }
    persist_in(dir, &capture(source).await?)
}

pub(crate) fn compose(
    sources: &[RoomTopologySource],
    hierarchy: RoomHierarchy,
) -> anyhow::Result<Vec<(u32, Vec<RoomGroup>)>> {
    let mut lookup = HashMap::new();
    let mut dbnums = std::collections::HashSet::new();
    for source in sources {
        ensure!(
            dbnums.insert(source.dbnum),
            "duplicate dbnum in room topology composite"
        );
        for node in &source.nodes {
            ensure!(
                lookup.insert(node.refno, node).is_none(),
                "duplicate physical RefNo across room topology sources: {}",
                node.refno
            );
        }
    }
    sources
        .iter()
        .map(|source| {
            let groups = collect_room_groups(&source.roots, hierarchy, |refno| {
                let node = lookup
                    .get(&refno)
                    .with_context(|| format!("missing exact room topology node {refno}"))?;
                Ok(SubtreeElement {
                    noun: node.noun.clone(),
                    name: node.name.clone(),
                    members: node.members.clone(),
                })
            })?;
            Ok((source.dbnum, groups))
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn fixture_source(dbnum: u32, generation: u64) -> RoomTopologySource {
    #[cfg(windows)]
    let file_id = serde_json::json!({"Windows":{"volume_serial":1,"file_index":generation}});
    #[cfg(unix)]
    let file_id = serde_json::json!({"Unix":{"device":1,"inode":generation}});
    let source_token =
        serde_json::json!({"version":1,"canonical_path":format!("/missing-test-source-{dbnum}"),
        "file_id":file_id,"dbnum":dbnum,"db_type":"DESI","target_sesno":112,"latest_sesno":112,
        "opened_len":2048,"header_prefix_hex":"", "anchor":{"session_page":{"extent":0,"page":1},
        "index_root":{"extent":0,"page":2},"session_page_sha256":"00","index_root_sha256":"00"}})
        .to_string();
    RoomTopologySource {
        version: SCHEMA_VERSION,
        dbnum,
        source_sesno: 112,
        source_token,
        roots: vec![],
        nodes: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn r(db: u32, id: u32) -> RefnoEnum {
        refno_from_e3d(e3d_io::refno::RefNo::new(db, id))
    }
    fn node(id: RefnoEnum, noun: &str, name: &str, members: Vec<RefnoEnum>) -> TopologyNode {
        TopologyNode {
            refno: id,
            noun: noun.into(),
            name: name.into(),
            members,
        }
    }
    fn cross_sources() -> Vec<RoomTopologySource> {
        let mut a = fixture_source(1, 1);
        a.roots = vec![r(1, 1)];
        a.nodes = vec![node(r(1, 1), "WORL", "", vec![r(2, 1)])];
        let mut b = fixture_source(2, 1);
        b.roots = vec![r(2, 1)];
        b.nodes = vec![node(
            r(2, 1),
            "FRMW",
            "ROOM-001",
            vec![r(3, 2), r(3, 1), r(3, 2)],
        )];
        let mut c = fixture_source(3, 1);
        c.roots = vec![r(3, 1), r(3, 2)];
        c.nodes = vec![
            node(r(3, 1), "PANE", "", vec![]),
            node(r(3, 2), "PANE", "", vec![]),
        ];
        vec![a, b, c]
    }
    #[test]
    fn composite_discovers_rooms_across_files_and_preserves_source_and_member_order() {
        let source = cross_sources();
        let bytes = serde_json::to_vec(&source).unwrap();
        let decoded: Vec<RoomTopologySource> = serde_json::from_slice(&bytes).unwrap();
        let groups = compose(&decoded, RoomHierarchy::Hd).unwrap();
        assert_eq!(
            groups.iter().map(|x| x.0).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(groups[0].1, groups[1].1);
        assert_eq!(groups[0].1[0].panels, vec![r(3, 2), r(3, 1)]);
        assert!(groups[2].1.is_empty());
        assert_eq!(source[2].roots, decoded[2].roots);
    }
    #[test]
    fn unchanged_source_groups_change_with_the_exact_other_source_token() {
        let old = cross_sources();
        let mut next = old.clone();
        next[2].source_token = fixture_source(3, 2).source_token;
        next[2].nodes[0].noun = "BOX".into();
        assert_eq!(old[0].source_token, next[0].source_token);
        assert_eq!(old[2].source_sesno, next[2].source_sesno);
        // 分片身份 = 文件令牌本身（`source_key`）。`RoomTopologySource::key()` 随库里的
        // 分片表一起退役：那把键是库记录 id，而分片现在只按令牌寻址成磁盘工件
        // （`the_shard_artifact_is_addressed_by_the_file_token_alone`）。
        assert_ne!(
            source_key(&old[2].source_token),
            source_key(&next[2].source_token)
        );
        assert_ne!(
            compose(&old, RoomHierarchy::Hd).unwrap()[0].1,
            compose(&next, RoomHierarchy::Hd).unwrap()[0].1
        );
    }
    #[test]
    fn missing_and_ambiguous_cross_file_nodes_fail_closed() {
        let mut source = cross_sources();
        assert!(compose(&source[..2], RoomHierarchy::Hd).is_err());
        let duplicate = source[1].nodes[0].clone();
        source[2].nodes.push(duplicate);
        assert!(compose(&source, RoomHierarchy::Hd).is_err());
    }
    /// 磁盘分片的往返：落一次、再落一次幂等、读回逐字节相等；没落过的令牌读不到；
    /// 工件被改（信封校验不过）→ 读不到，**也不许**被同令牌的重写盖掉。一次都不打开
    /// E3D 文件（夹具令牌指向不存在的路径）。
    #[test]
    fn exact_source_roundtrip_on_disk_never_opens_missing_source_and_detects_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let source = cross_sources().remove(1);
        assert!(
            !FrozenSource::from_commit_token_json(&source.source_token)
                .unwrap()
                .path()
                .exists()
        );
        assert!(!exists_in(dir.path(), &source.source_token));
        persist_in(dir.path(), &source).unwrap();
        persist_in(dir.path(), &source).unwrap();
        assert!(exists_in(dir.path(), &source.source_token));
        let loaded = load_from(dir.path(), &source.source_token).unwrap();
        assert_eq!(
            serde_json::to_vec(&source).unwrap(),
            serde_json::to_vec(&loaded).unwrap()
        );
        assert!(load_from(dir.path(), &fixture_source(2, 2).source_token).is_err());

        // 同令牌、不同内容：不可变分片不许被盖。
        let mut conflicting = source.clone();
        conflicting.nodes[0].name = "ROOM-999".into();
        assert!(
            persist_in(dir.path(), &conflicting).is_err(),
            "同一令牌下两份不同拓扑是上游 bug，不能静默覆盖"
        );

        // 工件损坏：信封校验不过 → 读不到、按缺片处置。
        let mut corrupted = None;
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "json") {
                let mut text = std::fs::read_to_string(&path).unwrap();
                text = text.replace("ROOM-001", "ROOM-XXX");
                std::fs::write(&path, text).unwrap();
                corrupted = Some(path);
            }
        }
        assert!(corrupted.is_some(), "夹具至少落了一份工件");
        assert!(load_from(dir.path(), &source.source_token).is_err());
        assert!(!exists_in(dir.path(), &source.source_token));
    }

    /// 分片工件键按**本文件令牌**寻址、不含 composite（d-89）：两个不同文件版本的令牌各有
    /// 各的工件，互不遮蔽；键里带着令牌原文、不另算摘要。
    #[test]
    fn the_shard_artifact_is_addressed_by_the_file_token_alone() {
        let a = fixture_source(7997, 1);
        let b = fixture_source(7997, 2);
        assert_ne!(a.source_token, b.source_token);
        assert_ne!(artifact_key(&a.source_token), artifact_key(&b.source_token));
        assert!(artifact_key(&a.source_token).ends_with(&a.source_token));
        assert!(!artifact_key(&a.source_token).contains("composite"));

        let dir = tempfile::tempdir().unwrap();
        persist_in(dir.path(), &a).unwrap();
        assert!(exists_in(dir.path(), &a.source_token));
        assert!(!exists_in(dir.path(), &b.source_token));
        persist_in(dir.path(), &b).unwrap();
        assert!(exists_in(dir.path(), &a.source_token));
        assert!(exists_in(dir.path(), &b.source_token));
    }

    /// 这一层从此不碰库：没有 `SUL_DB`、没有 `.query(`、没有分片表名。
    #[test]
    fn room_topology_shards_never_touch_the_database() {
        let source = include_str!("room_topology_source.rs");
        let production = source.split_once("#[cfg(test)]").expect("test boundary").0;
        for needle in ["SUL_DB", ".query(", "surrealdb", "publication_request", "_part"] {
            assert!(
                !production.contains(needle),
                "room_topology_source.rs 生产路径不该再出现 `{needle}`"
            );
        }
        assert!(production.contains("model_artifact::store_value"));
        assert!(production.contains("model_artifact::load_value"));
    }
    /// 真文件门：在带房间的库上量一次 `capture` 的节点数、分片体量与耗时（常驻房间模型计划
    /// P0 的取证，`docs/plans/2026-09-07-resident-room-model-plan.md` §7）。默认 ams7997——
    /// 124 间房那个库；`AIOS_ROOM_TOPOLOGY_LIVE_FILE` 可换。只钉「合成得出、房间名下有面板」，
    /// 数字打印给人看。
    #[tokio::test]
    #[ignore = "manual live: needs a real DESI file listed in the current MDB and the E3D template directory"]
    async fn live_capture_reports_node_count_payload_and_elapsed_on_a_room_bearing_database() {
        let file = std::path::PathBuf::from(
            std::env::var("AIOS_ROOM_TOPOLOGY_LIVE_FILE").unwrap_or_else(|_| {
                r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7997_0001".into()
            }),
        );
        let source =
            FrozenSource::capture(&file).expect("freeze the DESI file at its latest session");
        let started = std::time::Instant::now();
        let captured = capture(&source)
            .await
            .expect("capture room topology source");
        let capture_elapsed = started.elapsed();
        let payload = serde_json::to_string(&captured).unwrap();
        let room_nodes = captured
            .nodes
            .iter()
            .filter(|node| {
                matches!(
                    node.noun.trim().to_ascii_uppercase().as_str(),
                    "FRMW" | "SBFR"
                )
            })
            .count();
        let keyword_rooms = captured
            .nodes
            .iter()
            .filter(|node| node.name.contains("-RM"))
            .count();
        println!(
            // `parts=` 那一格随发布视图的分片切分一起退役：工件现在整份按令牌落盘。
            "[live] {} dbnum={} sesno={} nodes={} room_nodes={} named_-RM={} payload_bytes={} capture={:?}",
            file.display(),
            captured.dbnum,
            captured.source_sesno,
            captured.nodes.len(),
            room_nodes,
            keyword_rooms,
            payload.len(),
            capture_elapsed
        );
        let started = std::time::Instant::now();
        let composed =
            compose(std::slice::from_ref(&captured), RoomHierarchy::Hd).expect("compose");
        let groups = &composed[0].1;
        let filtered = super::super::room_topology::room_panel_groups(groups, &["-RM".to_string()]);
        println!(
            "[live] compose: {} FRMW group(s) / {} panel(s); keyword -RM: {} room(s) / {} panel(s); compose={:?}",
            groups.len(),
            groups.iter().map(|group| group.panels.len()).sum::<usize>(),
            filtered.len(),
            filtered
                .iter()
                .map(|(_, _, panels)| panels.len())
                .sum::<usize>(),
            started.elapsed()
        );
        assert!(
            filtered.iter().any(|(_, _, panels)| !panels.is_empty()),
            "带房间的库上 -RM 房间名下该有面板"
        );
    }
}
