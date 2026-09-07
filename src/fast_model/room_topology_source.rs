//! Immutable, source-local room topology. Only `capture` reads E3D; publication
//! combines exact source tokens and never consults the current model projection.
use super::room_topology::{RoomGroup, RoomHierarchy, collect_room_groups};
use crate::data_interface::{
    frozen_source::FrozenSource,
    generation_root::{SubtreeElement, refno_from_e3d, subtree_element_from_set},
    model_publish_queue::digest,
};
use aios_core::RefnoEnum;
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use surrealdb::{Surreal, engine::any::Any};

const SCHEMA_VERSION: u16 = 1;
const PART_BYTES: usize = 512 * 1024;
const HEADER: &str = "room_topology_source";
const PART: &str = "room_topology_source_part";

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

pub(crate) fn source_key(token: &str) -> String {
    digest(format!("room-topology-source-v{SCHEMA_VERSION}\n{token}").as_bytes())
}

impl RoomTopologySource {
    pub fn key(&self) -> String {
        source_key(&self.source_token)
    }
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

#[derive(Debug, Deserialize)]
struct SourceHeader {
    source_token: String,
    payload_hash: String,
    part_count: usize,
    complete: bool,
}

async fn query(db: &Surreal<Any>, sql: String) -> anyhow::Result<surrealdb::Response> {
    super::model_db_adapter::publication_request(async { Ok(db.query(sql).await?.check()?) }).await
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

async fn read_header(db: &Surreal<Any>, key: &str) -> anyhow::Result<Option<SourceHeader>> {
    let rows: Vec<SourceHeader> = query(
        db,
        format!("SELECT source_token,payload_hash,part_count,complete FROM {HEADER}:⟨{key}⟩;"),
    )
    .await?
    .take(0)?;
    Ok(rows.into_iter().next())
}

pub(crate) async fn exists_on(db: &Surreal<Any>, token: &str) -> anyhow::Result<bool> {
    let Some(header) = read_header(db, &source_key(token)).await? else {
        return Ok(false);
    };
    ensure!(
        header.source_token == token && header.complete && header.part_count > 0,
        "incomplete or mismatched room topology source"
    );
    Ok(true)
}

/// Producer/upgrade-baseline entry only. Async initialization uses its durable
/// journal artifact instead; its consumer must never invoke this capture path.
pub(crate) async fn prepare_producer_source(source: &FrozenSource) -> anyhow::Result<()> {
    if !source.db_type().eq_ignore_ascii_case("DESI") {
        return Ok(());
    }
    let token = source.commit_token_json()?;
    if exists_on(&aios_core::SUL_DB, &token).await? {
        load_on(&aios_core::SUL_DB, &token).await?;
    } else {
        persist_on(&aios_core::SUL_DB, &capture(source).await?).await?;
    }
    Ok(())
}

fn parts(payload: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut rest = payload;
    while !rest.is_empty() {
        let mut end = rest.len().min(PART_BYTES);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        result.push(&rest[..end]);
        rest = &rest[end..];
    }
    result
}

/// Parts are immutable and individually acknowledged; the complete header is
/// written last. Replaying a partial upload checks every existing part instead
/// of replacing it. No whole-file or whole-database transaction is required.
pub(crate) async fn persist_on(
    db: &Surreal<Any>,
    source: &RoomTopologySource,
) -> anyhow::Result<()> {
    source.validate()?;
    let key = source.key();
    let payload = serde_json::to_string(source)?;
    let hash = digest(payload.as_bytes());
    if let Some(header) = read_header(db, &key).await? {
        ensure!(
            header.complete
                && header.source_token == source.source_token
                && header.payload_hash == hash,
            "immutable room topology source conflicts with existing header"
        );
        load_on(db, &source.source_token).await?;
        return Ok(());
    }
    let parts = parts(&payload);
    for (index, part) in parts.iter().enumerate() {
        let part_hash = digest(part.as_bytes());
        let text = serde_json::to_string(part)?;
        query(db, format!("BEGIN TRANSACTION; LET $old=(SELECT * FROM {PART}:['{key}',{index}])[0]; \
            IF $old IS NONE {{ CREATE {PART}:['{key}',{index}] SET payload={text},payload_hash='{part_hash}' RETURN NONE; }} \
            ELSE {{ IF $old.payload_hash != '{part_hash}' OR $old.payload != {text} {{ THROW 'room_topology_part_conflict'; }}; }}; COMMIT TRANSACTION;"))
            .await?;
    }
    let token = serde_json::to_string(&source.source_token)?;
    let count = parts.len();
    query(db, format!("BEGIN TRANSACTION; LET $old=(SELECT * FROM {HEADER}:⟨{key}⟩)[0]; \
        IF $old IS NONE {{ CREATE {HEADER}:⟨{key}⟩ SET source_token={token},payload_hash='{hash}',part_count={count},complete=true RETURN NONE; }} \
        ELSE {{ IF $old.source_token != {token} OR $old.payload_hash != '{hash}' OR $old.part_count != {count} OR $old.complete != true {{ THROW 'room_topology_source_conflict'; }}; }}; COMMIT TRANSACTION;"))
        .await?;
    Ok(())
}

pub(crate) async fn load_on(db: &Surreal<Any>, token: &str) -> anyhow::Result<RoomTopologySource> {
    let key = source_key(token);
    let header = read_header(db, &key)
        .await?
        .context("missing exact room topology source; retain publication without reopening E3D")?;
    ensure!(
        header.complete && header.source_token == token && header.part_count > 0,
        "room topology source identity or completion mismatch"
    );
    #[derive(Deserialize)]
    struct Part {
        payload: String,
        payload_hash: String,
    }
    let mut payload = String::new();
    for index in 0..header.part_count {
        let rows: Vec<Part> = query(
            db,
            format!("SELECT payload,payload_hash FROM {PART}:['{key}',{index}];"),
        )
        .await?
        .take(0)?;
        let part = rows
            .into_iter()
            .next()
            .context("missing room topology source part")?;
        ensure!(
            digest(part.payload.as_bytes()) == part.payload_hash,
            "room topology source part checksum mismatch"
        );
        payload.push_str(&part.payload);
    }
    ensure!(
        digest(payload.as_bytes()) == header.payload_hash,
        "room topology source checksum mismatch"
    );
    let source: RoomTopologySource = serde_json::from_str(&payload)?;
    source.validate()?;
    ensure!(
        source.source_token == token,
        "room topology payload token mismatch"
    );
    Ok(source)
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
        assert_ne!(old[2].key(), next[2].key());
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
    #[test]
    fn multipart_payload_preserves_unicode_boundaries() {
        let payload = "门\"\\\n".repeat(PART_BYTES);
        let split = parts(&payload);
        assert!(split.iter().all(|s| s.len() <= PART_BYTES));
        assert_eq!(split.join(""), payload);
    }
    async fn db() -> Surreal<Any> {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("topology").use_db("test").await.unwrap();
        db
    }
    #[tokio::test]
    async fn exact_source_roundtrip_never_opens_missing_source_and_detects_corruption() {
        let db = db().await;
        let source = cross_sources().remove(1);
        assert!(
            !FrozenSource::from_commit_token_json(&source.source_token)
                .unwrap()
                .path()
                .exists()
        );
        persist_on(&db, &source).await.unwrap();
        persist_on(&db, &source).await.unwrap();
        let loaded = load_on(&db, &source.source_token).await.unwrap();
        assert_eq!(
            serde_json::to_vec(&source).unwrap(),
            serde_json::to_vec(&loaded).unwrap()
        );
        assert!(
            load_on(&db, &fixture_source(2, 2).source_token)
                .await
                .is_err()
        );
        db.query(format!(
            "UPDATE {PART}:['{}',0] SET payload_hash='corrupt';",
            source.key()
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
        assert!(load_on(&db, &source.source_token).await.is_err());
        assert!(
            persist_on(&db, &source).await.is_err(),
            "do not overwrite corrupt retained content"
        );
    }
    #[tokio::test]
    async fn failed_part_upload_has_no_complete_header_and_explicit_replay_is_idempotent() {
        let db = db().await;
        let mut source = cross_sources().remove(1);
        source.nodes[0].name = "R".repeat(PART_BYTES * 2);
        let key = source.key();
        db.query(format!("DEFINE EVENT reject_second ON TABLE {PART} WHEN $after.id={PART}:['{key}',1] THEN {{ THROW 'injected topology upload'; }};"))
            .await.unwrap().check().unwrap();
        assert!(persist_on(&db, &source).await.is_err());
        assert!(!exists_on(&db, &source.source_token).await.unwrap());
        db.query(format!("REMOVE EVENT reject_second ON TABLE {PART};"))
            .await
            .unwrap()
            .check()
            .unwrap();
        persist_on(&db, &source).await.unwrap();
        assert_eq!(
            load_on(&db, &source.source_token).await.unwrap().nodes[0]
                .name
                .len(),
            PART_BYTES * 2
        );
    }
    #[tokio::test]
    async fn ordinary_round_cannot_promote_without_its_complete_exact_source() {
        use crate::data_interface::side_effect_pending::SideEffectCompensator;
        let db = db().await;
        let source = fixture_source(7997, 1);
        let token = serde_json::to_string(&source.source_token).unwrap();
        let id = "incr_side_effect_pending:spatial_reconcile_7997_112";
        db.query(format!("CREATE {id} SET kind='spatial_reconcile',dbnum=7997,end_sesno=112,db_type='DESI',commit_token={token},status='collecting';"))
            .await.unwrap().check().unwrap();
        assert!(
            SideEffectCompensator::promote_file_round_on(&db, 7997, 112)
                .await
                .is_err()
        );
        let states: Vec<String> = db
            .query(format!("SELECT VALUE status FROM {id};"))
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(states, vec!["collecting"]);
        persist_on(&db, &source).await.unwrap();
        SideEffectCompensator::promote_file_round_on(&db, 7997, 112)
            .await
            .unwrap();
        let states: Vec<String> = db
            .query(format!("SELECT VALUE status FROM {id};"))
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(states, vec!["pending"]);
    }
}
