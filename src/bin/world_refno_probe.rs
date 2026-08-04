// Verify patched aios_core get_world_refno / get_world fall through to the first
// parsed (has-WORL) design db by CURD order. Connects to the DbOption SurrealDB,
// calls both for /ALL and prints. Expected on current data (CURD first design db
// 1112 has no data, 8000 has data): returns 8000's WORL (pe:16192_0).

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    aios_core::init_surreal().await?;

    let mdb = "/ALL".to_string();

    let refno = aios_core::get_world_refno(mdb.clone()).await?;
    println!("[world_refno_probe] get_world_refno({}) = {:?}", mdb, refno);

    let world = aios_core::get_world(mdb.clone()).await?;
    println!(
        "[world_refno_probe] get_world({}).refno = {:?}",
        mdb,
        world.map(|w| w.refno)
    );

    Ok(())
}
