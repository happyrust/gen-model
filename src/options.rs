use serde::{Serialize, Deserialize};
use clap::Parser;

#[derive(Debug, Default, Clone, Parser, Serialize, Deserialize)]
pub struct DbOption {
    #[clap(long)]
    pub total_sync: bool,
    #[clap(long)]
    pub incr_sync: bool,
    #[clap(long)]
    pub replace_dbs: bool,
    #[clap(skip)]
    pub replace_types: Option<Vec<String>>,
    #[clap(long)]
    pub gen_model_mesh: bool,
    #[clap(long)]
    pub save_model_mesh_to_graph_db: bool,
    #[clap(long)]
    pub gen_spatial_tree: bool,
    #[clap(long, default_value = "12.1SP4Projects")]
    pub project_path: String,
    //#[clap(long, default_value = "MASTER", "SAMPLE")]
    pub included_projects: Vec<String>,
    #[clap(skip)]
    pub included_db_files: Option<Vec<String>>,
    #[clap(long)]
    pub mdb_name: String,
    #[clap(long)]
    pub module: String,
    #[clap(long)]
    pub project_name: String,
    #[clap(short)]
    pub debug_db_num: Option<u32>,
    #[clap(skip)]
    pub manual_db_nums: Option<Vec<i32>>,

    #[clap(long)]
    pub debug_print_world_transform: bool,
    #[clap(skip)]
    pub debug_root_refno: Option<Vec<String>>,
    #[clap(skip)]
    pub debug_branch_refno: Option<String>,
    #[clap(skip)]
    pub debug_desi_refno: Option<String>,
    #[clap(skip)]
    pub debug_refno_type: Option<String>,
    #[clap(long)]
    pub replace_mesh: bool,
    #[clap(long)]
    pub only_update_dbinfo: bool,
    #[clap(long)]
    pub ip: String,
    #[clap(long)]
    pub user: String,
    #[clap(long)]
    pub password: String,
    #[clap(long)]
    pub port: String,
    #[clap(short)]
    pub sql_threads_number: u32,
    #[clap(short)]
    pub rebuild_ssc_tree: bool,
    #[clap(short)]
    pub batch_insert_sql_cnt: u32,
    #[clap(short)]
    pub gen_model_batch_size: usize,
    #[clap(long)]
    pub arangodb_url: String,
    #[clap(long)]
    pub server_release_ip: String,
    #[clap(long)]
    pub arangodb_user: String,
    #[clap(long)]
    pub arangodb_password: String,
    #[clap(long)]
    pub arangodb_database: String,
    #[clap(skip)]
    pub withing_room_refnos: Option<String>,
    #[clap(skip)]
    pub arch_db_nums: Option<Vec<i32>>,
    #[clap(long)]
    pub save_spatial_tree_to_db: bool,
    #[clap(long)]
    pub multi_threads: bool,
    #[clap(short)]
    pub only_sync_sys: bool,
}
