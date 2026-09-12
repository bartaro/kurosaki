use thiserror::Error;

pub type Result<T> = std::result::Result<T, KurosakiError>;

#[derive(Debug, Error)]
pub enum KurosakiError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid ROM: {0}")]
    InvalidRom(String),

    #[error("unsupported mapper {mapper}: {reason}")]
    UnsupportedMapper { mapper: u16, reason: String },

    #[error("CPU stopped: {0}")]
    CpuStopped(String),

    #[error("unimplemented opcode {opcode:#04X} at PC {pc:#06X}")]
    UnimplementedOpcode { opcode: u8, pc: u16 },

    #[error("snapshot format mismatch: {0}")]
    SnapshotFormat(String),

    #[error("battery save: {0}")]
    BatterySave(String),

    #[error("battery save unsupported: {0}")]
    BatterySaveUnsupported(String),
}
