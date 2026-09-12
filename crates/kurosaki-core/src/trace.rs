use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct TraceConfig {
    pub cpu: bool,
    pub mem_read: bool,
    pub mem_write: bool,
    pub ppu: bool,
    pub apu: bool,
    pub mapper: bool,
    pub nmi: bool,
    pub dma: bool,
    pub source: bool,
    pub compact: bool,
}

impl TraceConfig {
    pub fn none() -> Self {
        Self::default()
    }
    pub fn cpu_only() -> Self {
        Self {
            cpu: true,
            ..Self::default()
        }
    }
    pub fn full() -> Self {
        Self {
            cpu: true,
            mem_read: true,
            mem_write: true,
            ppu: true,
            apu: true,
            mapper: true,
            nmi: true,
            dma: true,
            source: true,
            compact: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEvent {
    pub kind: String,
    pub frame: u64,
    pub cpu_cycle: u64,
    pub scanline: Option<i16>,
    pub dot: Option<u16>,
    pub pc: Option<u16>,
    pub prg_bank: Option<u16>,
    pub addr: Option<u16>,
    pub value: Option<u8>,
    pub opcode: Option<u8>,
    pub mnemonic: Option<String>,
    pub message: Option<String>,
    pub severity: Option<String>,
    pub source_file: Option<String>,
    pub source_line: Option<u32>,
    pub source_function: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<BTreeMap<String, Value>>,
}

impl TraceEvent {
    pub fn new(kind: impl Into<String>, frame: u64, cpu_cycle: u64) -> Self {
        Self {
            kind: kind.into(),
            frame,
            cpu_cycle,
            scanline: None,
            dot: None,
            pc: None,
            prg_bank: None,
            addr: None,
            value: None,
            opcode: None,
            mnemonic: None,
            message: None,
            severity: None,
            source_file: None,
            source_line: None,
            source_function: None,
            details: None,
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TraceSink {
    pub events: Vec<TraceEvent>,
}

impl TraceSink {
    pub fn push(&mut self, event: TraceEvent) {
        self.events.push(event);
    }
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
    pub fn to_jsonl(&self) -> crate::Result<String> {
        let mut out = String::new();
        for event in &self.events {
            out.push_str(&serde_json::to_string(event)?);
            out.push('\n');
        }
        Ok(out)
    }
}
