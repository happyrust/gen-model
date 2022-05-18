use aios_core::pdms_types::{AiosStr, AttrMap, EleNode, EleNodeTIDB, RefU64};
use parse_pdms_db::db_tool::db1_hash;
use smol_str::SmolStr;
use sqlx::{MySql, Pool, Row};
use crate::database::get_tidb_pool;

fn gen_query_refno_infos_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select project from refno_infos where ref0 = {} limit 1;", refno.get_0()));
    sql
}

pub async fn query_refno_infos(refno: RefU64, pool: Pool<MySql>) -> anyhow::Result<String> {
    let sql = gen_query_refno_infos_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    let val = result.get::<String, _>("project");
    Ok(val)
}

fn gen_query_pdms_elements_type_name_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select type_name from pdms_elements where id = {}", refno.0));
    sql
}

pub async fn query_pdms_elements_type_name(refno: RefU64, pool: Pool<MySql>) -> anyhow::Result<String> {
    let sql = gen_query_pdms_elements_type_name_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(result.get::<String, _>("type_name"))
}

fn gen_query_implicit_attr_sql(refno: RefU64, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select * from {} where id = {}", type_name, refno.0));
    sql
}

// pub async fn query_implicit_attr(refno: RefU64, pool: Pool<MySql>) -> anyhow::Result<AttrMap> {
//     let type_name = query_pdms_elements_type_name(refno, pool.clone()).await?;
//     let sql = gen_query_implicit_attr_sql(refno, &type_name);
//     let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
// }

fn gen_query_world_sql(pool: Pool<MySql>) -> String {
    let mut sql = String::new();
    sql.push_str("select * from worl");
    sql
}

fn gen_pdms_elements_dbno_sql(dbno: u32, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select id, owner ,name from pdms_elements where dbno = {} and type = '{}' ;", dbno, type_name));
    sql
}

fn gen_pdms_elements_get_children_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select id,name,type from pdms_elements where owner = {}", refno.0));
    sql
}

fn gen_pdms_elements_get_all_world_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select id,name,type from pdms_elements where owner = '0/0' ;"));
    sql
}

fn gen_pdms_elements_get_children_count_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select count(*) from pdms_elements where owner = {}", refno.0));
    sql
}

pub async fn query_children_count(refno:RefU64,pool:Pool<MySql>) -> anyhow::Result<usize> {
    let count_sql = gen_pdms_elements_get_children_count_sql(refno);
    let count_result = sqlx::query(&count_sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(count_result.get::<i32,_>(0) as usize)
}

pub async fn query_world(main_db: u32, pool: Pool<MySql>) -> anyhow::Result<EleNodeTIDB> {
    let sql = gen_pdms_elements_dbno_sql(main_db, "WORL");
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    let refno = RefU64(result.get::<i64, _>("id") as u64);

    let count = query_children_count(refno,pool).await?;
    let world = EleNodeTIDB {
        refno,
        owner: RefU64(result.get::<i64, _>("owner") as u64),
        name: AiosStr(SmolStr::new(result.get::<String, _>("name"))),
        noun: AiosStr(SmolStr::new("WORL")),
        version: 0,
        children_count: count as usize,
    };
    Ok(world)
}

pub async fn query_children(refno:RefU64,pool:Pool<MySql>) -> anyhow::Result<Vec<EleNodeTIDB>> {
    let mut r = vec![];
    let sql = gen_pdms_elements_get_children_sql(refno);
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let child_refno = RefU64(val.get::<i64,_>("id") as u64);
        let name = AiosStr(SmolStr::new(val.get::<String,_>("name")));
        let type_name = AiosStr(SmolStr::new(val.get::<String,_>("type")));
        let count = query_children_count(child_refno,pool.clone()).await?;
        r.push(EleNodeTIDB {
            refno:child_refno,
            owner: refno,
            name,
            noun: type_name,
            version: 0,
            children_count: count,
        })
    }
    Ok(r)
}

#[tokio::test]
async fn test_get_world() -> anyhow::Result<()>{
    let url ="mysql://root:root@127.0.0.1:3306";
    let pool = get_tidb_pool(&format!("{}/{}", url, "sample")).await;
    let v = query_world(7600,pool).await?;
    println!("v={:?}",v);
    Ok(())
}

#[tokio::test]
async fn test_get_children() -> anyhow::Result<()>{
    let url ="mysql://root:root@127.0.0.1:3306";
    let info_pool = get_tidb_pool(&format!("{}/{}", url,"refno_infos")).await;
    let refno = RefU64(105548821299203);
    let project = query_refno_infos(refno,info_pool).await?;
    let pool = get_tidb_pool(&format!("{}/{}", url,project)).await;
    let v = query_children(RefU64(105548821299203),pool).await?;
    println!("v={:?}",v);
    Ok(())
}