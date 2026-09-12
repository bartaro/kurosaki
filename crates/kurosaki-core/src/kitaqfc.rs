use crate::error::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KitaqfcDebugBundle {
    pub path: PathBuf,
    pub format_hint: String,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceLocation {
    pub file: String,
    pub line: u32,
    pub function: Option<String>,
}

impl KitaqfcDebugBundle {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path_buf = path.as_ref().to_path_buf();
        let text = fs::read_to_string(&path_buf)?;
        let raw: Value = serde_json::from_str(&text)?;
        let format_hint = raw
            .get("format")
            .and_then(Value::as_str)
            .or_else(|| raw.get("Format").and_then(Value::as_str))
            .unwrap_or("unknown-kitaqfc-json")
            .to_string();
        Ok(Self {
            path: path_buf,
            format_hint,
            raw,
        })
    }

    pub fn function_name_for_pc(&self, _pc: u16) -> Option<String> {
        // Phase 0-3 keeps this conservative because current KITAQFC debug JSON is
        // not yet frozen. Future phases should read function_table.json or
        // *.kqfcdbg.json ranges here.
        None
    }
}
