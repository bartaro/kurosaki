use crate::bus::BusSnapshot;
use crate::cpu::CpuState;
use crate::mapper::MapperDebugState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub last_event_count: usize,
}
