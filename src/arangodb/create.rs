use aios_core::options::DbOption;
use arangors_lite::collection::CollectionType::*;
use crate::graph_db::pdms_arango::{connect_arangodb, create_arango_document};
use crate::consts::*;



/// 提前创建图数据库需要的几个collection
pub async fn create_arangodb_docs(db_option: &DbOption) -> anyhow::Result<()> {
    let pool = connect_arangodb(db_option).await?;
    let database = pool
        .get()
        .await?
        .db(db_option.arangodb_database.as_str())
        .await?;
    create_arango_document(&database, AQL_DATA_ELES_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_DESPARA_ELES_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_FOREIGN_EDGES_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_PDMS_MDBS_EDGES_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_INSTANCE_EDGES_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_PARA_ELES_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_PDMS_EDGES_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_PDMS_ELES_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_PDMS_MESH_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_PDMS_COMPOUND_INST_INFO_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_PDMS_NGMS_INST_INFO_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_PDMS_INST_INFO_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_PDMS_INST_GEO_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_PDMS_INST_TUBI_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_PDMS_INST_EDGE_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_PLIN_ELES_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_SIBL_EDGES_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_SSC_EDGE_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_SSC_ELES_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_TUBI_EDGES_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_ROOM_ELES_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_HOLE_DATA_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_ROOM_EDGES_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_GEO_INFOS_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_HOLE_DATA_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_EMBED_DATA_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_WATER_CALCULATION_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_HOLE_EDGE_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_EMBED_EDGE_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_VIRTUAL_HOLE_COLLECTION, Document).await?;
    Ok(())
}
