use std::ops::Neg;
use aios_core::options::DbOption;
use aios_core::pdms_data::{PlinParam, PlinParamData};
use aios_core::pdms_types::*;
use bb8_arangodb::arangors_lite::{AqlQuery, Database};
use dashmap::{DashMap, DashSet};
use glam::{Vec2, Vec3};
use nom::combinator::value;
use smol_str::SmolStr;
use crate::aql_api::children::*;
use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::aql_api::PdmsPLINAttrAql;
use crate::cata::direction_parse::parse_expr_to_dir;
use crate::consts::AQL_PLIN_ELES_COLLECTION;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;
use std::str::FromStr;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;



impl AiosDBManager {
    ///查询形集PLIN的值，todo 需要做缓存优化
    pub async fn query_pline(&self, refno: RefU64, jusl: &str) -> anyhow::Result<Option<PlinParamData>> {

        if self.plin_params_map.contains_key(&refno) {
            return Ok(self.plin_params_map.get(&refno).unwrap().get(jusl).map(|x| x.value().clone()));
        }
        let att = aios_core::get_named_attmap(refno).await?;
        let spre_att = aios_core::get_named_attmap(att.get_foreign_refno("SPRE").unwrap_or_default()).await.unwrap_or_default();
        let cat_att = aios_core::get_named_attmap(att.get_foreign_refno("CATR").unwrap_or_default()).await.unwrap_or_default();
        let psref = cat_att.get_foreign_refno("PSTR").unwrap_or(cat_att.get_foreign_refno("PTSS").unwrap_or_default());
        if !psref.is_valid() { return Ok(None);  }
        let c_refnos = aios_core::get_children_refnos(psref).await.unwrap_or_default();
        // dbg!(&c_refnos);
        let mut result = None;
        for c_refno in c_refnos {
            let a = aios_core::get_named_attmap(c_refno).await?;
            let Some(p_key) = a.get_as_string("PKEY") else {
                continue;
            };
            let param = PlinParam {
                vxy: [
                    a.get_as_string("PX").unwrap_or("0".to_string()),
                    a.get_as_string("PY").unwrap_or("0".to_string()),
                ],
                dxy: [
                    a.get_as_string("DX").unwrap_or("0".to_string()),
                    a.get_as_string("DY").unwrap_or("0".to_string()),
                ],
                plax: a.get_as_string("PLAX").unwrap_or("unset".to_string()),
            };
            let x = self.resolve_expression_to_f32(&param.vxy[0], refno).await?;
            let y = self.resolve_expression_to_f32(&param.vxy[1], refno).await?;
            let dx = self.resolve_expression_to_f32(&param.dxy[0], refno).await?;
            let dy = self.resolve_expression_to_f32(&param.dxy[1], refno).await?;
            let plax = parse_expr_to_dir(&param.plax).unwrap_or(Vec3::Z).normalize();
            let plin_data = PlinParamData{
                pt: Vec3::new(x, y, 0.0) + Vec3::new(dx, dy, 0.0) * plax,
                plax,
            };
            self.plin_params_map.entry(refno).or_default().insert(p_key.clone(), plin_data.clone());
            if p_key == jusl {
                result = Some(plin_data);
            }
        }
        Ok(result)
    }
}
