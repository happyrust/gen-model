//! What an MDB declares, and which of it is on disk.
//!
//! Same resolution `init_mdb` performs, run on its own so the answer can be
//! read before wiring anything to it — in particular to produce the
//! `--dictionary-db-list` for `e3d-descriptor emit-uda-table`, which has to
//! know the Dictionary set and has no way to ask a running service.
//!
//! Reads `DbOption.toml` from the working directory for the project roots, the
//! same as the service.
//!
//! ```text
//! cargo run --release --bin mdb_dict_probe -- --project AvevaMarineSample --mdb /ALL
//! ```

use aios_database::data_interface::mdb_membership::{self, DICT_STYP};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "mdb_dict_probe")]
struct Args {
    #[arg(short, long, default_value = "AvevaMarineSample")]
    project: String,
    #[arg(short, long, default_value = "/ALL")]
    mdb: String,
    /// `STYP` to print a path list for; defaults to Dictionary.
    #[arg(long, default_value_t = DICT_STYP)]
    styp: i64,
}

fn styp_name(styp: i64) -> &'static str {
    match styp {
        1 => "DESI",
        2 => "CATA",
        3 => "PROP",
        4 => "ISOD",
        5 => "PADD",
        7 => "ENGI",
        8 => "DICT",
        14 => "SCHE",
        _ => "?",
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let db_option = aios_core::get_db_option();
    let membership = mdb_membership::resolve(db_option, &args.project, &args.mdb)?;

    println!(
        "{} / {} declares {} databases",
        membership.mdb(),
        membership.project(),
        membership.databases().len()
    );
    for (styp, count) in membership.counts_by_type() {
        println!("  {:<5} ({styp}) x{count}", styp_name(styp));
    }

    println!("\n{} databases:", styp_name(args.styp));
    for database in membership.of_type(args.styp) {
        // A dbnum is unique inside a project only, so the owning project is
        // part of the answer rather than decoration.
        println!(
            "  dbnum={:<8} PROJ={:<3} {:<28} [{}] {}{}",
            database.dbnum,
            database.proj,
            database.name,
            database.project.as_deref().unwrap_or("-"),
            database
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<not found on disk>".into()),
            if database.shadowed.is_empty() {
                String::new()
            } else {
                format!(
                    "  (同号遮蔽 {}: {})",
                    database.shadowed.len(),
                    database
                        .shadowed
                        .iter()
                        .map(|other| other.display().to_string())
                        .collect::<Vec<_>>()
                        .join(" / ")
                )
            }
        );
    }
    for database in membership
        .of_type(args.styp)
        .filter(|database| database.off_declared_project(membership.project()))
    {
        println!(
            "  ! dbnum={} 声明 PROJ={}，实际落在项目 {}",
            database.dbnum,
            database.proj,
            database.project.as_deref().unwrap_or("-")
        );
    }
    for problem in membership.problems() {
        println!("  ! {problem}");
    }

    let paths: Vec<String> = membership
        .of_type(args.styp)
        .filter_map(|database| database.path.as_ref())
        .map(|path| path.display().to_string())
        .collect();
    if !paths.is_empty() {
        println!("\n--dictionary-db-list \"{}\"", paths.join(";"));
    }
    Ok(())
}
