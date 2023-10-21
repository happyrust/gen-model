use std::{env, fs};
use std::io::Write;
use aios_core::create_attas_structs::VirtualEmbedGraphNode;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject, ItemValue};
use aios_core::negative_mesh_type::NegativeEdges;
use aios_core::options::DbOption;
use aios_core::pdms_types::RefU64;
use aios_core::tool::hash_tool::hash_two_str;
use aios_core::create_attas_structs::VirtualEmbedGraphNodeQuery;
use bb8_arangodb::arangors_lite::AqlQuery;
use sqlx::{MySql, Pool, Row};
use crate::arangodb::ArDatabase;
use crate::consts::AQL_PDMS_ELES_COLLECTION;
use crate::data_center_api::hole::{convert_time_to_vec, get_pos_from_str};
use crate::consts::{AQL_EMBED_DATA_COLLECTION, AQL_EMBED_EDGE_COLLECTION};
use crate::data_center_api::data_api::get_refno_latest_version;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::{save_arangodb_doc, update_arangodb_doc};
use crate::test::common::get_arangodb_conn_from_db_option_for_test;
use crate::consts::*;

pub async fn create_embed_data(pool: &Pool<MySql>) -> anyhow::Result<Option<DataCenterProject>> {
    let mut instances = Vec::new();
    for i in 7..30 {
        let instance = query_embed_data(i, pool).await?;
        if let Some(instance) = instance {
            instances.push(instance);
        }
    }
    let project = DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: "1516".to_string(),
        owner: "KY1801".to_string(),
        instances,
    };
    Ok(Some(project))
}

pub async fn create_embed_data_aql(keys: Vec<String>, project_code: &str, database: &ArDatabase) -> anyhow::Result<Option<Vec<DataCenterInstance>>> {
    let mut instances = Vec::new();
    let embed_datas = query_embed_data_by_keys_aql(keys, &database).await?;
    for (idx, embed_data) in embed_datas.into_iter().enumerate() {
        let Some(instance) = get_embed_data_aql(idx, embed_data, project_code).await? else { continue; };
        instances.push(instance);
    }
    // let project = DataCenterProject {
    //     package_code: DataCenterProject::convert_package_code(),
    //     project_code: "1516".to_string(),
    //     owner: "KY1801".to_string(),
    //     instances,
    // };
    Ok(Some(instances))
    // instances
}

async fn query_embed_data(id: u64, pool: &Pool<MySql>) -> anyhow::Result<Option<DataCenterInstance>> {
    let mut instances = Vec::new();
    let sql = gen_query_embed_data_sql(id);
    let result = sqlx::query(&sql).fetch_one(pool).await;
    match result {
        Ok(result) => {
            let ref_str = result.try_get::<String, _>("REF").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC1".to_string(),
                value: AttrValue::AttrString(ref_str).into(),
            });
            let code = result.try_get::<String, _>("Code").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC2".to_string(),
                value: AttrValue::AttrString(code).into(),
            });
            let rely_item = result.try_get::<String, _>("RelyItem").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC3".to_string(),
                value: AttrValue::AttrString(rely_item).into(),
            });
            let rely_item_ref = result.try_get::<String, _>("RelyItemRef").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC4".to_string(),
                value: AttrValue::AttrString(rely_item_ref).into(),
            });

            let main_item = result.try_get::<String, _>("MainItem").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC6".to_string(),
                value: AttrValue::AttrString(main_item).into(),
            });

            let mut stucc_7 = Vec::new();
            stucc_7.push(ItemValue::String("T".to_string()));
            stucc_7.push(ItemValue::String("Te".to_string()));
            stucc_7.push(ItemValue::String("Test".to_string()));
            stucc_7.push(ItemValue::Int(1));
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC7".to_string(),
                value: AttrValue::AttrItemArray(stucc_7).into(),
            });

            let ori = result.try_get::<String, _>("Ori").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC10".to_string(),
                value: AttrValue::AttrString(ori).into(),
            });

            let work = result.try_get::<String, _>("Work").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC11".to_string(),
                value: AttrValue::AttrString(work).into(),
            });
            let load = result.try_get::<String, _>("Load").unwrap_or("".to_string());
            let load = get_pos_from_str(load);
            let load = if load.len() > 5 { load } else { vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0] };
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC12".to_string(),
                value: AttrValue::AttrFloatArray(load.to_vec()).into(),
            });

            let sub_material = result.try_get::<String, _>("SubsMaterial").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC14".to_string(),
                value: AttrValue::AttrString(sub_material).into(),
            });
            let work_by = result.try_get::<String, _>("WorkBy").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC15".to_string(),
                value: AttrValue::AttrString(work_by).into(),
            });
            let time = result.try_get::<String, _>("Time").unwrap_or("".to_string());
            let time = time.replace("/", "-");
            let time = convert_time_to_vec(&time);
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC16".to_string(),
                value: AttrValue::AttrStrArray(time).into(),
            });

            let open_item = result.try_get::<String, _>("OpenItem").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC17".to_string(),
                value: AttrValue::AttrString(open_item).into(),
            });
            let note = result.try_get::<String, _>("Note").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC18".to_string(),
                value: AttrValue::AttrString(note).into(),
            });
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC19".to_string(),
                value: AttrValue::AttrString("Test".to_string()).into(),
            });

            let fitt_id = result.try_get::<String, _>("FittID").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC27".to_string(),
                value: AttrValue::AttrString(fitt_id).into(),
            });
            let embed_bpid = result.try_get::<String, _>("EmbedBPID").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC28".to_string(),
                value: AttrValue::AttrString(embed_bpid).into(),
            });
            let embed_b_pver = result.try_get::<String, _>("EmbedBPVER").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC29".to_string(),
                value: AttrValue::AttrString(embed_b_pver).into(),
            });
            let rely_item_bpid = result.try_get::<String, _>("RelyItemBPID").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC30".to_string(),
                value: AttrValue::AttrString(rely_item_bpid).into(),
            });
            let rely_item_bpver = result.try_get::<String, _>("RelyItemBPVER").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC31".to_string(),
                value: AttrValue::AttrString(rely_item_bpver).into(),
            });

            let form = result.try_get::<String, _>("Form").unwrap_or("".to_string());

            return match form.as_str() {
                "标准埋件(P)" => {
                    let stander_type = result.try_get::<String, _>("StanderType").unwrap_or("".to_string());
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCA1".to_string(),
                        value: AttrValue::AttrString(stander_type).into(),
                    });
                    let size_length = result.try_get::<f32, _>("SizeLength").unwrap_or(0.0);
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCA2".to_string(),
                        value: AttrValue::AttrFloat(size_length).into(),
                    });
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCA3".to_string(),
                        value: AttrValue::AttrString("预埋板".to_string()).into(),
                    });
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCA4".to_string(),
                        value: AttrValue::AttrFloat(0.0).into(),
                    });
                    Ok(Some(DataCenterInstance {
                        object_model_code: "STUCCA".to_string(),
                        project_code: "1516".to_string(),
                        instance_code: format!("STUCCA{}", id),
                        version: get_refno_latest_version(),
                        attributes: instances,
                    }))
                }
                "非标准埋件(N)" => {
                    let size_length = result.try_get::<f32, _>("SizeLength").unwrap_or(0.0);
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCB1".to_string(),
                        value: AttrValue::AttrFloat(size_length).into(),
                    });
                    let size_thickness = result.try_get::<f32, _>("SizeThickness").unwrap_or(0.0);
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCB2".to_string(),
                        value: AttrValue::AttrFloat(size_thickness).into(),
                    });
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCB3".to_string(),
                        value: AttrValue::AttrString("非标预埋板".to_string()).into(),
                    });
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCB4".to_string(),
                        value: AttrValue::AttrString("Test".to_string()).into(),
                    });
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCB5".to_string(),
                        value: AttrValue::AttrFloat(0.0).into(),
                    });
                    Ok(Some(DataCenterInstance {
                        object_model_code: "STUCCB".to_string(),
                        project_code: "1516".to_string(),
                        instance_code: format!("STCUCCB{}", id),
                        version: get_refno_latest_version(),
                        attributes: instances,
                    }))
                }
                _ => {
                    Ok(None)
                }
            };
        }
        _ => {}
    }
    Ok(None)
}

async fn get_embed_data_aql(idx: usize, embed_data: VirtualEmbedGraphNode, project_code: &str) -> anyhow::Result<Option<DataCenterInstance>> {
    let mut instances = Vec::new();
    let ref_str = embed_data.ref_standard;
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC1".to_string(),
        value: AttrValue::AttrString(ref_str).into(),
    });
    let code = embed_data._key;
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC2".to_string(),
        value: AttrValue::AttrString(code).into(),
    });
    let rely_item = embed_data.rely_item;
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC3".to_string(),
        value: AttrValue::AttrString(rely_item).into(),
    });
    let rely_item_ref = embed_data.rely_item_ref;
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC4".to_string(),
        value: AttrValue::AttrString(rely_item_ref).into(),
    });

    let main_item = embed_data.main_item;
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC6".to_string(),
        value: AttrValue::AttrString(main_item).into(),
    });

    let mut stucc_7 = Vec::new();
    stucc_7.push(ItemValue::String("T".to_string()));
    stucc_7.push(ItemValue::String("Te".to_string()));
    stucc_7.push(ItemValue::String("Test".to_string()));
    stucc_7.push(ItemValue::Int(1));
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC7".to_string(),
        value: AttrValue::AttrItemArray(stucc_7).into(),
    });

    let ori = embed_data.ori;
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC10".to_string(),
        value: AttrValue::AttrString(ori).into(),
    });

    let work = embed_data.work;
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC11".to_string(),
        value: AttrValue::AttrString(work).into(),
    });
    let load = embed_data.load;
    let load = get_pos_from_str(load);
    let load = if load.len() > 5 { load } else { vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0] };
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC12".to_string(),
        value: AttrValue::AttrFloatArray(load.to_vec()).into(),
    });

    let sub_material = embed_data.subs_material;
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC14".to_string(),
        value: AttrValue::AttrString(sub_material).into(),
    });
    let work_by = embed_data.work_by;
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC15".to_string(),
        value: AttrValue::AttrString(work_by).into(),
    });
    let time = embed_data.time.replace("/", "-");
    let time = convert_time_to_vec(&time);
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC16".to_string(),
        value: AttrValue::AttrStrArray(time).into(),
    });

    let open_item = embed_data.open_item;
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC17".to_string(),
        value: AttrValue::AttrString(open_item).into(),
    });
    let note = embed_data.note;
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC18".to_string(),
        value: AttrValue::AttrString(note).into(),
    });
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC19".to_string(),
        value: AttrValue::AttrString("Test".to_string()).into(),
    });

    let fitt_id = embed_data.fitt_id;
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC27".to_string(),
        value: AttrValue::AttrString(fitt_id).into(),
    });
    let embed_bpid = embed_data.embed_bpid;
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC28".to_string(),
        value: AttrValue::AttrString(embed_bpid).into(),
    });
    let embed_b_pver = embed_data.embed_bpver;
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC29".to_string(),
        value: AttrValue::AttrString(embed_b_pver).into(),
    });
    let rely_item_bpid = embed_data.rely_item_bpid;
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC30".to_string(),
        value: AttrValue::AttrString(rely_item_bpid).into(),
    });
    let rely_item_bpver = embed_data.rely_item_bpver;
    instances.push(DataCenterAttr {
        attribute_model_code: "STUCC31".to_string(),
        value: AttrValue::AttrString(rely_item_bpver).into(),
    });

    let form = embed_data.form;

    return match form.as_str() {
        "标准埋件(P)" => {
            let stander_type = embed_data.stander_type;
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCCA1".to_string(),
                value: AttrValue::AttrString(stander_type).into(),
            });
            let size_length = embed_data.size_length;
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCCA2".to_string(),
                value: AttrValue::AttrFloat(size_length).into(),
            });
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCCA3".to_string(),
                value: AttrValue::AttrString("预埋板".to_string()).into(),
            });
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCCA4".to_string(),
                value: AttrValue::AttrFloat(0.0).into(),
            });
            Ok(Some(DataCenterInstance {
                object_model_code: "STUCCA".to_string(),
                project_code: project_code.to_string(),
                instance_code: format!("STUCCA{}", idx),
                version: get_refno_latest_version(),
                attributes: instances,
            }))
        }
        "非标准埋件(N)" => {
            let size_length = embed_data.size_length;
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCCB1".to_string(),
                value: AttrValue::AttrFloat(size_length).into(),
            });
            let size_thickness = embed_data.size_thickness;
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCCB2".to_string(),
                value: AttrValue::AttrFloat(size_thickness).into(),
            });
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCCB3".to_string(),
                value: AttrValue::AttrString("非标预埋板".to_string()).into(),
            });
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCCB4".to_string(),
                value: AttrValue::AttrString("Test".to_string()).into(),
            });
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCCB5".to_string(),
                value: AttrValue::AttrFloat(0.0).into(),
            });
            Ok(Some(DataCenterInstance {
                object_model_code: "STUCCB".to_string(),
                project_code: project_code.to_string(),
                instance_code: format!("STCUCCB{}", idx),
                version: get_refno_latest_version(),
                attributes: instances,
            }))
        }
        _ => { return Ok(None); }
    };
}

/// 将埋件数据保存到图数据库
pub async fn save_embed_data_to_arangodb(mut datas: Vec<VirtualEmbedGraphNode>, database: &ArDatabase) -> anyhow::Result<String> {
    for mut data in datas.iter_mut() {
        if data.map.contains_key("Version") {
            data.map.remove("Version");
        }
    }
    let json = serde_json::to_value(&datas);
    if json.is_err() { return Ok("输入的数据格式不符合规则".to_string()); }
    let json = json.unwrap();
    let r = save_arangodb_doc(json, AQL_EMBED_DATA_COLLECTION, database, false).await;
    let _edge_r = create_embed_data_edge(&datas, &database).await?;
    if let Err(r) = r {
        Ok(r.to_string())
    } else {
        Ok("保存成功".to_string())
    }
}

/// 替换埋件数据
pub async fn replace_embed_data_to_arangodb(datas: Vec<VirtualEmbedGraphNode>, database: &ArDatabase) -> anyhow::Result<String> {
    // 删除原来的边
    let keys = datas.iter().map(|x| x._key.clone()).collect::<Vec<_>>();
    let edge_aql = AqlQuery::new("\
    With embed_data,embed_edge
    for key in @keys
        for c,e in 1 inbound CONCAT('embed_data/',key) embed_edge
            REMOVE e._key IN embed_edge
    ").bind_var("keys", keys.clone());
    let result = database.aql_query::<Vec<()>>(edge_aql).await?;
    // 重新插入新的边
    match replace_embed_data_edge(&datas, &database).await {
        Ok(_) => {}
        Err(e) => {
            return Ok(e.to_string());
        }
    }
    let data_len = datas.len();
    // 替换数据
    for data in datas {
        let json = serde_json::to_value(&data);
        if json.is_err() { return Ok("输入的数据格式不符合规则".to_string()); }
        let json = json.unwrap();
        match update_arangodb_doc(&data._key, json, AQL_EMBED_DATA_COLLECTION, &database).await {
            Ok(_) => {}
            Err(e) => {
                return Ok(e.to_string());
            }
        }
    }
    // let json = serde_json::to_value(&datas);
    // if json.is_err() { return Ok("输入的数据格式不符合规则".to_string()); }
    // let json = json.unwrap();
    // match save_arangodb_doc(json, AQL_EMBED_DATA_COLLECTION, &database, true).await {
    //     Ok(_) => {}
    //     Err(e) => {
    //         return Ok(e.to_string());
    //     }
    // }
    Ok(format!("替换 {} 条数据 成功", data_len))
}

async fn create_embed_data_edge(data: &Vec<VirtualEmbedGraphNode>, database: &ArDatabase) -> anyhow::Result<()> {
    let mut edges = Vec::new();
    for d in data {
        let refno = RefU64::from_refno_str(&d.rely_item_ref);
        if refno.is_err() { continue; }
        let refno = refno.unwrap();
        let from = format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_url_refno());
        let to = format!("{}/{}", AQL_EMBED_DATA_COLLECTION, d._key);
        let hash = hash_two_str(&from, &to);
        edges.push(NegativeEdges {
            _key: hash.to_string(),
            _from: from,
            _to: to,
        });
    }
    if !edges.is_empty() {
        let json = serde_json::to_value(&edges)?;
        save_arangodb_doc(json, AQL_EMBED_EDGE_COLLECTION, database, false).await?;
    }
    Ok(())
}

async fn replace_embed_data_edge(data: &Vec<VirtualEmbedGraphNode>, database: &ArDatabase) -> anyhow::Result<()> {
    let mut edges = Vec::new();
    for d in data {
        let refno = RefU64::from_refno_str(&d.rely_item_ref);
        if refno.is_err() { continue; }
        let refno = refno.unwrap();
        let from = format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_url_refno());
        let to = format!("{}/{}", AQL_EMBED_DATA_COLLECTION, d._key);
        let hash = hash_two_str(&from, &to);
        edges.push(NegativeEdges {
            _key: hash.to_string(),
            _from: from,
            _to: to,
        });
    }
    if !edges.is_empty() {
        let json = serde_json::to_value(&edges)?;
        save_arangodb_doc(json, AQL_EMBED_EDGE_COLLECTION, &database, true).await?;
    }
    Ok(())
}

/// 通过埋件依附的墙或板来查询这个墙上所有的埋件数据
pub async fn query_embed_data_aql(rely_refno: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<VirtualEmbedGraphNodeQuery>> {
    let keys = rely_refno.into_iter().map(|refno| format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_url_refno())).collect::<Vec<_>>();
    let aql = AqlQuery::new("\
    with @@pdms_eles,@@embed_edge,@@embed_data
    for key in @keys
        for c in 1 outbound key @@embed_edge
            filter c != null
            return unset(c , '_id','_rev')").bind_var("keys", keys)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@embed_edge", AQL_EMBED_EDGE_COLLECTION)
        .bind_var("@embed_data", AQL_EMBED_DATA_COLLECTION);
    ;
    let result = database.aql_query::<VirtualEmbedGraphNodeQuery>(aql).await?;
    Ok(result)
}

///查询现在可进行提资的埋件
pub async fn query_available_embed_data(rely_refno: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<VirtualEmbedGraphNodeQuery>> {
    let keys = rely_refno.into_iter().map(|refno| format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_url_refno())).collect::<Vec<_>>();
    let aql = AqlQuery::new("\
    with @@pdms_eles,@@embed_edge,@@embed_data
    for key in @keys
        for c in 1 outbound key @@embed_edge
            filter c != null && c.Work=='CONFIRM'
            return unset(c , '_id','_rev')").bind_var("keys", keys)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@embed_edge", AQL_EMBED_EDGE_COLLECTION)
        .bind_var("@embed_data", AQL_EMBED_DATA_COLLECTION);
    ;
    let result = database.aql_query::<VirtualEmbedGraphNodeQuery>(aql).await?;
    Ok(result)
}


pub async fn get_embed_data_total_aql(database: &ArDatabase) -> anyhow::Result<Vec<VirtualEmbedGraphNodeQuery>> {
    let aql = AqlQuery::new("\
    for c in @@collection
        return unset(c , '_id','_rev')").bind_var("@collection", AQL_EMBED_DATA_COLLECTION);
    let result = database.aql_query::<VirtualEmbedGraphNodeQuery>(aql).await?;
    Ok(result)
}

/// 通过key来查询埋件数据
pub async fn query_embed_data_by_keys_aql(keys: Vec<String>, database: &ArDatabase) -> anyhow::Result<Vec<VirtualEmbedGraphNode>> {
    let aql = AqlQuery::new("\
    for key in @keys
        let c = document(@@embed_collection,key)
        filter c != null
        return {
            '_key': c._key,
            'RelyItem': c.RelyItem,
            'RelyItemRef': c.RelyItemRef,
            'MainItem': c.MainItem,
            'Speciality': c.Speciality,
            'Position': c.Position,
            'Ori': c.Ori,
            'Work': c.Work,
            'WorkBy': c.WorkBy,
            'Time': c.Time,
            'StanderType': c.StanderType,
            'OpenItem': c.OpenItem,
            'SizeLength': c.SizeLength,
            'SizeWidth': c.SizeWidth,
            'SizeThickness': c.SizeThickness,
            'MinThickness': c.MinThickness,
            'Load': c.Load,
            'MinDistance': c.MinDistance,
            'SubsMaterial': c.SubsMaterial,
            'FittID': c.FittID,
            'REF': c.REF,
            'Shape': c.Shape,
            'Note': c.Note,
            'EmbedBPID': c.EmbedBPID,
            'EmbedBPVER': c.EmbedBPVER,
            'RelyItemBPID': c.RelyItemBPID,
            'RelyItemBPVER': c.RelyItemBPVER,
            'Form': c.Form
        }")
        .bind_var("keys", keys)
        .bind_var("@embed_collection", AQL_EMBED_DATA_COLLECTION);
    let result = database.aql_query::<VirtualEmbedGraphNode>(aql).await?;
    Ok(result)
}

/// 删除埋件的信息，并删除边
pub async fn delete_embed_data_aql(keys: Vec<String>, database: &ArDatabase) -> anyhow::Result<bool> {
    let edge_aql = AqlQuery::new("\
    for key in @keys
        for c,e in 1 inbound CONCAT('embed_data/',key) embed_edge
            REMOVE e._key IN embed_edge
    ").bind_var("keys", keys.clone());
    let result = database.aql_query::<Vec<()>>(edge_aql).await;
    let data_aql = AqlQuery::new("\
    for key in @keys
       REMOVE key IN embed_data
    ").bind_var("keys", keys);
    let result = database.aql_query::<Vec<()>>(data_aql).await;
    Ok(!result.is_err())
}

fn get_relay_item(relay_item: String) -> AttrValue {
    let mut result = Vec::new();
    let relay_items = relay_item.split("/").collect::<Vec<_>>();
    let relay_items = relay_items.last().unwrap();
    let relay_items = relay_items.split("-").collect::<Vec<_>>();
    for i in 0..5 {
        if let Some(item) = relay_items.get(i) {
            result.push(item.to_string());
        }
    }
    AttrValue::AttrStrArray(result)
}

fn gen_query_embed_data_sql(id: u64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT * FROM {EMBED_TABLE} WHERE IntelId = {}", id));
    sql
}

#[tokio::test]
async fn test_query_embed_data() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "avevamarinesample").await?;
    let r = create_embed_data(&pool).await?;
    if let Some(r) = r {
        let mut file = fs::File::create("埋件.json")?;
        let data = serde_json::to_string(&r).unwrap();
        file.write_all(&data.into_bytes())?;
    }
    Ok(())
}

#[tokio::test]
async fn test_query_embed_data_by_keys() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let keys = vec!["7f80f3a5-66a4-481f-afd1-22242966de80".to_string()];
    let r = create_embed_data_aql(keys, &db_option.project_code, &database).await?;
    // if let Some(r) = r {
    //     let mut file = fs::File::create("埋件_aql.json")?;
    //     let data = serde_json::to_string(&r).unwrap();
    //     file.write_all(&data.into_bytes())?;
    // }
    Ok(())
}