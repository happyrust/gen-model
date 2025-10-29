use aios_core::{RefnoEnum, SUL_DB};

/// 级联删除 inst_relate 及其关联的 geo_relate 和 inst_geo 数据
///
/// 当 replace_mesh 开启时，需要完全删除之前生成的数据，包括：
/// - inst_geo: 几何体节点
/// - geo_relate: 几何关系边
/// - inst_info: 实例信息节点
/// - inst_relate: 实例关系边
///
/// # 参数
/// * `refnos` - 需要删除的 refno 列表
/// * `chunk_size` - 分批处理的大小
///
/// # 删除顺序
/// 1. inst_geo (最外层)
/// 2. geo_relate (关系边)
/// 3. inst_info (信息节点)
/// 4. inst_relate (关系边)
pub async fn delete_inst_relate_cascade(
    refnos: &[RefnoEnum],
    chunk_size: usize,
) -> anyhow::Result<()> {
    for chunk in refnos.chunks(chunk_size) {
        let mut delete_sql_vec = vec![];

        let mut inst_ids: Vec<String> = vec![];
        for refno in chunk {
            inst_ids.push(refno.to_inst_relate_key());
            let delete_sql = format!(
                r#"
                    delete array::flatten(select value [out, id, in] from {}->inst_info->geo_relate);
                "#,
                refno.to_inst_relate_key()
            );
            delete_sql_vec.push(delete_sql);
        }

        if !delete_sql_vec.is_empty() {
            let mut sql = "BEGIN TRANSACTION;\n".to_string();
            sql.push_str(&delete_sql_vec.join(""));
            sql.push_str(&format!("delete [{}];", inst_ids.join(",")));
            sql.push_str("\nCOMMIT TRANSACTION;");

            // println!("Delete Sql is {}", &sql);
            SUL_DB
                .query(sql)
                .await
                .expect("delete model insts info failed");
        }
    }

    Ok(())
}
