use std::fs::File;
use std::io::BufReader;
use aios_core::create_attas_structs::VirtualHoleGraphNodeQuery;
use aios_core::get_default_pdms_db_info;
use aios_core::options::DbOption;
use itertools::Itertools;
use termnius_client::structs::CreateDBAction;
use crate::graph_db::structs::PdmsEleDataVersioned;
use crate::versioned_db::client::get_versioned_client;


///创建所有的需要版本管理的schema
pub async fn create_versioned_schemas(project: &str) -> anyhow::Result<()>{

    let mut client = get_versioned_client(project).await;
    let res = client.create_db(&CreateDBAction {
        db_id: project.to_string(),
        team: Some("admin".to_string()),
        label: Some(project.into()),
        description: Some("ams e3d project".into()),
        prefixes: None,
        include_schema: false,
    }).await?;
    dbg!(res);

    //创建pdms element 的schema
    let docs = PdmsEleDataVersioned::get_scheme();
    let ele_schema_res = client.insert_doc(docs, "admin", "Create pdms element schema", true, false, true).await?;
    dbg!(ele_schema_res);

    let docs = VirtualHoleGraphNodeQuery::get_scheme();
    let virtual_hole_schema_res = client.insert_doc(&docs, "admin", "Create virtual hole schema", true, false, true).await?;
    dbg!(virtual_hole_schema_res);



    // 孔洞的测试数据
    {
        ///增加一个孔洞的测试数据
        let file = File::open("test_data/virtual_hole/hole.json")?;
        let reader = BufReader::new(file);
        let holes: Vec<VirtualHoleGraphNodeQuery> = serde_json::from_reader(reader).unwrap();
        dbg!(&holes);
        let json = holes[0].gen_versioned_data_json().unwrap();
        let virtual_hole_doc_res0 = client.insert_doc(&json, "admin", "Create virtual hole data", false, false, true).await?;
        dbg!(virtual_hole_doc_res0);

        let json = holes[1].gen_versioned_data_json().unwrap();
        let virtual_hole_doc_res1 = client.insert_doc(&json, "admin", "Modify virtual hole data", false, false, true).await?;
        dbg!(virtual_hole_doc_res1);
    }


    //创建属性的schema
    let db_info = get_default_pdms_db_info();
    let schemas = db_info.get_all_schemas();
    dbg!(schemas.len());
    let json = serde_json::to_string_pretty(&schemas).unwrap();
    let att_schema_res = client.insert_doc(&json, "admin", "Create element attribute schema", true, false, true).await?;
    dbg!(att_schema_res);
    return Ok(());

}