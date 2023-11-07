use std::fs::File;
use std::ops::Add;
use aios_core::pdms_types::EleOperation;
use chrono::Days;
use serde::{Deserialize, Serialize};
use once_cell::sync::Lazy;
use surrealdb::engine::local::{Db, RocksDb};
use surrealdb::sql::Thing;
use surrealdb::Surreal;
use crate::data_interface::increment_record::IncrUpdateLog;

static DB: Lazy<Surreal<Db>> = Lazy::new(Surreal::init);

#[derive(Debug, Serialize, Deserialize)]
struct Name<'a> {
    first: &'a str,
    last: &'a str,
}

#[derive(Debug, Serialize, Deserialize)]
struct Person<'a> {
    title: &'a str,
    name: Name<'a>,
    marketing: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct Responsibility {
    marketing: bool,
}

#[derive(Debug, Deserialize)]
struct Record {
    #[allow(dead_code)]
    id: Thing,
}

#[tokio::test]
async fn test_save_incr_log() -> surrealdb::Result<()> {
    // Connect to the database
    let db = Surreal::new::<RocksDb>("~/work/gen-model/database.sdb").await.unwrap();
    db.use_ns("ams").use_db("incr").await.unwrap();

    let logs: Vec<IncrUpdateLog> = db.delete("incr_log").await?;

    let mut log_data = IncrUpdateLog {
        refno: "1/1".into(),
        data_operate: EleOperation::Add,
        ..Default::default()
    };

    let created: Option<Record> = db
        .create(("incr_log", *log_data.refno))
        .content(log_data)
        .await.unwrap();
    dbg!(&created);

    let mut date = surrealdb::sql::Datetime::default();
    let new_date = date.checked_add_days(Days::new(1)).unwrap();
    let mut log_data = IncrUpdateLog {
        refno: "1/2".into(),
        data_operate: EleOperation::Add,
        timestamp: new_date.into(),
        ..Default::default()
    };

    let created: Vec<Record> = db
        .create("incr_log")
        .content(log_data)
        .await.unwrap();
    dbg!(&created);


    return Ok(());
}


#[tokio::test]
async fn test_db() -> surrealdb::Result<()> {
    // Connect to the database
    let db = Surreal::new::<RocksDb>("~/work/gen-model/test.sdb").await?;
    db.use_ns("test").use_db("test").await?;
    let created: Vec<Record> = db
        .create("person")
        .content(Person {
            title: "Founder & CEO",
            name: Name {
                first: "Tobie",
                last: "Morgan Hitchcock",
            },
            marketing: true,
        })
        .await?;
    dbg!(&created);

    // Update a person record with a specific id
    let updated: Option<Record> = db
        .update(("person", created[0].id.id.clone()))
        .merge(Responsibility { marketing: false })
        .await?;
    dbg!(updated);

    // Select all people records
    let people: Vec<Record> = db.select("person").await?;
    dbg!(people);

    // Perform a custom advanced query
    let groups = db
        .query("SELECT marketing, count() FROM type::table($table) GROUP BY marketing")
        .bind(("table", "person"))
        .await?;
    dbg!(groups);

    let mut result = db
        .query("SELECT marketing, name FROM type::table($table)")
        .bind(("table", "person"))
        .await?;
    dbg!(&result);
    // let names: Vec<Name> = result.take("name")?;
    // dbg!(&names);
    // let r0: Vec<bool> = result.take("marketing")?;
    // dbg!(&r0);


    Ok(())
}