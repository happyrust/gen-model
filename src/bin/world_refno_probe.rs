// Verify patched aios_core get_world_refno / get_world fall through to the first
// parsed (has-WORL) design db by CURD order. Connects to the DbOption SurrealDB,
// calls both for /ALL and prints. Expected on current data (CURD first design db
// 1112 has no data, 8000 has data): returns 8000's WORL (pe:16192_0).

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 按配置连库：探针要看的是服务手上那个库，所以这只在 store_mode=rocksdb 下有意义
    // ——mem 档的库在服务进程里，这里连到的只会是本进程自己的一份空库。
    aios_database::attach_store_connected().await?;

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
