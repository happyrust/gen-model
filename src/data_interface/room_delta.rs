//! Copy-on-write room results. Unchanged members reference immutable fragments;
//! a single watermark CAS publishes the new directory and room completion.
use std::collections::{BTreeMap, BTreeSet};
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use surrealdb::{Surreal, engine::any::Any, sql::Thing};
use super::{dbnum_state::RoomInputVersion, digest::digest};
use crate::fast_model::room_model::ElementRoomEdge;

const PAGE_SIZE: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Edge {
    pub panel: String,
    pub room_num: String,
    pub inside_count: u8,
    pub center_dist: f32,
}

/// Persisted locally alongside model output, never reconstructed from database
/// memberships. The database stores the same directory for result readers only.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Directory {
    pub version: Option<String>,
    pub input: Option<RoomInputVersion>,
    pub members: BTreeMap<String, Vec<Edge>>,
    pub fragments: BTreeMap<String, String>,
    pub buckets: BTreeMap<String, Vec<String>>,
    /// Local reverse index. Legacy cache artifacts rebuild it once; result
    /// readers continue using the unchanged database directory format.
    #[serde(default)]
    bucket_members: Option<BTreeMap<String, BTreeSet<String>>>,
}

pub(crate) struct PreparedDelta {
    pub next: Directory,
    dbnum: u32,
    fragments: Vec<(String, Vec<Edge>)>,
    pages: Vec<(String, BTreeMap<String, String>)>,
    directory_members_visited: usize,
}

pub(crate) fn bucket(member: &str) -> String { digest(member.as_bytes())[..2].to_owned() }

impl PreparedDelta {
    /// `previous` 是调用方按登记里的上一版版本号从磁盘工件装回来的目录（装不到就是缺省的
    /// 空目录）；此前还带一个 `expected`（水位行上的 `room_result_version`）在这里比对基线，
    /// 那是对手 CAS 的一半，随 ADR-072 D4 一起退——同库单写者，目录就是按登记装的那一份。
    pub(crate) fn new(dbnum: u32, previous: Directory, input: RoomInputVersion,
        changed: &BTreeSet<String>, edges: &[ElementRoomEdge]) -> anyhow::Result<Self> {
        ensure!(input.is_valid(), "invalid incremental room input");
        let mut replacements: BTreeMap<String, Vec<Edge>> = changed.iter().map(|member| (member.clone(), Vec::new())).collect();
        for edge in edges {
            let member = edge.member.refno.to_string();
            ensure!(edge.member.inside_count <= 8 && edge.member.center_dist.is_finite() && edge.member.center_dist >= 0.0, "invalid room edge");
            replacements.get_mut(&member).context("room edge is outside change set")?.push(Edge {
                panel: edge.panel.to_string(), room_num: edge.room_num.clone(),
                inside_count: edge.member.inside_count, center_dist: edge.member.center_dist,
            });
        }
        // The caller transfers its privately loaded directory: no full clone of
        // every unchanged member and edge for each incremental Task.
        let mut next = previous;
        let mut directory_members_visited = 0;
        if next.bucket_members.is_none() {
            let mut index: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for member in next.fragments.keys() {
                directory_members_visited += 1;
                index.entry(bucket(member)).or_default().insert(member.clone());
            }
            next.bucket_members = Some(index);
        }
        let mut fragments = Vec::new();
        let mut dirty_buckets = BTreeSet::new();
        for (member, mut rows) in replacements {
            rows.sort_by(|a,b| a.panel.cmp(&b.panel));
            ensure!(!rows.windows(2).any(|pair| pair[0].panel == pair[1].panel), "duplicate member panel");
            if next.members.get(&member) == Some(&rows) { continue; }
            let fragment = digest(&serde_json::to_vec(&(dbnum, &member, &rows))?);
            let key = bucket(&member);
            dirty_buckets.insert(key.clone());
            next.bucket_members.as_mut().unwrap().entry(key).or_default().insert(member.clone());
            next.fragments.insert(member.clone(), fragment.clone());
            next.members.insert(member, rows.clone());
            fragments.push((fragment, rows));
        }
        let mut pages = Vec::new();
        for key in dirty_buckets {
            let members: Vec<_> = next.bucket_members.as_ref().unwrap()[&key].iter().collect();
            directory_members_visited += members.len();
            let existing_pages: BTreeSet<_> = next.buckets.get(&key).into_iter().flatten().collect();
            let mut page_ids = Vec::new();
            for chunk in members.chunks(PAGE_SIZE) {
                let page: BTreeMap<String,String> = chunk.iter().map(|member| {
                    ((*member).clone(), next.fragments[*member].clone())
                }).collect();
                let id = digest(&serde_json::to_vec(&(dbnum, &page))?);
                page_ids.push(id.clone());
                if !existing_pages.contains(&id) { pages.push((id,page)); }
            }
            next.buckets.insert(key, page_ids);
        }
        let version = format!("delta2_{}", digest(&serde_json::to_vec(&(dbnum, &input, &next.buckets))?));
        next.version = Some(version);
        next.input = Some(input);
        Ok(Self { next, dbnum, fragments, pages, directory_members_visited })
    }

    pub(crate) fn edge_writes(&self) -> usize { self.fragments.iter().map(|(_,edges)| edges.len()).sum() }
    pub(crate) fn directory_members_visited(&self) -> usize { self.directory_members_visited }

    pub(crate) async fn stage_on(&self, db: &Surreal<Any>) -> anyhow::Result<()> {
        for (id, edges) in &self.fragments {
            let mut parts = Vec::new();
            for (index, chunk) in edges.chunks(PAGE_SIZE).enumerate() {
                let part = format!("{id}_{index}");
                db.query("UPSERT $id SET edges=$edges;")
                    .bind(("id", Thing::from(("room_fragment_part", part.as_str()))))
                    .bind(("edges", chunk.to_vec())).await?.check()?;
                parts.push(part);
            }
            db.query("UPSERT $id SET parts=$parts, complete=true;")
                .bind(("id", Thing::from(("room_fragment", id.as_str()))))
                .bind(("parts", parts)).await?.check()?;
        }
        for (id, members) in &self.pages {
            db.query("UPSERT $id SET members=$members;")
                .bind(("id", Thing::from(("room_directory_page",id.as_str()))))
                .bind(("members", members.clone())).await?.check()?;
        }
        db.query("UPSERT $id SET buckets=$buckets, complete=true;")
            .bind(("id",Thing::from(("room_directory",self.next.version.as_deref().unwrap()))))
            .bind(("buckets",self.next.buckets.clone())).await?.check()?;
        Ok(())
    }

    /// 对手 CAS（两道水位、房间水位单调性、义务比对、基线版本比对）随 ADR-072 D4 退役
    /// ——同库单写者，没有「另一个任务换了义务 / 挪了基线」这回事，门已经前移到
    /// `room_task` 那道 `TaskModelOutcome`。**留下的那一道不是 CAS 而是自检**：目录齐不齐
    /// 问的是这一笔自己刚 stage 的东西，答错了是本地 bug，不是并发。
    pub(crate) async fn commit_on(&self, db: &Surreal<Any>) -> anyhow::Result<()> {
        db.query("BEGIN TRANSACTION;
            IF $directory.complete != true { THROW 'room delta staging incomplete'; };
            UPDATE $watermark SET room_result_version=$version,room_input_version=$input,
                room_sesno_time=model_sesno_time,room_sesno=$input.model_sesno,room_settled_at=time::now(),
                state_revision=(state_revision ?? 0)+1;
            COMMIT TRANSACTION;")
            .bind(("watermark", Thing::from(("dbnum_watermark", surrealdb::sql::Id::Number(self.dbnum as i64)))))
            .bind(("input",self.next.input.clone().unwrap()))
            .bind(("version",self.next.version.clone().unwrap()))
            .bind(("directory",Thing::from(("room_directory",self.next.version.as_deref().unwrap()))))
            .await?.check().context("commit incremental room result")?;
        super::dbnum_state::DbnumState::invalidate_mirrored_on(db,self.dbnum);
        super::dbnum_consistency::invalidate_census();
        Ok(())
    }
}

/// Only result presentation calls this. Calculation takes the local Directory.
pub(crate) async fn read_member_on(db: &Surreal<Any>, dbnum: u32, member: &str) -> anyhow::Result<Vec<Edge>> {
    // One query pins the visible directory before dereferencing immutable pages.
    let mut response = db.query("LET $version=$watermark.room_result_version;
        LET $directory=type::thing('room_directory',$version);
        LET $pages=$directory.buckets[$bucket] ?? [];
        LET $fragments=(SELECT VALUE members[$member] FROM (array::map($pages, |$p| type::thing('room_directory_page',$p))));
        LET $parts=array::flatten((SELECT VALUE parts FROM (array::map($fragments[WHERE $this != NONE], |$f| type::thing('room_fragment',$f)))));
        RETURN array::flatten((SELECT VALUE edges FROM (array::map($parts, |$p| type::thing('room_fragment_part',$p)))));")
        .bind(("watermark", Thing::from(("dbnum_watermark",surrealdb::sql::Id::Number(dbnum as i64)))))
        .bind(("bucket",bucket(member))).bind(("member",member.to_owned())).await?.check()?;
    Ok(response.take(5)?)
}

/// Compatible output reader pins the selected format/version inside one query.
pub(crate) async fn read_member_compatible_on(db: &Surreal<Any>, dbnum: u32, member: &str) -> anyhow::Result<Vec<Edge>> {
    let mut response = db.query("LET $version=$watermark.room_result_version;
        RETURN IF string::starts_with($version ?? '', 'delta2_') {
            LET $directory=type::thing('room_directory',$version);
            LET $pages=$directory.buckets[$bucket] ?? [];
            LET $fragments=(SELECT VALUE members[$member] FROM (array::map($pages, |$p| type::thing('room_directory_page',$p))));
            LET $parts=array::flatten((SELECT VALUE parts FROM (array::map($fragments[WHERE $this != NONE], |$f| type::thing('room_fragment',$f)))));
            array::flatten((SELECT VALUE edges FROM (array::map($parts, |$p| type::thing('room_fragment_part',$p)))))
        } ELSE {
            (SELECT panel,room_num,inside_count ?? 0 AS inside_count,center_dist ?? 0.0 AS center_dist FROM room_result
            WHERE dbnum=$dbnum AND result_version=$version AND member=$member)
        };")
        .bind(("watermark",Thing::from(("dbnum_watermark",surrealdb::sql::Id::Number(dbnum as i64)))))
        .bind(("dbnum",dbnum)).bind(("bucket",bucket(member))).bind(("member",member.to_owned()))
        .await?.check()?;
    Ok(response.take(1)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::room_model::RoomMember;

    fn input() -> RoomInputVersion {
        RoomInputVersion { version: 1, source_identity: "source".into(), model_sesno: 12,
            model_version: "model".into(), room_definition_version: "rooms".into(),
            generation_rule_version: "rules".into() }
    }
    fn edge(member: u32) -> ElementRoomEdge {
        ElementRoomEdge { panel: aios_core::RefU64::from_two_nums(8000, 99).into(),
            room_num: "R1".into(), member: RoomMember {
                refno: aios_core::RefU64::from_two_nums(8000, member).into(),
                inside_count: 8, center_dist: 1.0 } }
    }
    #[tokio::test]
    async fn delta_keeps_unrelated_members_and_commits_atomically() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("room_delta").await.unwrap();
        db.query("CREATE dbnum_watermark:8000 SET model_sesno=12,applied_sesno=12,room_sesno=0,room_required_input=$input;")
            .bind(("input", input())).await.unwrap().check().unwrap();
        let edges = vec![edge(1),edge(2)];
        let changed = edges.iter().map(|e| e.member.refno.to_string()).collect();
        let first = PreparedDelta::new(8000,Directory::default(),input(),&changed,&edges).unwrap();
        assert!(first.commit_on(&db).await.is_err());
        first.stage_on(&db).await.unwrap();
        first.commit_on(&db).await.unwrap();
        let member = edges[0].member.refno.to_string();
        let untouched = edges[1].member.refno.to_string();
        assert_eq!(read_member_on(&db,8000,&member).await.unwrap().len(),1);
        let remove = PreparedDelta::new(8000,first.next.clone(),input(),&[member.clone()].into_iter().collect(),&[]).unwrap();
        assert_eq!(remove.edge_writes(),0);
        assert_eq!(remove.next.fragments[&untouched],first.next.fragments[&untouched]);
        remove.stage_on(&db).await.unwrap();
        assert_eq!(read_member_on(&db,8000,&member).await.unwrap().len(),1,"staged deletion is private");
        remove.commit_on(&db).await.unwrap();
        assert!(read_member_on(&db,8000,&member).await.unwrap().is_empty());
        assert_eq!(read_member_on(&db,8000,&untouched).await.unwrap().len(),1);
        assert!(read_member_compatible_on(&db,8000,&member).await.unwrap().is_empty());
        assert_eq!(read_member_compatible_on(&db,8000,&untouched).await.unwrap().len(),1);
        // 对手 CAS 退役（ADR-072 D4）：同库单写者，重放一份旧结果不再被库这一层挡住——
        // 挡陈旧结果的是 `room_task` 那道任务结果门。这里只钉「重放照样过得去且自洽」。
        first.commit_on(&db).await.unwrap();
        assert_eq!(read_member_on(&db,8000,&member).await.unwrap().len(),1,"replayed directory is visible again");
        db.query("UPDATE dbnum_watermark:8000 SET room_result_version='legacy';
            CREATE room_result:legacy SET dbnum=8000,result_version='legacy',member=$member,panel='8000/99',room_num='R1';")
            .bind(("member",member.clone())).await.unwrap().check().unwrap();
        assert_eq!(read_member_compatible_on(&db,8000,&member).await.unwrap().len(),1);
    }

    #[test]
    fn small_delta_visits_only_dirty_bucket_and_preserves_full_result() {
        let edges: Vec<_> = (1..=60_000).map(edge).collect();
        let changed = edges.iter().map(|row| row.member.refno.to_string()).collect();
        let initial = PreparedDelta::new(8000, Directory::default(), input(), &changed, &edges).unwrap();
        // Exercise the actual local artifact representation, not just a warm
        // in-memory index which would hide a restart-time full rebuild.
        let restored: Directory = serde_json::from_slice(&serde_json::to_vec(&initial.next).unwrap()).unwrap();
        let member = edges[0].member.refno.to_string();
        let dirty = bucket(&member);
        let dirty_count = restored.bucket_members.as_ref().unwrap()[&dirty].len();
        let untouched = edges[1].member.refno.to_string();
        let untouched_edges = restored.members[&untouched].as_ptr();
        let delta = PreparedDelta::new(8000, restored, input(), &[member.clone()].into_iter().collect(), &[]).unwrap();
        assert_eq!(delta.directory_members_visited(), dirty_count);
        assert_eq!(delta.next.members[&untouched].as_ptr(), untouched_edges,
            "unchanged edge allocations must be transferred, not cloned");
        assert!(dirty_count < 600, "small change must not scan 60,000 members");
        assert_eq!(delta.fragments.len(), 1);
        assert_eq!(delta.edge_writes(), 0);
        assert!(delta.pages.iter().all(|(_, page)| page.len() <= PAGE_SIZE));
        for (key, pages) in &initial.next.buckets {
            if key != &dirty { assert_eq!(&delta.next.buckets[key], pages); }
        }
        let mut expected_members = initial.next.members.clone();
        expected_members.insert(member, Vec::new());
        assert_eq!(delta.next.members, expected_members);
        // Full calculation of exactly the same input must publish exactly the
        // same content-addressed directory, including the deletion tombstone.
        let full = PreparedDelta::new(8000, Directory::default(), input(), &changed, &edges[1..]).unwrap();
        assert_eq!(delta.next.version, full.next.version);
        assert_eq!(delta.next.fragments, full.next.fragments);
        println!("directory fixture total=60000 visited={} fragments={} pages={} edge_writes={}",
            delta.directory_members_visited(), delta.fragments.len(), delta.pages.len(), delta.edge_writes());
    }

    #[test]
    fn legacy_directory_index_migrates_once_without_changing_result_version() {
        let edges: Vec<_> = (1..=1000).map(edge).collect();
        let changed = edges.iter().map(|row| row.member.refno.to_string()).collect();
        let initial = PreparedDelta::new(8000, Directory::default(), input(), &changed, &edges).unwrap();
        let mut legacy = serde_json::to_value(&initial.next).unwrap();
        legacy.as_object_mut().unwrap().remove("bucket_members");
        let restored = serde_json::from_value(legacy).unwrap();
        let upgraded = PreparedDelta::new(8000, restored, input(), &BTreeSet::new(), &[]).unwrap();
        assert_eq!(upgraded.directory_members_visited(), 1000);
        assert_eq!(upgraded.next.version, initial.next.version);
        assert!(upgraded.pages.is_empty() && upgraded.fragments.is_empty());
        let again: Directory = serde_json::from_slice(&serde_json::to_vec(&upgraded.next).unwrap()).unwrap();
        let unchanged = PreparedDelta::new(8000, again, input(), &changed, &edges).unwrap();
        assert_eq!(unchanged.directory_members_visited(), 0);
        assert!(unchanged.pages.is_empty() && unchanged.fragments.is_empty());
        assert_eq!(unchanged.next.version, initial.next.version);
    }

    #[test]
    fn dirty_bucket_reuses_unchanged_pages() {
        let key = bucket(&edge(1).member.refno.to_string());
        let edges: Vec<_> = (1..1_000_000).map(edge)
            .filter(|row| bucket(&row.member.refno.to_string()) == key).take(600).collect();
        assert_eq!(edges.len(), 600);
        let changed = edges.iter().map(|row| row.member.refno.to_string()).collect();
        let initial = PreparedDelta::new(8000, Directory::default(), input(), &changed, &edges).unwrap();
        assert_eq!(initial.pages.len(), 3);
        let member = edges[0].member.refno.to_string();
        let old_pages = initial.next.buckets[&key].clone();
        let delta = PreparedDelta::new(8000, initial.next, input(), &[member].into_iter().collect(), &[]).unwrap();
        assert_eq!(delta.pages.len(), 1, "unchanged pages in a dirty bucket must not be rewritten");
        assert_eq!(delta.next.buckets[&key].iter().filter(|page| old_pages.contains(page)).count(), 2);
        assert_eq!(delta.fragments.len(), 1);
    }
}
