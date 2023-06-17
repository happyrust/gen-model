use std::ops::Neg;
use aios_core::options::DbOption;
use aios_core::pdms_data::{PlinParam, PlinParamData};
use aios_core::pdms_types::{AttrMap, AttrVal, RefU64};
use bb8_arangodb::arangors_lite::{AqlQuery, Database};
use dashmap::{DashMap, DashSet};
use glam::{Vec2, Vec3};
use smol_str::SmolStr;
use crate::aql_api::children::*;
use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::aql_api::PdmsPLINAttrAql;
use crate::cata::direction_parse::parse_expr_to_dir;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;

#[derive(Debug, Default)]
pub struct PlinAxis {
    pub axis: Vec2,
    pub origin: Vec2,
    pub offset: Vec2,
}

/// 传入desi的参考号，返回该参考号对应的plin attr_map 和 wall 引用的 NA 等对应的数值
/// 将所有有形集的信息提前缓存到图数据里,要不要缓存，不缓存就做插件处理，如果有desp这些还是需要做计算处理的
/// 获取所有型集的信息
pub async fn query_plin_attrs(refnos: Vec<(RefU64, String)>, database: &ArDatabase) -> anyhow::Result<DashMap<RefU64, String>> {
    let mut result = DashMap::new();
    // 存 wall下的所有p_key以及对应的值
    let mut wall_map: DashMap<RefU64, DashMap<String, String>> = DashMap::new();
    let mut owner_map = DashMap::new();
    for (refno, _) in &refnos {
        let owner = query_owner_with_type_aql(database, *refno).await?;
        if owner.is_none() { continue; }
        let owner = owner.unwrap().0;
        owner_map.insert(*refno, owner);
        dbg!(owner);
        if wall_map.contains_key(&owner) { continue; }
        let pstr = query_foreign_refno_aql(&database, owner, &["SPRE", "PSTR"]).await?;
        if pstr.is_none() { continue; }
        let pstr_children = query_children_eles(&database, pstr.unwrap()).await?;
        let mut children = vec![];
        pstr_children.into_iter().for_each(|ele| {
            children.push(ele.refno);
        });
        let plin_attrs = query_plin_attrs_with_refnos(children, &database).await?;
        for plin_attr in plin_attrs {
            let plin_refno = RefU64::from_url_refno(&plin_attr._key);
            if plin_refno.is_none() { continue; }
            let attr = plin_attr.attr;
            let p_key = attr.get_val("PKEY");
            let plax = attr.get_val("PLAX");
            if p_key.is_none() || plax.is_none() { continue; }
            wall_map.entry(owner).or_insert_with(DashMap::new)
                .entry(p_key.unwrap().string_value()).or_insert(plax.unwrap().string_value());
        }
    }
    for (refno, pos_line) in refnos {
        let owner = owner_map.get(&refno);
        if owner.is_none() { continue; }
        let plin_map = wall_map.get(&owner.unwrap());
        if plin_map.is_none() { continue; }
        if let Some(value) = plin_map.unwrap().value().get(&pos_line) {
            result.entry(refno).or_insert(value.value().to_string());
        }
    }
    Ok(result)
}


impl AiosDBManager {
    ///查询形集PLIN的值，todo 需要做缓存优化
    pub async fn query_pline(&self, refno: RefU64, jusl: &str) -> anyhow::Result<Option<PlinParamData>> {
        let database = self.get_arango_db().await?;
        let Some(psref) = query_foreign_refno_aql(&database, refno, &["SPRE", "PSTR"]).await? else {
            return Ok(None);
        };
        // dbg!(psref);
        let c_refnos = query_children_refnos(&database, psref).await?;
        // dbg!(&c_refnos);
        for c_refno in c_refnos {
            let a = self.get_attr(c_refno).await?;
            let Some(p_key) = a.get_as_string("PKEY") else {
                continue;
            };
            if p_key == jusl {
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

                return Ok(Some(PlinParamData{
                    pt: Vec3::new(x, y, 0.0) + Vec3::new(dx, dy, 0.0) * plax,
                    plax,
                }));
            }
        }
        Ok(None)
    }
}

/// 传入plin参考号集合，返回集合中的所有plin的attr_map
async fn query_plin_attrs_with_refnos(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<PdmsPLINAttrAql>> {
    let mut children = vec![];
    refnos.into_iter().for_each(|refno| {
        children.push(RefU64::to_url_refno(&refno))
    });
    let aql = AqlQuery::new("
    let data = @element
    for v in data
    let e = document('plin_eles',v)
        return {
            '_key':e._key,
            'attr':e.attr
        } "
    ).bind_var("element", children)
        ;
    let result: Vec<PdmsPLINAttrAql> = database.aql_query(aql).await?;
    Ok(result)
}

async fn query_plin_attrs_with_refno(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Vec<PdmsPLINAttrAql>> {
    let aql = AqlQuery::new("
    let e = document('plin_eles',@refno)
        return {
            '_key':e._key,
            'attr':e.attr
        } "
    ).bind_var("refno", refno.to_url_refno());
    let result: Vec<PdmsPLINAttrAql> = database.aql_query(aql).await?;
    Ok(result)
}

pub fn match_jusline_attr(exp: String, para: Vec<f64>) -> f64 {
    match exp.as_str() {
        "DESP[1]" => para[0],
        "DESP[2]" => para[1],
        _ => 0.0,
    }
}

#[tokio::test]
async fn test_query_plin_attrs() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    // let request = vec![(RefU64::from_refno_str("23584/5934").unwrap(), "IBOW".to_string()),
    //                    (RefU64::from_refno_str("23584/5935").unwrap(), "IBOW".to_string()),
    //                    (RefU64::from_refno_str("23584/5936").unwrap(), "OBOW".to_string())];
    let request = vec![(RefU64::from_refno_str("17496/145248").unwrap(), "OBOW".to_string())];
    let result = query_plin_attrs(request, &database).await?;
    dbg!(&result);
    Ok(())
}

#[tokio::test]
async fn test_query_wall_jusl_value() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let result = query_pline_value(&database, RefU64::from_refno_str("23584/5931").unwrap(), "NA").await?;
    dbg!(&result);
    Ok(())
}