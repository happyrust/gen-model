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
    #[clap(long)]
    pub gen_model_mesh: bool,
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
    pub main_db_code: u32,
    #[clap(skip)]
    pub manual_db_nums: Option<Vec<i32>>,

    #[clap(skip)]
    pub debug_branch_refno: Option<String>,

    #[clap(skip)]
    pub debug_desi_refno: Option<String>,

    #[clap(long)]
    pub only_rebuild_pdms_element: bool,
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
    pub arangodb_url:String,
    #[clap(long)]
    pub rebuild_arangodb :bool,
    #[clap(long)]
    pub server_release_ip :String,
    #[clap(long)]
    pub arangodb_user: String,
    #[clap(long)]
    pub arangodb_password: String,
}
