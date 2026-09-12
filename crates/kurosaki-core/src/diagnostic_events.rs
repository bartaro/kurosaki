use crate::diagnostics::{DiagnosticReport, Severity};
use crate::trace::{TraceEvent, TraceSink};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiagnosticEvent {
    pub schema: String,
    pub schema_version: u32,
    pub event_id: String,
    pub event_type: String,
    pub severity: String,
    pub frame: Option<u64>,
    pub scanline: Option<i16>,
    pub dot: Option<u16>,
    pub cpu_cycle: Option<u64>,
    pub pc: Option<String>,
    pub prg_bank: Option<i64>,
    pub addr: Option<String>,
    pub value: Option<String>,
    pub function_hint: Option<String>,
    pub summary_key: String,
    pub count: u64,
    pub first_seen: Option<u64>,
    pub last_seen: Option<u64>,
    pub snapshot_ref: Option<String>,
    pub trace_window_ref: Option<String>,
    pub message: Option<String>,
    pub recommendation: Option<String>,
}

pub fn diagnostic_events_from_trace_and_report(
    trace: &TraceSink,
    report: &DiagnosticReport,
) -> Vec<DiagnosticEvent> {
    let mut builder = DiagnosticEventBuilder::default();
    for event in &trace.events {
        if let Some(mapped) = map_trace_event(event) {
            builder.record(mapped);
        }
    }
    for item in &report.items {
        let mapped = DiagnosticEvent {
            schema: "kurosaki-diagnostic-event".to_string(),
            schema_version: 1,
            event_id: String::new(),
            event_type: diagnostic_to_sarakura_event_type(&item.code, &item.severity).to_string(),
            severity: severity_to_string(&item.severity),
            frame: item.frame,
            scanline: None,
            dot: None,
            cpu_cycle: None,
            pc: item.pc.map(|pc| format!("0x{pc:04X}")),
            prg_bank: Some(0),
            addr: None,
            value: None,
            function_hint: item.function.clone(),
            summary_key: format!("report:{}:{:?}:{:?}", item.code, item.frame, item.pc),
            count: 1,
            first_seen: item.frame,
            last_seen: item.frame,
            snapshot_ref: Some("snapshot.json".to_string()),
            trace_window_ref: Some("trace.jsonl".to_string()),
            message: Some(item.message.clone()),
            recommendation: item.recommendation.clone(),
        };
        builder.record(mapped);
    }
    builder.finish()
}

fn map_trace_event(event: &TraceEvent) -> Option<DiagnosticEvent> {
    let severity = event.severity.as_deref().unwrap_or("info");
    let is_warning = matches!(severity, "warn" | "warning" | "error" | "err");
    if !is_warning {
        return None;
    }
    let event_type = match event.kind.as_str() {
        "mapper.mmc1_write_ignored_consecutive" => "FC_MMC1_CONSECUTIVE_WRITE_IGNORED",
        "mapper.mmc1_chr_mode_unsafe" => "FC_MMC1_CHR_MODE_UNSAFE",
        "mapper.mmc1_prg_ram_disabled_access" => "FC_MMC1_PRG_RAM_DISABLED",
        _ if event.kind.contains("ppu") => {
            if event.addr == Some(0x2005) && event.kind.contains("race") {
                "PPU_SCROLL_UPDATE_RACE"
            } else if event.addr == Some(0x2006) && event.kind.contains("sequence") {
                "PPUADDR_SEQUENCE_BROKEN"
            } else if event.addr == Some(0x2007) {
                "PPU_WRITE_OUTSIDE_VBLANK"
            } else {
                "PPU_REGISTER_WRITE_RISK"
            }
        }
        _ if event.kind.contains("dma") => "OAM_DMA_CONFLICT",
        _ if event.kind.contains("apu") || event.kind.contains("dmc") => {
            "DMC_DMA_PAD_READ_CONFLICT"
        }
        _ if event.kind.contains("mapper") => "BANK_SWITCH_MISMATCH",
        _ => "KUROSAKI_DEBUG_DIAGNOSTIC",
    };
    Some(DiagnosticEvent {
        schema: "kurosaki-diagnostic-event".to_string(),
        schema_version: 1,
        event_id: String::new(),
        event_type: event_type.to_string(),
        severity: normalize_severity(severity).to_string(),
        frame: Some(event.frame),
        scanline: event.scanline,
        dot: event.dot,
        cpu_cycle: Some(event.cpu_cycle),
        pc: event.pc.map(|pc| format!("0x{pc:04X}")),
        prg_bank: event.prg_bank.map(|bank| bank as i64),
        addr: event.addr.map(|addr| format!("0x{addr:04X}")),
        value: event.value.map(|value| format!("0x{value:02X}")),
        function_hint: event.source_function.clone(),
        summary_key: format!(
            "trace:{}:{:?}:{:?}:{:?}",
            event_type, event.pc, event.addr, event.scanline
        ),
        count: 1,
        first_seen: Some(event.frame),
        last_seen: Some(event.frame),
        snapshot_ref: Some("snapshot.json".to_string()),
        trace_window_ref: Some("trace.jsonl".to_string()),
        message: event.message.clone(),
        recommendation: None,
    })
}

fn diagnostic_to_sarakura_event_type(code: &str, severity: &Severity) -> &'static str {
    let upper = code.to_ascii_uppercase();
    if matches!(severity, Severity::Info) {
        if upper.contains("OAM") || upper.contains("DMA") || upper.contains("SPRITE") {
            return "OAM_DMA_OBSERVED";
        }
        if upper.contains("APU") || upper.contains("AUDIO") || upper.contains("DMC") {
            return "APU_AUDIO_OBSERVED";
        }
        if upper.contains("IRQ") {
            return "MAPPER_IRQ_CAPABILITY";
        }
        if upper.contains("FDS") {
            return "FDS_MAPPER_OBSERVED";
        }
        return "KUROSAKI_DEBUG_DIAGNOSTIC";
    }

    if upper.contains("PPU-0002") || upper.contains("VRAM") {
        "PPU_WRITE_OUTSIDE_VBLANK"
    } else if upper.contains("PPU-0003") || upper.contains("PPU-0004") || upper.contains("SCROLL") {
        "PPU_REGISTER_WRITE_RISK"
    } else if upper.contains("PALETTE") {
        "PALETTE_WRITE_OUTSIDE_SAFE_PERIOD"
    } else if upper.contains("OAM") || upper.contains("DMA") || upper.contains("SPRITE") {
        "OAM_DMA_CONFLICT"
    } else if upper.contains("DMC") || upper.contains("JOY") || upper.contains("PAD") {
        "DMC_DMA_PAD_READ_CONFLICT"
    } else if upper.contains("MMC3") || upper.contains("IRQ") {
        "MMC3_IRQ_TIMING_RISK"
    } else if upper.contains("FDS") {
        "FDS_WAVE_WRITE_TIMING_RISK"
    } else if upper.contains("VRC6") {
        "VRC6_AUDIO_UPDATE_JITTER"
    } else if upper.contains("VRC7") {
        "VRC7_REGISTER_WRITE_RISK"
    } else if upper.contains("MAPPER") || upper.contains("BANK") || upper.contains("CPU") {
        "BANK_SWITCH_MISMATCH"
    } else if upper.contains("APU") || upper.contains("AUDIO") {
        "DMC_DMA_PAD_READ_CONFLICT"
    } else {
        "KUROSAKI_DEBUG_DIAGNOSTIC"
    }
}

fn severity_to_string(severity: &Severity) -> String {
    match severity {
        Severity::Info => "info",
        Severity::Warn => "warn",
        Severity::Error => "error",
    }
    .to_string()
}

fn normalize_severity(severity: &str) -> &str {
    match severity {
        "error" | "err" => "error",
        "warn" | "warning" => "warn",
        _ => "info",
    }
}

#[derive(Default)]
struct DiagnosticEventBuilder {
    next_id: u64,
    events: Vec<DiagnosticEvent>,
    index_by_summary_key: BTreeMap<String, usize>,
}

impl DiagnosticEventBuilder {
    fn record(&mut self, mut event: DiagnosticEvent) {
        if let Some(index) = self.index_by_summary_key.get(&event.summary_key).copied() {
            let existing = &mut self.events[index];
            existing.count = existing.count.saturating_add(event.count);
            existing.first_seen = min_opt(existing.first_seen, event.first_seen);
            existing.last_seen = max_opt(existing.last_seen, event.last_seen);
            return;
        }
        self.next_id = self.next_id.saturating_add(1);
        event.event_id = format!("kurosaki_evt_{:06}", self.next_id);
        self.index_by_summary_key
            .insert(event.summary_key.clone(), self.events.len());
        self.events.push(event);
    }

    fn finish(self) -> Vec<DiagnosticEvent> {
        self.events
    }
}

fn min_opt(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn max_opt(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

#[cfg(test)]
mod raster_mapping_tests {
    use super::*;
    #[test]
    fn visible_register_write_is_a_candidate_not_a_proven_sequence_failure() {
        let mut event = TraceEvent::new("ppu.reg_write", 1, 100);
        event.severity = Some("warn".into());
        event.addr = Some(0x2006);
        assert_eq!(
            map_trace_event(&event).unwrap().event_type,
            "PPU_REGISTER_WRITE_RISK"
        );
        event.addr = Some(0x2005);
        assert_eq!(
            map_trace_event(&event).unwrap().event_type,
            "PPU_REGISTER_WRITE_RISK"
        );
        event.addr = Some(0x2007);
        assert_eq!(
            map_trace_event(&event).unwrap().event_type,
            "PPU_WRITE_OUTSIDE_VBLANK"
        );
    }
}
