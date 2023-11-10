use std::collections::HashMap;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject};
use aios_core::pdms_types::RefU64;
use arangors_lite::AqlQuery;
use bevy_transform::prelude::Transform;
use glam::Vec3;
use parry2d::utils::Array1;
use crate::aql_api::children::query_ancestor_till_types_aql;
use crate::aql_api::pdms_room::query_room_name_from_refnos_aql;
use crate::consts::{AQL_FOREIGN_EDGES_COLLECTION, AQL_PDMS_EDGES_COLLECTION, AQL_PDMS_ELES_COLLECTION};
use crate::data_center_api::data_api::{get_refno_desc, get_refno_desi_desc, get_refno_latest_version, get_refno_paras};
use crate::data_center_api::electric_major::sctn::EleNodeWithSpreName;
use crate::data_center_api::electric_major::stru::get_dq_jldatu_fixing_data;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;

// 圆板类
pub async fn get_dq_fixing_data(refnos: Vec<RefU64>, aios_mgr: &AiosDBManager) -> anyhow::Result<Vec<DataCenterInstance>> {
    //获取图形数据库
    let database = aios_mgr.get_arango_db().await?;
    let mut result = Vec::new();
    // 获取电气圆板的数据
    let fixings = query_dq_circular_plate(refnos, &database).await.unwrap_or(vec![]);
    let fixing_refnos = fixings.iter().map(|x| x.refno).collect::<Vec<RefU64>>();
    //根据引用号从数据库中获取对应的房间号
    let room_name = query_room_name_from_refnos_aql(fixing_refnos, &database).await.unwrap_or(vec![]);
    //构建引用号-房间号map
    let room_map = room_name
        .into_iter()
        .map(|x| (x.refno, x.room_name))
        .collect::<HashMap<RefU64, String>>();
    //依次遍历所有的fixing，构建圆板类元数据
    for fixing in fixings {
        let mut fixing_attrs = Vec::new();
        fixing_attrs.push(DataCenterAttr {
            //fixing类型元数据的PART1参数都是引用号，统一处理
            attribute_model_code: "PART1".to_string(),
            value: AttrValue::AttrString(fixing.refno.to_refno_str()).into(),
        });
        //根据参考号，循环查找父节点，知道节点类型为STRU
        let owner_refno = aios_mgr.get_ancestor_refno_till_type(fixing.refno, &vec!["STRU"]);
        // 往上找到STRU的NAME
        let mut owner_name = "".to_string();
        //判断是否找到STRU节点
        if let Some(owner_refno) = owner_refno {
            //获取STRU节点的属性
            let owner_attr = aios_mgr.get_attr(owner_refno).await.unwrap_or_default();
            //获取STRU节点的名称
            owner_name = owner_attr.get_name().unwrap_or("".to_string());
        }
        fixing_attrs.push(DataCenterAttr {
            //PART1参数对应STRU的名称
            attribute_model_code: "PART2".to_string(),
            value: AttrValue::AttrString(owner_name).into(),
        });
        //获取fixing的世界坐标转换
        let transform = aios_mgr.get_world_transform(fixing.refno).await?.unwrap_or_default();
        let pos = transform.translation;
        fixing_attrs.push(DataCenterAttr {
            //PART4参数对应fixing节点的世界坐标位置
            attribute_model_code: "PART4".to_string(),
            value: AttrValue::AttrVec3(pos).into(),
        });
        //获取fixing节点的属性表
        let attr = aios_mgr.get_attr(fixing.refno).await.unwrap_or_default();
        //读取ORI属性值，方向
        let ori = attr.get_vec3("ORI").unwrap_or(Vec3::ZERO);
        fixing_attrs.push(DataCenterAttr {
            //PART5参数对应fixing节点的方向
            attribute_model_code: "PART5".to_string(),
            value: AttrValue::AttrVec3(ori).into(),
        });
        //SPRE名称
        let spre_name = fixing.spre_name.clone();
        fixing_attrs.push(DataCenterAttr {
            //PARTD4对应物项编码
            attribute_model_code: "PARTD4".to_string(),
            value: AttrValue::AttrString(spre_name.clone()).into(),
        });
        //房间号，默认为空
        let room_name = room_map.get(&fixing.refno).map_or("".to_string(), |x| x.to_string());
        fixing_attrs.push(DataCenterAttr {
            //PARTD14对应房间号
            attribute_model_code: "PARTD14".to_string(),
            value: AttrValue::AttrString(room_name).into(),
        });
        let mut object_code = "".to_string();
        //根据spre判断fixing所属类型
        match spre_name {
            //PARTDK（GENSEC下，type=FIXING，and spre contain JT3）
            s if s.contains("JT3") => {
                //PARTDK类型其它元数据项获取
                get_dq_finxing_jt_3(&fixing, aios_mgr, &database, &mut fixing_attrs, false).await;
                object_code = "PARTDK".to_string();
            }
            //PARTDK（GENSEC下，type=FIXING，and spre contain JT4）
            s if s.contains("JT4") => {
                //PARTDK类型其它元数据项获取
                get_dq_finxing_jt_3(&fixing, aios_mgr, &database, &mut fixing_attrs, true).await;
                object_code = "PARTDK".to_string();
            }
            //PARTDJ（type=fixing and spre contain C1）
            s if s.contains("C1") => {
                //PARTDJ类型其它元数据项获取
                get_dq_finxing_c1(&s, &mut fixing_attrs);
                object_code = "PARTDJ".to_string()
            }
            //PARTDE（type=fixing and spre contain C2 ）
            s if s.contains("C2") => {
                //PARTDE类型其它元数据项获取
                get_dq_finxing_c2(fixing.refno, &s, &mut fixing_attrs, aios_mgr);
                object_code = "PARTDE".to_string()
            }
            //PARTDH（type=fixing and spre contain MGB）
            s if s.contains("MGB") => {
                //PARTDH类型其它元数据项获取
                get_dq_finxing_mgb(fixing.refno, &s, &mut fixing_attrs, aios_mgr);
                object_code = "PARTDH".to_string()
            }
            //PARTDG （方钢(GENSEC)下面的JLDATU>FIXING）,如果和其它类型（eg:PARTDK）冲突，则其它的类型优先
            s => {
                //PARTDG类型其它元数据项获取
                get_dq_jldatu_fixing_data(fixing.refno, &s, &mut fixing_attrs, aios_mgr);
                object_code = "PARTDG".to_string()
            }
        }
        //最终的元数据构造
        result.push(DataCenterInstance {
            object_model_code: object_code,//对象代码，对应类型
            project_code: aios_mgr.db_option.project_code.to_string(),//项目代码
            instance_code: fixing.refno.to_refno_str(),//句柄，对应引用号
            version: get_refno_latest_version(),//版本号
            attributes: fixing_attrs,//属性及值
        });
    }
    Ok(result)
}

/**PARTDK（GENSEC下，type=FIXING，and spre contain JT3/JT4）独有元数据处理
@param fixing:fixing节点数据，含spre
@param aios_mgr：tidb数据库管理类
@param database：图形数据库
@param fixing_attrs:fixing节点数据属性表
 */
async fn get_dq_finxing_jt_3(fixing: &EleNodeWithSpreName,
                             aios_mgr: &AiosDBManager,
                             database: &ArDatabase,
                             mut fixing_attrs: &mut Vec<DataCenterAttr>, b_jt_4: bool) {
    fixing_attrs.push(DataCenterAttr {
        //PART3对应PARTDK的中文名：圆板
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("圆板".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD3".to_string(),
        value: AttrValue::AttrString("圆板".to_string()).into(),
    });
    // 往上找到GENSEC判断Gtype是BOX：2，BEAM：1
    let owner_refno = aios_mgr.get_ancestor_refno_till_type(fixing.refno, &vec!["GENSEC"]);
    let mut owner_name = "".to_string();
    if let Some(owner_refno) = owner_refno {
        //获取GENSEC节点的属性表
        let owner_attr = aios_mgr.get_attr(owner_refno).await.unwrap_or_default();
        //获取GENSEC节点的GTYP属性值
        owner_name = owner_attr.get_str("GTYP").map_or("".to_string(), |x| x.to_string());
    }
    match owner_name.as_str() {
        "BEAM" => {//如果TYPE为BEAM,PARTD6取值1
            fixing_attrs.push(DataCenterAttr {
                attribute_model_code: "PARTD6".to_string(),
                value: AttrValue::AttrString("1".to_string()).into(),
            });
        }
        "BOX" => {//如果TYPE为BOX,PARTD6取值2
            fixing_attrs.push(DataCenterAttr {
                attribute_model_code: "PARTD6".to_string(),
                value: AttrValue::AttrString("2".to_string()).into(),
            });
        }
        _ => {//都不是这为空
            fixing_attrs.push(DataCenterAttr {
                attribute_model_code: "PARTD6".to_string(),
                value: AttrValue::AttrString("".to_string()).into(),
            });
        }
    }
    fixing_attrs.push(DataCenterAttr {
        //引用主数据MD000008(功能等级代码),默认值F-SC1
        attribute_model_code: "PARTD8".to_string(),
        value: AttrValue::AttrString("F-SC1".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //引用主数据MD000010(规范等级代码),默认值NA
        attribute_model_code: "PARTD9".to_string(),
        value: AttrValue::AttrString("NA".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //引用主数据MD000009(抗震类别代码),默认值"抗震I级"
        attribute_model_code: "PARTD10".to_string(),
        value: AttrValue::AttrString("抗震I级".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //材质，默认值"Q355B"
        attribute_model_code: "PARTD11".to_string(),
        value: AttrValue::AttrString("Q355B".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //单位，默认值"个"
        attribute_model_code: "PARTD12".to_string(),
        value: AttrValue::AttrString("个".to_string()).into(),
    });

    //desc of cat获取对应原件的描述
    let desc = get_refno_desc(fixing.refno, &aios_mgr)
        .await
        .unwrap_or("".to_string());
    fixing_attrs.push(DataCenterAttr {
        //PARTD15d对应标准号
        attribute_model_code: "PARTD15".to_string(),
        value: desc,
    });
    //获取fixing节点的参数列表
    let mut stru_desc = None;
    let paras = get_refno_paras(fixing.refno, &aios_mgr)
        .unwrap_or(Vec::new());

    //PARTDK1对应fixing的规格,JT3取para1Xpara2，JT4取para7
    if b_jt_4 {
        //PARTDK1对应fixing的规格,JT4取para7
        fixing_attrs.push(DataCenterAttr {
            attribute_model_code: "PARTDK1".to_string(),
            value: AttrValue::AttrFloat(*(paras.get(6).unwrap_or(&0.0)) as f32)
                .into(),
        });
    } else{
        //PARTDK1对应fixing的规格,JT3取para1Xpara2
        fixing_attrs.push(DataCenterAttr {
            attribute_model_code: "PARTDK1".to_string(),
            value: AttrValue::AttrString(format!(
                "{}X{}",
                paras.get(0).unwrap_or(&0.0),
                paras.get(1).unwrap_or(&0.0)
            ))
                .into(),
        });
    }

    //PARTDK2对应fixing的厚度,JT3取para3，JT4取para8
    if b_jt_4 {
        //PARTDK2对应fixing的厚度,JT4取para8
        fixing_attrs.push(DataCenterAttr {
            attribute_model_code: "PARTDK2".to_string(),
            value: AttrValue::AttrFloat(*(paras.get(7).unwrap_or(&0.0)) as f32)
                .into(),
        });
    }
    else {
        //PARTDK2对应fixing的厚度,JT3取para3
        fixing_attrs.push(DataCenterAttr {
            attribute_model_code: "PARTDK2".to_string(),
            value: AttrValue::AttrFloat(*(paras.get(2).unwrap_or(&0.0)) as f32)
                .into(),
        });
    }

    //查找STRU父类节点
    let stru = query_ancestor_till_types_aql(
        database,
        fixing.refno,
        vec!["STRU"],
    ).await.unwrap_or(None);
    if let Some(stru) = stru {
        //获取STRU节点的desc
        let desc = get_refno_desi_desc(stru.refno, &aios_mgr)
            .await
            .unwrap_or("".to_string());
        stru_desc = Some(desc);
    }
    let mut partdk_3 = 0.0;
    let mut partdk_4 = 0.0;
    //JT3先区分S1-150取2*para4，JT4取para2
    //JT3先区分S1-151取para5
    if b_jt_4 {
        partdk_3 = *paras.get(1).unwrap_or(&0.0) as f32;
    } else {
        if let Some(stru_desc) = &stru_desc {
            match stru_desc {
                s if s.contains("S1-150") => {
                    partdk_3 = 2.0 * *paras.get(3).unwrap_or(&0.0) as f32;
                }
                s if s.contains("S1-151") => {
                    partdk_4 = *paras.get(4).unwrap_or(&0.0) as f32;
                }
                _ => {}
            }
        }
    }
    fixing_attrs.push(DataCenterAttr {
        //PARTDK3对应左右孔距
        attribute_model_code: "PARTDK3".to_string(),
        value: AttrValue::AttrFloat(partdk_3).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTDK3对应上下孔距
        attribute_model_code: "PARTDK4".to_string(),
        value: AttrValue::AttrFloat(
            partdk_4
        ).into(),
    });
}

/**PARTDJ（type=fixing and spre contain C1）独有元数据处理
@param spre_name:fixing节点的spre
@param fixing_attrs:fixing节点数据属性表
 */
fn get_dq_finxing_c1(spre_name: &str,
                     mut fixing_attrs: &mut Vec<DataCenterAttr>) {
    fixing_attrs.push(DataCenterAttr {
        //PART3对应类型中文名
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("螺纹杆".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD3对应类型名称
        attribute_model_code: "PARTD3".to_string(),
        value: AttrValue::AttrString("欧姆卡".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD4对应物项编码
        attribute_model_code: "PARTD4".to_string(),
        value: AttrValue::AttrString("SCTN".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD6对应数量
        attribute_model_code: "PARTD6".to_string(),
        value: AttrValue::AttrString("1".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD8引用主数据MD000008(功能等级代码)
        attribute_model_code: "PARTD8".to_string(),
        value: AttrValue::AttrString("F-SC1".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD9引用主数据MD000010(规范等级代码)
        attribute_model_code: "PARTD9".to_string(),
        value: AttrValue::AttrString("NA".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD10引用主数据MD000009(抗震类别代码)
        attribute_model_code: "PARTD10".to_string(),
        value: AttrValue::AttrString("抗震I级".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD11对应材质
        attribute_model_code: "PARTD11".to_string(),
        value: AttrValue::AttrString("Q355B".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD12对应单位
        attribute_model_code: "PARTD12".to_string(),
        value: AttrValue::AttrString("个".to_string()).into(),
    });
    let spre_last = spre_name.split("-").collect::<Vec<&str>>().last().map_or("".to_string(), |x| x.to_string());
    fixing_attrs.push(DataCenterAttr {
        //PARTD15对应标准号，SPRE按-分割取最后一个
        attribute_model_code: "PARTD15".to_string(),
        value: spre_last,
    });
}

/**PARTDE（type=fixing and spre contain C2 ）独有元数据处理
@param refno:fixing节点引用号
@param spre_name：fixing节点spre
@param fixing_attrs:fixing节点数据属性表
@param aios_mgr：tidb数据库管理类
 */
fn get_dq_finxing_c2(refno: RefU64,
                     spre_name: &str,
                     mut fixing_attrs: &mut Vec<DataCenterAttr>,
                     aios_mgr: &AiosDBManager) {
    fixing_attrs.push(DataCenterAttr {
        //PART3对应PARTDE的中文名称：管卡
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("管卡".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD3对应PARTDE的类型名称：管卡
        attribute_model_code: "PARTD3".to_string(),
        value: AttrValue::AttrString("管卡".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD4对应物项编码
        attribute_model_code: "PARTD4".to_string(),
        value: AttrValue::AttrString(spre_name.to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD6对应数量，默认1
        attribute_model_code: "PARTD6".to_string(),
        value: AttrValue::AttrString("1".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD8引用主数据MD000008(功能等级代码)
        attribute_model_code: "PARTD8".to_string(),
        value: AttrValue::AttrString("F-SC1".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD9引用主数据MD000010(规范等级代码)
        attribute_model_code: "PARTD9".to_string(),
        value: AttrValue::AttrString("NA".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD10引用主数据MD000009(抗震类别代码)
        attribute_model_code: "PARTD10".to_string(),
        value: AttrValue::AttrString("抗震I级".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD11对应材质
        attribute_model_code: "PARTD11".to_string(),
        value: AttrValue::AttrString("Q355B".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD12对应单位
        attribute_model_code: "PARTD12".to_string(),
        value: AttrValue::AttrString("个".to_string()).into(),
    });
    let spre_last = spre_name.split("-").collect::<Vec<&str>>().last().map_or("".to_string(), |x| x.to_string());
    fixing_attrs.push(DataCenterAttr {
        //PARTD15对应标准号，SPRE按-分割取最后一个
        attribute_model_code: "PARTD15".to_string(),
        value: spre_last,
    });
    //获取fixing节点参数列表
    let paras = get_refno_paras(refno, aios_mgr).unwrap_or(vec![]);
    //获取第10个参数
    let para_10 = paras.get(9).map_or(0.0, |x| *x);
    //获取第8个参数
    let para_8 = paras.get(7).map_or(0.0, |x| *x);
    fixing_attrs.push(DataCenterAttr {
        //PARTDE26长度对应参数10
        attribute_model_code: "PARTDE26".to_string(),
        value: AttrValue::AttrFloat(para_10 as f32).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTDE27高度对应参数8
        attribute_model_code: "PARTDE27".to_string(),
        value: AttrValue::AttrFloat(para_8 as f32).into(),
    });
}

/**PARTDH（type=fixing and spre contain MGB）独有元数据处理
@param spre_name:fixing节点的spre
@param fixing_attrs:fixing节点数据属性表
@param aios_mgr：tidb数据库管理类
 */
fn get_dq_finxing_mgb(refno: RefU64, spre_name: &str,
                      mut fixing_attrs: &mut Vec<DataCenterAttr>, aios_mgr: &AiosDBManager) {
    fixing_attrs.push(DataCenterAttr {
        //PART3对应中文名称
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("螺纹杆".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD3对应类型名称
        attribute_model_code: "PARTD3".to_string(),
        value: AttrValue::AttrString("螺纹杆".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD4对应物项编码
        attribute_model_code: "PARTD4".to_string(),
        value: AttrValue::AttrString(spre_name.to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD6对应数量
        attribute_model_code: "PARTD6".to_string(),
        value: AttrValue::AttrString("1".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD8引用主数据MD000008(功能等级代码)
        attribute_model_code: "PARTD8".to_string(),
        value: AttrValue::AttrString("F-SC1".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD9引用主数据MD000010(规范等级代码)
        attribute_model_code: "PARTD9".to_string(),
        value: AttrValue::AttrString("NA".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD10引用主数据MD000009(抗震类别代码)
        attribute_model_code: "PARTD10".to_string(),
        value: AttrValue::AttrString("抗震I级".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD11对应材质
        attribute_model_code: "PARTD11".to_string(),
        value: AttrValue::AttrString("Q355B".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTD12对应单位
        attribute_model_code: "PARTD12".to_string(),
        value: AttrValue::AttrString("个".to_string()).into(),
    });
    let spre_last = spre_name.split("-").collect::<Vec<&str>>().last().map_or("".to_string(), |x| x.to_string());
    fixing_attrs.push(DataCenterAttr {
        //PARTD15对应标准号
        attribute_model_code: "PARTD15".to_string(),
        value: spre_last,
    });
    //获取fixing节点参数列表
    let paras = get_refno_paras(refno, aios_mgr).unwrap_or(vec![]);
    //参数1
    let para_1 = paras.get(0).map_or(0.0, |x| *x);
    //参数2
    let para_2 = paras.get(1).map_or(0.0, |x| *x);
    //参数3
    let para_3 = paras.get(2).map_or(0.0, |x| *x);
    fixing_attrs.push(DataCenterAttr {
        //PARTDH26对应规格，取值para1Xpara2
        attribute_model_code: "PARTDH26".to_string(),
        value: AttrValue::AttrString(format!("{}X{}", para_1, para_2)).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        //PARTDH27对应厚度，取值参数3
        attribute_model_code: "PARTDH27".to_string(),
        value: AttrValue::AttrFloat(para_3 as f32).into(),
    });
}

/// 获取电气圆板的数据
async fn query_dq_circular_plate(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<EleNodeWithSpreName>> {
    let id = refnos.into_iter()
        .map(|refno| format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_url_refno()))
        .collect::<Vec<_>>();
    let aql = AqlQuery::new("
        with @@pdms_edges,@@pdms_eles,@@foreign_edges
        for refno in @id
        let node = document(refno)
        filter node != null
        filter node.noun == 'GENSEC'
        FOR z in 0..100 INBOUND node._id @@pdms_edges
        filter z.noun == 'FIXING'
        filter z != null
        let foreign = (
        for v, e, p in 1..2 outbound z._id @@foreign_edges
            filter p.edges[0].foreign_type == 'SPRE'
            filter e.foreign_type == 'SPRE'
            return v.name
        )
        filter foreign[0] != null
        return {
            'refno':z._key,
            'owner':z.owner,
            'name':z.name,
            'noun':z.noun,
            'spre_name': foreign[0]
        }
      ").bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("@foreign_edges", AQL_FOREIGN_EDGES_COLLECTION)
        .bind_var("id", id);
    let result = database.aql_query::<EleNodeWithSpreName>(aql).await.unwrap();
    Ok(result)
}