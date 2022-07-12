
use serde::{Serialize, Deserialize};
use clap::Parser;

#[derive(Debug, Default, Clone, Parser, Serialize, Deserialize)]
pub struct DbOption {
    #[clap(long)]
    pub total_sync: bool,
    #[clap(long)]
    pub incr_sync: bool,
    #[clap(long)]
    pub recreate_db: bool,
    #[clap(long, default_value = "12.1SP4Projects")]
    pub project_path: String,
    //#[clap(long, default_value = "MASTER", "SAMPLE")]
    pub included_projects: Vec<String>,
    #[clap(skip)]
    pub included_db_files: Option<Vec<String>>,  //if none all files parsed, if not, only included parsed
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
    pub debug_cata_refnos: Option<String>,
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
    pub sql_batch_insert_chunk: u32,
    #[clap(short)]
    pub files_multi_thread: bool,
    #[clap(short)]
    pub types_multi_thread: bool,
    #[clap(short)]
    pub batch_insert_handles_chunk: u32,
    #[clap(skip)]
    pub only_save_types_db:Option<Vec<String>>,
}
