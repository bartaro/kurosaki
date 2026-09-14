use crate::bus::BusSnapshot;
use crate::cpu::CpuState;
use crate::mapper::MapperDebugState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
// Serializable execution checkpoint, identified by format and ROM fingerprint.
// The mapper-private payload carries restorable mapper state; its separate hash
// is checked during restore. The readable mapper summary alone is insufficient.
pub struct Snapshot {
    pub format: String,
    pub emulator_version: String,
    pub rom_sha256: String,
    pub frame: u64,
    pub instructions: u64,
    pub cpu: CpuState,
    pub bus: BusSnapshot,
    pub mapper: MapperDebugState,
    pub mapper_private: Vec<u8>,
    pub mapper_private_sha256: String,
    // Retain the observation count as metadata; the full trace event history is not embedded here.
    pub last_event_count: usize,
}
