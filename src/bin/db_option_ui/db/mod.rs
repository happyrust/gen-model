use aios_core::options::DbOption;
use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

const DB_PATH: &str = "deployment_sites.sqlite";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentSite {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub config: DbOption,
    pub created_at: String,
    pub updated_at: String,
}

pub fn init_db() -> Result<Connection> {
    let conn = Connection::open(DB_PATH).with_context(|| format!("无法打开数据库: {}", DB_PATH))?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS deployment_sites (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            description TEXT,
            config_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        [],
    )?;

    Ok(conn)
}

pub fn save_site(name: &str, description: Option<&str>, config: &DbOption) -> Result<String> {
    let conn = init_db()?;
    let site_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Local::now().to_rfc3339();
    let config_json = serde_json::to_string(config)?;

    conn.execute(
        "INSERT INTO deployment_sites (id, name, description, config_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(name) DO UPDATE SET
            description = excluded.description,
            config_json = excluded.config_json,
            updated_at = excluded.updated_at",
        params![site_id, name, description, config_json, now, now],
    )?;

    Ok(site_id)
}

pub fn list_sites() -> Result<Vec<DeploymentSite>> {
    let conn = init_db()?;
    let mut stmt = conn.prepare(
        "SELECT id, name, description, config_json, created_at, updated_at
         FROM deployment_sites
         ORDER BY updated_at DESC",
    )?;

    let sites = stmt
        .query_map([], |row| {
            let config_json: String = row.get(3)?;
            let config: DbOption = serde_json::from_str(&config_json).unwrap_or_default();

            Ok(DeploymentSite {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                config,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(sites)
}

pub fn get_site_by_name(name: &str) -> Result<Option<DeploymentSite>> {
    let conn = init_db()?;
    let mut stmt = conn.prepare(
        "SELECT id, name, description, config_json, created_at, updated_at
         FROM deployment_sites
         WHERE name = ?1",
    )?;

    let mut rows = stmt.query(params![name])?;

    if let Some(row) = rows.next()? {
        let config_json: String = row.get(3)?;
        let config: DbOption = serde_json::from_str(&config_json).unwrap_or_default();

        Ok(Some(DeploymentSite {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            config,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn delete_site(name: &str) -> Result<()> {
    let conn = init_db()?;
    conn.execute(
        "DELETE FROM deployment_sites WHERE name = ?1",
        params![name],
    )?;
    Ok(())
}
