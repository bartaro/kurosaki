use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
// Controller values and optional actions associated with a frame interval.
// This record only stores requests; the replay consumer decides how to apply them.
pub struct ReplayFrame {
    pub frame: u64,
    pub pad1: u8,
    pub pad2: u8,
    pub reset: bool,
    pub disk_side: Option<u8>,
    pub expected_frame_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// Input sequence metadata and optional expected hashes. Deserialization alone
// does not verify ROM identity, region compatibility or any expected result.
pub struct Replay {
    pub format: String,
    pub rom_sha256: String,
    pub emulator_version: String,
    pub region: String,
    pub frames: Vec<ReplayFrame>,
    pub expected_final_state_hash: Option<String>,
}

impl Replay {
    // Create an empty v1 replay for the supplied ROM fingerprint, recording this
    // crate version and the default NTSC region. Expected result hashes are unset.
    pub fn empty(rom_sha256: impl Into<String>) -> Self {
        Self {
            format: "kurosaki-replay-v1".to_string(),
            rom_sha256: rom_sha256.into(),
            emulator_version: env!("CARGO_PKG_VERSION").to_string(),
            region: "ntsc".to_string(),
            frames: Vec::new(),
            expected_final_state_hash: None,
        }
    }
}

impl ReplayFrame {
    // Treat an absent or zero duration as one frame and saturate the exclusive
    // end at u64::MAX. A start at that limit therefore has no representable interval.
    pub fn end_frame_exclusive(&self) -> u64 {
        self.frame.saturating_add(self.duration.unwrap_or(1).max(1))
    }

    // Test membership in the half-open frame interval. This does not resolve
    // overlapping replay entries or apply their input/reset/disk fields.
    pub fn covers(&self, frame: u64) -> bool {
        self.frame <= frame && frame < self.end_frame_exclusive()
    }
}
