//! On-demand model generation service.
//!
//! The viewer calls this service when a requested reference has no renderable
//! model. Concurrent misses for the same delivery unit are coalesced: after
//! acquiring the unit lock the service rechecks the database, so only the first
//! request performs generation.

use std::sync::Arc;

use aios_core::RefnoEnum;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::data_interface::generation_root::{
    configured_delivery_unit_types, resolve_live_element_generation_root,
};
use crate::data_interface::manual_update::generate_unit_model;
use crate::data_interface::tidb_manager::AiosDBManager;

static GENERATION_LOCKS: Lazy<DashMap<RefnoEnum, Arc<Mutex<()>>>> = Lazy::new(DashMap::new);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnDemandModelStatus {
    AlreadyAvailable,
    Generated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnDemandModelResult {
    pub requested_refno: String,
    pub generation_root: String,
    pub generation_root_noun: String,
    pub status: OnDemandModelStatus,
    pub model_available: bool,
    pub model_instance_count: usize,
}

impl AiosDBManager {
    /// Ensure that `requested_refno` has a renderable model.
    ///
    /// A missing model is normalized through the shared generation-root policy,
    /// then generated once through the same backend path used by manual updates.
    pub async fn ensure_model_generated(
        &self,
        requested_refno: RefnoEnum,
    ) -> anyhow::Result<OnDemandModelResult> {
        let (root, root_noun) = resolve_generation_root(requested_refno).await?;
        let initial_count = renderable_instance_count(requested_refno).await?;
        if initial_count > 0 {
            return Ok(OnDemandModelResult {
                requested_refno: requested_refno.to_pdms_str(),
                generation_root: root.to_pdms_str(),
                generation_root_noun: root_noun,
                status: OnDemandModelStatus::AlreadyAvailable,
                model_available: true,
                model_instance_count: initial_count,
            });
        }

        let lock = GENERATION_LOCKS
            .entry(root)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        // Another request may have completed while this request was waiting.
        let rechecked_count = renderable_instance_count(requested_refno).await?;
        if rechecked_count > 0 {
            return Ok(OnDemandModelResult {
                requested_refno: requested_refno.to_pdms_str(),
                generation_root: root.to_pdms_str(),
                generation_root_noun: root_noun,
                status: OnDemandModelStatus::AlreadyAvailable,
                model_available: true,
                model_instance_count: rechecked_count,
            });
        }

        generate_unit_model(self, &root.to_pdms_str()).await?;

        let generated_count = renderable_instance_count(requested_refno).await?;
        if generated_count == 0 {
            anyhow::bail!(
                "已生成生成根 {} ({})，但请求构件 {} 仍没有可渲染模型",
                root.to_pdms_str(),
                root_noun,
                requested_refno
            );
        }
        Ok(OnDemandModelResult {
            requested_refno: requested_refno.to_pdms_str(),
            generation_root: root.to_pdms_str(),
            generation_root_noun: root_noun,
            status: OnDemandModelStatus::Generated,
            model_available: true,
            model_instance_count: generated_count,
        })
    }
}

async fn renderable_instance_count(refno: RefnoEnum) -> anyhow::Result<usize> {
    // `query_deep_children_refnos` includes the requested node itself. This is
    // important for tree/container refs such as EQUI and BRAN: those nodes
    // commonly have no direct inst_relate row although their generated subtree
    // is fully renderable.
    let scope = aios_core::query_deep_children_refnos(refno).await?;
    let rows = aios_core::query_insts(scope.iter(), true).await?;
    Ok(rows
        .iter()
        .map(|row| {
            row.insts.len() + usize::from(row.pts.as_ref().is_some_and(|points| !points.is_empty()))
        })
        .sum())
}

async fn resolve_generation_root(
    requested_refno: RefnoEnum,
) -> anyhow::Result<(RefnoEnum, String)> {
    let unit_types = configured_delivery_unit_types();
    let root = resolve_live_element_generation_root(requested_refno, &unit_types)
        .await?
        .ok_or_else(|| anyhow::anyhow!("构件 {} 无法解析生成根", requested_refno))?;
    Ok((root.root, root.noun))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_status_is_stable_for_client_protocol() {
        let result = OnDemandModelResult {
            requested_refno: "16192/1".into(),
            generation_root: "16192/2".into(),
            generation_root_noun: "EQUI".into(),
            status: OnDemandModelStatus::Generated,
            model_available: true,
            model_instance_count: 3,
        };
        let json = serde_json::to_string(&result).unwrap();
        let roundtrip: OnDemandModelResult = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip, result);
    }

    /// Without project overrides the on-demand path must still recover the
    /// delivery units the viewer asks for most often.
    #[test]
    fn default_units_include_required_types() {
        let units = crate::data_interface::generation_root::resolve_delivery_unit_types(&[]);
        for required in ["BRAN", "HANG", "SUPPO", "EQUI"] {
            assert!(units.iter().any(|unit| unit == required));
        }
    }

    /// Live recovery probe for the configured local SurrealDB/E3D dataset.
    ///
    /// Prepare a target without `inst_relate`, set
    /// `AIOS_ON_DEMAND_TEST_REFNO=24384/24777`, then run this ignored test.
    #[tokio::test]
    #[ignore = "requires the configured local SurrealDB and parsed E3D project"]
    async fn live_generates_a_missing_model() {
        let refno =
            std::env::var("AIOS_ON_DEMAND_TEST_REFNO").expect("set AIOS_ON_DEMAND_TEST_REFNO");
        let refno = RefnoEnum::from(refno.as_str());
        assert!(refno.is_valid(), "invalid test refno");

        aios_core::init_test_surreal()
            .await
            .expect("connect to local SurrealDB");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("initialize local database manager");
        let result = manager
            .ensure_model_generated(refno)
            .await
            .expect("generate missing model");

        assert!(result.model_available);
        assert!(result.model_instance_count > 0);
        if let Ok(expected) = std::env::var("AIOS_ON_DEMAND_EXPECT_ROOT_NOUN") {
            assert_eq!(result.generation_root_noun, expected);
        }
        if let Ok(expected) = std::env::var("AIOS_ON_DEMAND_EXPECT_ROOT") {
            assert_eq!(result.generation_root, expected);
        }
    }
}
