use sqlx::MySqlPool;

//重新创建database
pub async fn init_database(project: &str) {
    let connection = MySqlPool::connect("mysql://root:@127.0.0.1:4000")
        .await
        .unwrap();
    let mut pool = connection.try_acquire().unwrap();

    let result = sqlx::query(&format!("drop database if exists {project}")).execute(&mut pool).await;
    let result = sqlx::query(&format!("create database {project}")).execute(&mut pool).await;
    let result = sqlx::query(&format!("use {project}")).execute(&mut pool).await;

}