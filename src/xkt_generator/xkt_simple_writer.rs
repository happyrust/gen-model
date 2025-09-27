use anyhow::Result;
use byteorder::{LittleEndian, WriteBytesExt};
use std::io::Write;

use crate::xkt_generator::XKTFile;

pub struct XKTSimpleWriter;

impl XKTSimpleWriter {
    pub fn new() -> Self {
        Self
    }

    /// Generate the simplest possible valid XKT v10 file
    pub fn write_to_bytes(&self, _xkt_file: &XKTFile, _compress: bool) -> Result<Vec<u8>> {
        let mut buffer = Vec::new();

        // 1. Write version number (first 4 bytes)
        buffer.write_u32::<LittleEndian>(10)?;

        // 2. Write 20 section offsets (20 * 4 = 80 bytes)
        let base_offset = 4 + 80; // Version + section offsets = 84 bytes

        for i in 0..20 {
            let offset = base_offset + i * 4; // Each section is just 4 bytes for now
            buffer.write_u32::<LittleEndian>(offset as u32)?;
        }

        // 3. Write 20 minimal sections
        for _i in 0..20 {
            // Each section contains a single u32 value (length = 0 for empty sections)
            buffer.write_u32::<LittleEndian>(0)?;
        }

        println!("Generated minimal XKT file: {} bytes", buffer.len());
        Ok(buffer)
    }
}

impl Default for XKTSimpleWriter {
    fn default() -> Self {
        Self::new()
    }
}
