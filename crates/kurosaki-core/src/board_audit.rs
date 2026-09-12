use crate::cart::Cartridge;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const BANK_SIZE: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoardAuditDiagnostic {
    pub code: String,
    pub severity: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardAuditReport {
    pub schema: String,
    pub schema_version: u32,
    pub expected_board: String,
    pub observed_board: Option<String>,
    pub board_profile_source: Option<String>,
    pub board_profile_confidence: Option<f32>,
    pub mapper: u16,
    pub prg_rom_bytes: usize,
    pub chr_rom_bytes: usize,
    pub chr_ram_bytes: usize,
    pub prg_ram_bytes: usize,
    pub battery: bool,
    pub common_physical_banks: Vec<u16>,
    pub common_replica_equal: bool,
    pub vector_replica_equal: bool,
    pub common_replica_sha256: Option<String>,
    pub reset_tail_coverage_odd_banks: bool,
    pub metadata_mapping_equal: Option<bool>,
    pub input_hash_redacted: bool,
    pub pass: bool,
    pub diagnostics: Vec<BoardAuditDiagnostic>,
}

pub fn audit_board(
    cart: &Cartridge,
    expected_board: &str,
    metadata: Option<&Value>,
) -> BoardAuditReport {
    let expected = expected_board.trim().to_ascii_lowercase();
    let prg_ram_bytes = cart.info.prg_ram_size.unwrap_or(0);
    let chr_ram_bytes = cart.info.chr_ram_size.unwrap_or(0);
    let enough_banks = cart.prg_rom.len() >= 32 * BANK_SIZE;
    let (common_replica_equal, vector_replica_equal) = if enough_banks {
        let a = &cart.prg_rom[15 * BANK_SIZE..16 * BANK_SIZE];
        let b = &cart.prg_rom[31 * BANK_SIZE..32 * BANK_SIZE];
        (a == b, a[BANK_SIZE - 6..] == b[BANK_SIZE - 6..])
    } else {
        (false, false)
    };
    let reset_tail_coverage_odd_banks = enough_banks
        && (1..32).step_by(2).all(|bank| {
            let tail = &cart.prg_rom[(bank + 1) * BANK_SIZE - 64..(bank + 1) * BANK_SIZE];
            tail.iter().any(|&byte| byte != 0xff)
        });

    let metadata_mapping_equal = metadata.map(metadata_has_surom_mapping);
    let mut diagnostics = Vec::new();
    let header_matches = cart.info.mapper == 1
        && cart.info.prg_rom_size == 512 * 1024
        && cart.info.chr_rom_size == 0
        && chr_ram_bytes == 8 * 1024
        && prg_ram_bytes == 8 * 1024;
    if cart.info.board_profile.as_deref() == Some("surom512") {
        diagnostics.push(BoardAuditDiagnostic {
            code: "KS-MMC1-BOARD-0001".to_string(),
            severity: "info".to_string(),
            message: "SUROM 512 KiB profile inferred from mapper and memory sizes".to_string(),
        });
    }
    if !header_matches || expected != "surom512" {
        diagnostics.push(BoardAuditDiagnostic {
            code: "FC_SUROM_HEADER_MISMATCH".to_string(),
            severity: "error".to_string(),
            message: "header or requested board does not match the surom512 contract".to_string(),
        });
    }
    if !common_replica_equal {
        diagnostics.push(BoardAuditDiagnostic {
            code: "FC_MMC1_COMMON_REPLICA_DIVERGENCE".to_string(),
            severity: "error".to_string(),
            message: "physical common banks 15 and 31 differ".to_string(),
        });
    }
    if !vector_replica_equal {
        diagnostics.push(BoardAuditDiagnostic {
            code: "FC_SUROM_VECTOR_REPLICA_MISMATCH".to_string(),
            severity: "error".to_string(),
            message: "physical common-bank vectors differ".to_string(),
        });
    }
    if !reset_tail_coverage_odd_banks {
        diagnostics.push(BoardAuditDiagnostic {
            code: "FC_SUROM_VECTOR_REPLICA_MISMATCH".to_string(),
            severity: "error".to_string(),
            message: "reset-tail coverage is incomplete in one or more 32 KiB high banks"
                .to_string(),
        });
    }
    if metadata_mapping_equal == Some(false) {
        diagnostics.push(BoardAuditDiagnostic {
            code: "FC_SUROM_METADATA_MAPPING_MISMATCH".to_string(),
            severity: "error".to_string(),
            message: "build metadata logical-to-physical mapping differs from surom512".to_string(),
        });
    }

    let pass = expected == "surom512"
        && header_matches
        && common_replica_equal
        && vector_replica_equal
        && reset_tail_coverage_odd_banks
        && metadata_mapping_equal != Some(false);
    BoardAuditReport {
        schema: "kurosaki-board-audit".to_string(),
        schema_version: 1,
        expected_board: expected,
        observed_board: cart.info.board_profile.clone(),
        board_profile_source: cart.info.board_profile_source.clone(),
        board_profile_confidence: cart.info.board_profile_confidence,
        mapper: cart.info.mapper,
        prg_rom_bytes: cart.info.prg_rom_size,
        chr_rom_bytes: cart.info.chr_rom_size,
        chr_ram_bytes,
        prg_ram_bytes,
        battery: cart.info.battery,
        common_physical_banks: vec![15, 31],
        common_replica_equal,
        vector_replica_equal,
        common_replica_sha256: None,
        reset_tail_coverage_odd_banks,
        metadata_mapping_equal,
        input_hash_redacted: true,
        pass,
        diagnostics,
    }
}

fn metadata_has_surom_mapping(metadata: &Value) -> bool {
    let Some(layout) = metadata.get("bank_layout") else {
        return false;
    };
    if layout.get("common_physical_banks") != Some(&serde_json::json!([15, 31])) {
        return false;
    }
    let Some(mappings) = layout.get("mappings").and_then(Value::as_array) else {
        return false;
    };
    (1..=30).all(|logical| {
        let expected_physical = if logical <= 15 { logical - 1 } else { logical };
        mappings.iter().any(|item| {
            item.get("logical_bank").and_then(Value::as_i64) == Some(logical)
                && item.get("physical_bank").and_then(Value::as_i64) == Some(expected_physical)
                && item.get("is_common_replica").and_then(Value::as_bool) == Some(false)
        })
    })
}
