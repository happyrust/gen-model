//! Persistent ownership marker for the data-plane row set.

use aios_core::SUL_DB;
use serde::{Deserialize, Serialize};

const SCHEMA_VERSION: u32 = 1;
const RECORD: &str = "data_face_signature:current";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataFaceSignature {
    pub project: String,
    pub namespace: String,
    pub data_face: String,
    pub schema_version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaimDecision {
    UseExisting,
    ClaimEmpty,
    ClaimLegacyIngest,
    RefuseAmbiguous,
    RefuseMismatch,
}

fn decide_claim(
    stored_matches: Option<bool>,
    nonempty: bool,
    has_applied: bool,
    ingest: bool,
) -> ClaimDecision {
    match stored_matches {
        Some(true) => ClaimDecision::UseExisting,
        Some(false) => ClaimDecision::RefuseMismatch,
        None if !nonempty => ClaimDecision::ClaimEmpty,
        None if has_applied && ingest => ClaimDecision::ClaimLegacyIngest,
        None => ClaimDecision::RefuseAmbiguous,
    }
}

pub async fn validate_or_claim(
    project: &str,
    namespace: &str,
) -> anyhow::Result<DataFaceSignature> {
    let expected = DataFaceSignature {
        project: project.to_owned(),
        namespace: namespace.to_owned(),
        data_face: crate::options::data_face().as_str().to_owned(),
        schema_version: SCHEMA_VERSION,
    };
    let mut response = SUL_DB
        .query(format!(
            "SELECT project, namespace, data_face, schema_version FROM ONLY {RECORD};\n\
             SELECT VALUE id FROM pe LIMIT 1;\n\
             SELECT VALUE applied_sesno FROM dbnum_watermark LIMIT 1;\n\
             SELECT VALUE applied_sesno FROM dbnum_watermark WHERE applied_sesno > 0 LIMIT 1;"
        ))
        .await
        .map_err(|error| anyhow::anyhow!("read data-face signature: {error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("query data-face signature: {error}"))?;
    let stored: Option<DataFaceSignature> = response.take(0)?;
    if let Some(stored) = stored {
        if decide_claim(Some(stored == expected), true, false, false) == ClaimDecision::UseExisting
        {
            return Ok(stored);
        }
        anyhow::bail!(
            "DATA_FACE_SIGNATURE_MISMATCH: store is {}/{}/{} schema {}, process requests {}/{}/{} schema {}",
            stored.project,
            stored.namespace,
            stored.data_face,
            stored.schema_version,
            expected.project,
            expected.namespace,
            expected.data_face,
            expected.schema_version
        );
    }

    let pe: Vec<surrealdb::sql::Thing> = response.take(1).unwrap_or_default();
    let watermarks: Vec<i64> = response.take(2).unwrap_or_default();
    let applied: Vec<i64> = response.take(3).unwrap_or_default();
    let nonempty = !pe.is_empty() || !watermarks.is_empty();
    let decision = decide_claim(
        None,
        nonempty,
        !applied.is_empty(),
        crate::options::data_face() == crate::options::DataFace::Ingest,
    );
    anyhow::ensure!(
        decision != ClaimDecision::RefuseAmbiguous,
        "DATA_FACE_SIGNATURE_REQUIRED: unmarked non-empty store is ambiguous; run an explicit migration (only ingest with applied_sesno > 0 may auto-claim)"
    );
    let legacy_ingest = decision == ClaimDecision::ClaimLegacyIngest;
    if legacy_ingest {
        log::warn!(
            "unmarked legacy ingest store auto-claimed because applied_sesno > 0; project={} namespace={}",
            project,
            namespace
        );
    }
    let json = serde_json::to_string(&expected)?;
    SUL_DB
        .query(format!("UPSERT {RECORD} CONTENT {json};"))
        .await
        .map_err(|error| anyhow::anyhow!("write data-face signature: {error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("commit data-face signature: {error}"))?;
    Ok(expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_shape_includes_every_ownership_dimension() {
        let value = serde_json::to_value(DataFaceSignature {
            project: "P".into(),
            namespace: "N".into(),
            data_face: "read-through".into(),
            schema_version: 1,
        })
        .unwrap();
        for key in ["project", "namespace", "data_face", "schema_version"] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
    }

    #[test]
    fn claim_matrix_is_fail_closed() {
        assert_eq!(
            decide_claim(None, false, false, false),
            ClaimDecision::ClaimEmpty
        );
        assert_eq!(
            decide_claim(None, true, true, true),
            ClaimDecision::ClaimLegacyIngest
        );
        assert_eq!(
            decide_claim(None, true, false, false),
            ClaimDecision::RefuseAmbiguous
        );
        assert_eq!(
            decide_claim(Some(false), true, true, true),
            ClaimDecision::RefuseMismatch
        );
        assert_eq!(
            decide_claim(Some(true), true, false, false),
            ClaimDecision::UseExisting
        );
    }
}
