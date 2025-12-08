use aios_core::{init_surreal, SUL_DB};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_surreal().await?;
    
    println!("--- 查找 ELBO ---");
    let sql = r#"SELECT id, noun FROM pe WHERE noun = 'ELBO' LIMIT 10"#;
    let resp = SUL_DB.query(sql).await?;
    println!("{:?}", resp);
    
    Ok(())
}
