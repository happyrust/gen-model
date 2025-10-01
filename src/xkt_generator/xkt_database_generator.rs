use crate::xkt_generator::XKTFile;
use anyhow::Result;

pub struct XKTDatabaseGenerator;

impl XKTDatabaseGenerator {
    pub fn new() -> Self {
        Self
    }

    pub async fn generate_from_database(
        &self,
        _dbno: i32,
        _refno: Option<String>,
    ) -> Result<XKTFile> {
        let xkt_file = XKTFile::new();
        Ok(xkt_file)
    }
}
