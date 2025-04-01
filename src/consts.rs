pub const ARANGODB_SAVE_AMOUNT: usize = 10000;
pub const PDMS_INFO_DB: &'static str = "PDMS_INFO_DB";

pub const PDMS_EXPLICIT_TABLE: &'static str = "EXPLICIT_ATT";
pub const PDMS_UDA_ATT_TABLE: &'static str = "UDA_ATT";
pub const PDMS_UDA_TABLE: &'static str = "UDA";
pub const PDMS_UDET_TABLE: &'static str = "UDET";
pub const PDMS_FILE_VERSION_TABLE: &'static str = "FILE_VERSION";
pub const PDMS_ELEMENTS_TABLE: &'static str = "PDMS_ELEMENTS";
pub const PDMS_SSC_ELEMENTS_TABLE: &'static str = "PDMS_SSC_ELEMENTS";
pub const PDMS_REFNO_INFOS_TABLE: &'static str = "REFNO_INFOS";
pub const PDMS_DBNO_INFOS_TABLE: &'static str = "DBNO_INFOS";
pub const PDMS_PROJECT_MDB_TABLE: &'static str = "PROJECT_MDB";
pub const PDMS_DATA_STATE: &'static str = "DATA_STATE";
pub const INCREMENT_DATA: &'static str = "INCREMENT_DATA";
pub const PDMS_PROJECT_MDB_TABLE_JSON: &'static str = "PROJECT_MDB_JSON";
pub const ROOM_CODE: &'static str = "ROOM_CODE";
pub const VERSION_DATA: &'static str = "VERSION_DATA";
pub const PDMS_VERSION: &'static str = "PDMS_VERSION";
pub const PDMS_MESH: &'static str = "PDMS_MESH";
pub const METADATA_TABLE: &'static str = "METADATA_MANAGE";
pub const METADATA_DATA: &'static str = "METADATA_DATA";
pub const HOLES_TABLE: &'static str = "HOLEDATATABLE";
pub const EMBED_TABLE: &'static str = "EMBEDDATATABLE";
pub const SCTN_STANDARD: &'static str = "SCTN_STANDARD";
pub const CHANNEL_STEEL_STANDARD: &'static str = "CHANNEL_STEEL_STANDARD";
pub const GLOBAL_DATABASE: &'static str = "PROJECT_INFO";
// pub const SSC_DATABASE: &'static str = "SSC_INFO";
pub const FUZZY_QUERT: &'static str = "FUZZY_QUERY";
pub const PUHUA_MATERIAL_DATABASE: &'static str = "hdey_powerpmdb";
pub const PUHUA_GY_MATERIAL_TABLE: &'static str = "pdms_mat_gy";
pub const PUHUA_DQ_MATERIAL_TABLE: &'static str = "pdms_traymotlist";
// project_mdb
// 专业表单
pub const PROFESSION_TX_XT: &'static str = "TX_XT";
pub const TEAM_DATA_TABLE: &'static str = "TEAM_DATA";
pub const UNSET_NOUN: u32 = 28933118;

/// 批量块数量
pub const BATCH_CHUNKS_CNT: usize = 50;

///mdb下的数据关联关系
pub const AQL_PDMS_MDBS_EDGES_COLLECTION: &'static str = "pdms_mdbs";

/// pdms 的基本 element数据
pub const AQL_PDMS_ELES_COLLECTION: &'static str = "pdms_eles";
/// pdms 的 mesh数据
pub const AQL_PDMS_MESH_COLLECTION: &'static str = "pdms_mesh";
pub const AQL_PDMS_EDGES_COLLECTION: &'static str = "pdms_edges";
/// 负实体运算后的inst infos数据
pub const AQL_PDMS_COMPOUND_INST_INFO_COLLECTION: &'static str = "pdms_compound_inst_infos";
pub const AQL_PDMS_NGMS_INST_INFO_COLLECTION: &'static str = "pdms_ngmr_inst_infos";
pub const AQL_PDMS_INST_INFO_COLLECTION: &'static str = "pdms_inst_infos";
/// 负实体运算后的inst geos数据
pub const AQL_PDMS_COMPOUND_INST_GEO_COLLECTION: &'static str = "pdms_compound_inst_geos";

/// 运算后的compound由哪些组成的边的关系
pub const AQL_PDMS_COMPOUND_INST_EDGE_COLLECTION: &'static str = "pdms_compound_inst_geos";

pub const AQL_PDMS_INST_GEO_COLLECTION: &'static str = "pdms_inst_geos";
pub const AQL_PDMS_INST_TUBI_COLLECTION: &'static str = "pdms_inst_tubis";
pub const AQL_PDMS_INST_EDGE_COLLECTION: &'static str = "pdms_inst_edges";
pub const AQL_SSC_ELES_COLLECTION: &'static str = "ssc_eles";
pub const AQL_SSC_EDGE_COLLECTION: &'static str = "ssc_edges";
pub const AQL_THREED_REVIEW_COLLECTION: &'static str = "threed_review";
pub const AQL_VIRTUAL_HOLE_COLLECTION: &'static str = "virtual_hole";
pub const AQL_SIBL_EDGES_COLLECTION: &'static str = "sibl_edges";
pub const AQL_FOREIGN_EDGES_COLLECTION: &'static str = "foreign_edges";
pub const AQL_GEO_INFOS_COLLECTION: &'static str = "geo_infos";
pub const AQL_ROOM_EDGES_COLLECTION: &'static str = "room_edges";
pub const AQL_EMBED_DATA_COLLECTION: &'static str = "embed_data";
pub const AQL_ROOM_ELES_COLLECTION: &'static str = "room_eles";
pub const AQL_TUBI_EDGES_COLLECTION: &'static str = "tubi_edges";
pub const AQL_PLIN_ELES_COLLECTION: &'static str = "plin_eles";
pub const AQL_PARA_ELES_COLLECTION: &'static str = "para_eles";
pub const AQL_INSTANCE_EDGES_COLLECTION: &'static str = "instance_edges";
pub const AQL_DESPARA_ELES_COLLECTION: &'static str = "despara_eles";
pub const AQL_DATA_ELES_COLLECTION: &'static str = "data_eles";
pub const AQL_LOCK_REFNOS_COLLECTION: &'static str = "lock_refnos";
pub const AQL_HOLE_DATA_COLLECTION: &'static str = "hole_data";
pub const AQL_HOLE_EDGE_COLLECTION: &'static str = "hole_edge";
pub const AQL_EMBED_EDGE_COLLECTION: &'static str = "embed_edge";
pub const AQL_WATER_CALCULATION_COLLECTION: &'static str = "water_calculation";

pub const DESI: &'static str = "DESI";
