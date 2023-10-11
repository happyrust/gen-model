use aios_core::pdms_types::RefU64;
use crate::graph_db::pdms_arango::{remove_arangodb_with_refno_key, save_arangodb_doc};
use serde::{Deserialize, Serialize};
use crate::arangodb::ArDatabase;
use crate::consts::AQL_LOCK_REFNOS_COLLECTION;

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct LockRefnos {
    pub _key: String,
}

/// 设置锁定模型的参考号
pub async fn set_lock_refnos(refnos: &Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<()> {
    let lock_refnos = refnos.into_iter().map(|x| LockRefnos {
        _key: x.to_url_refno(),
    }).collect::<Vec<_>>();
    let json = serde_json::to_value(&lock_refnos)?;
    save_arangodb_doc(json, AQL_LOCK_REFNOS_COLLECTION, database, false).await?;
    Ok(())
}

/// 解锁模型的参考号
pub async fn unset_lock_refnos(refnos: &Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<bool> {
    remove_arangodb_with_refno_key(refnos, AQL_LOCK_REFNOS_COLLECTION, &database).await
}

