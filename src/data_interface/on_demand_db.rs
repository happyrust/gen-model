//! Request-scoped E3D database access for on-demand model generation.
//!
//! This module intentionally has one reader: `e3d_io::ReadOnlyEngine`, via
//! [`DirectStore`]. It selects the live session/index root once, descends the
//! B+ tree for each requested RefNo, and never falls back to a whole-file scan.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aios_core::{NamedAttrMap, RefU64};
use anyhow::Context;

use super::cata_closure::InMemoryCataLocator;
use super::direct_store::{DbPin, DirectSchema, DirectStore, DirectStoreError};

const INVALID_REF0_SENTINEL: u32 = 0x8000_0001;

#[derive(Debug, Clone)]
pub(crate) struct OnDemandElement {
    pub(crate) att: NamedAttrMap,
    pub(crate) owner: RefU64,
    pub(crate) noun: u32,
    pub(crate) children: Vec<RefU64>,
}

pub(crate) struct OnDemandDbSession {
    path: PathBuf,
    dbnum: u32,
    sesno: u32,
    store: DirectStore,
    parent: Option<Box<OnDemandDbSession>>,
}

impl OnDemandDbSession {
    pub(crate) fn open(path: &Path) -> anyhow::Result<Self> {
        let mut session = Self::open_single(path)?;
        if let Some(parent) = crate::data_interface::extract_family::parent_path_of(path)
            .filter(|parent| parent.is_file() && parent != path)
        {
            session.parent = Some(Box::new(Self::open_single(&parent)?));
        }
        Ok(session)
    }

    fn open_single(path: &Path) -> anyhow::Result<Self> {
        let mut engine = e3d_io::ReadOnlyEngine::open(path)
            .with_context(|| format!("open e3d-io session {}", path.display()))?;
        let dbnum = engine.descriptor().db_mark;
        let sesno = engine.session().sesno;
        let db_type = e3d_attlib::db1_dehash(engine.descriptor().noun)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "e3d-io descriptor noun 0x{:08x} is not a database type for {}",
                    engine.descriptor().noun,
                    path.display()
                )
            })?;

        let mut ref0_to_dbnum = HashMap::new();
        for (refno, _, _) in engine
            .indexed_refnos()
            .with_context(|| format!("walk e3d-io live index {}", path.display()))?
        {
            if refno.word0 != 0 && refno.word0 != INVALID_REF0_SENTINEL {
                ref0_to_dbnum.insert(refno.word0, dbnum);
            }
        }
        anyhow::ensure!(
            !ref0_to_dbnum.is_empty(),
            "e3d-io live index contains no Ref0 identities: {}",
            path.display()
        );

        let locator = InMemoryCataLocator::from_parts(
            ref0_to_dbnum,
            HashMap::from([(dbnum, (db_type.clone(), String::new(), path.to_path_buf()))]),
        );
        let schema = Arc::new(DirectSchema::open_from_env()?);
        let store = DirectStore::new(schema, Arc::new(locator));
        store.pin(DbPin {
            dbnum: dbnum as i32,
            db_type,
            file: path.to_path_buf(),
            sesno: Some(sesno),
        });

        Ok(Self {
            path: path.to_path_buf(),
            dbnum,
            sesno,
            store,
            parent: None,
        })
    }

    pub(crate) fn is_compare(&self) -> bool {
        false
    }

    pub(crate) fn selected_session(&self) -> Option<u32> {
        Some(self.sesno)
    }

    pub(crate) fn legacy_world_refno(&self) -> Option<RefU64> {
        None
    }

    pub(crate) async fn parse_element(
        &mut self,
        refno: RefU64,
    ) -> anyhow::Result<Option<OnDemandElement>> {
        if let Some(found) = self.parse_element_here(refno)? {
            return Ok(Some(found));
        }
        if let Some(parent) = self.parent.as_mut() {
            return parent.parse_element_here(refno);
        }
        Ok(None)
    }

    fn parse_element_here(&self, refno: RefU64) -> anyhow::Result<Option<OnDemandElement>> {
        let att = match self.store.named_attmap(refno) {
            Ok(att) => att,
            Err(DirectStoreError::NoSuchElement { .. })
            | Err(DirectStoreError::UnresolvedRef0 { .. }) => return Ok(None),
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "e3d-io read {} from dbnum {} session {} ({}): {error}",
                    refno.to_pe_key(),
                    self.dbnum,
                    self.sesno,
                    self.path.display()
                ));
            }
        };
        let children = self.store.members(refno).map_err(|error| {
            anyhow::anyhow!(
                "e3d-io read members of {} from dbnum {} session {}: {error}",
                refno.to_pe_key(),
                self.dbnum,
                self.sesno
            )
        })?;
        Ok(Some(OnDemandElement {
            owner: att.get_refu64("OWNER").unwrap_or_default(),
            noun: att.get_type_hash(),
            att,
            children,
        }))
    }
}

pub(crate) fn scan_ref0s(path: &Path, _project: &str) -> anyhow::Result<Vec<u32>> {
    let mut values = scan_ref0s_one(path)?;
    if let Some(parent) = crate::data_interface::extract_family::parent_path_of(path)
        .filter(|parent| parent.is_file() && parent != path)
    {
        values.extend(scan_ref0s_one(&parent)?);
    }
    values.sort_unstable();
    values.dedup();
    Ok(values)
}

fn scan_ref0s_one(path: &Path) -> anyhow::Result<Vec<u32>> {
    let mut engine = e3d_io::ReadOnlyEngine::open(path)
        .with_context(|| format!("open e3d-io index {}", path.display()))?;
    let mut values = engine
        .indexed_refnos()
        .with_context(|| format!("walk e3d-io live index {}", path.display()))?
        .into_iter()
        .map(|(refno, _, _)| refno.word0)
        .filter(|ref0| *ref0 != 0 && *ref0 != INVALID_REF0_SENTINEL)
        .collect::<Vec<_>>();
    values.sort_unstable();
    values.dedup();
    Ok(values)
}

#[cfg(test)]
mod tests {
    #[test]
    fn on_demand_reader_is_e3d_only() {
        let source = include_str!("on_demand_db.rs");
        assert!(source.contains("e3d_io::ReadOnlyEngine"));
        assert!(source.contains("DirectStore"));
        assert!(!source.contains(concat!("pdms", "_io::")));
        assert!(!source.contains(concat!("parse_pdms", "_db::")));
        assert!(!source.contains(concat!("AIOS_PDMS_", "ON_DEMAND_READ_MODE")));
    }
}
