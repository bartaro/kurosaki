use crate::cart::RomInfo;
use crate::mapper_db::{mapper_spec, MapperFamily, MapperSupportLevel};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
// Diagnostic importance, serialized as lowercase info/warn/error values.
pub enum Severity {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// A report item with optional source and execution coordinates. These labels
// are supplied by producers, not verified by this data structure.
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub title: String,
    pub message: String,
    pub recommendation: Option<String>,
    pub frame: Option<u64>,
    pub pc: Option<u16>,
    pub function: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// Stored diagnostics and cached severity totals. Mutating public items directly
// can make totals inconsistent; push maintains them for normal additions.
pub struct DiagnosticReport {
    pub format: String,
    pub rom_sha256: String,
    pub emulator: String,
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub items: Vec<Diagnostic>,
}

impl DiagnosticReport {
    // Create an empty v3 report with the ROM fingerprint and crate version.
    // Severity totals start at zero and are maintained by push.
    pub fn new(rom_sha256: impl Into<String>) -> Self {
        Self {
            format: "kurosaki-diagnostics-v3".to_string(),
            rom_sha256: rom_sha256.into(),
            emulator: env!("CARGO_PKG_VERSION").to_string(),
            errors: 0,
            warnings: 0,
            infos: 0,
            items: Vec::new(),
        }
    }

    // Increment the matching severity total and append the diagnostic as supplied.
    // There is no deduplication or validation of its source/timing fields.
    pub fn push(&mut self, item: Diagnostic) {
        match item.severity {
            Severity::Error => self.errors += 1,
            Severity::Warn => self.warnings += 1,
            Severity::Info => self.infos += 1,
        }
        self.items.push(item);
    }

    // Append an informational diagnostic without location or recommendation data.
    pub fn info(&mut self, code: &str, title: impl Into<String>, message: impl Into<String>) {
        self.push(Diagnostic {
            code: code.to_string(),
            severity: Severity::Info,
            title: title.into(),
            message: message.into(),
            recommendation: None,
            frame: None,
            pc: None,
            function: None,
            source: None,
        });
    }

    // Append a warning with an optional recommendation, leaving source and timing
    // fields unset. Use push directly for a diagnostic with those fields populated.
    pub fn warn(
        &mut self,
        code: &str,
        title: impl Into<String>,
        message: impl Into<String>,
        recommendation: Option<String>,
    ) {
        self.push(Diagnostic {
            code: code.to_string(),
            severity: Severity::Warn,
            title: title.into(),
            message: message.into(),
            recommendation,
            frame: None,
            pc: None,
            function: None,
            source: None,
        });
    }

    // Build initial diagnostics from the mapper registry, cartridge flags and
    // header warnings. These observations describe metadata and declared support,
    // not results of executing the ROM or proving mapper accuracy.
    pub fn from_rom_info(info: &RomInfo) -> Self {
        let mut report = Self::new(info.sha256.clone());
        let spec = mapper_spec(info.mapper);
        let family = match spec.family {
            MapperFamily::Unknown => "unknown".to_string(),
            other => format!("{:?}", other).to_lowercase(),
        };
        match spec.support {
            MapperSupportLevel::Implemented => report.info(
                &format!("KS-MAP-{mapper:04}", mapper = info.mapper),
                format!("{} implemented", spec.name),
                format!("Mapper {} is implemented for headless execution, trace, snapshot, and diagnostics. Family: {family}. Notes: {}", info.mapper, spec.notes),
            ),
            MapperSupportLevel::Scaffold => report.warn(
                &format!("KS-MAP-{mapper:04}", mapper = info.mapper),
                format!("{} scaffold", spec.name),
                format!("Mapper {} has a mapper-specific scaffold but is not yet accuracy-complete. Family: {family}. Notes: {}", info.mapper, spec.notes),
                Some("Use mapper trace, state hash, and golden fixtures before treating this mapper as release-grade.".to_string()),
            ),
            MapperSupportLevel::ProbeOnly => report.warn(
                "KS-MAP-PROBE",
                format!("{} probe-only", spec.name),
                format!("Mapper {} is registered, but KUROSAKI will use the generic fixed-window probe mapper. PRG/CHR/IRQ/audio behavior must not be considered accurate. Notes: {}", info.mapper, spec.notes),
                Some("Add a clean-room mapper module plus at least one golden fixture before using run/profile results for this board.".to_string()),
            ),
            MapperSupportLevel::Unknown => report.warn(
                "KS-MAP-UNKNOWN",
                format!("Mapper {} unknown", info.mapper),
                "Mapper number is outside KUROSAKI's current classification table.".to_string(),
                Some("Keep ROM inspection enabled, but do not rely on execution until the board is classified.".to_string()),
            ),
        }
        if spec.has_irq {
            report.info("KS-MAP-FEATURE-IRQ", "Mapper may generate IRQ", "Mapper registry marks this board family as IRQ-capable; trace IRQ lines and snapshot mapper state during tests.");
        }
        if spec.has_expansion_audio {
            report.info("KS-MAP-FEATURE-AUDIO", "Mapper may provide expansion audio", "Mapper registry marks this board family as expansion-audio-capable; APU-only hashes are insufficient for final validation.");
        }
        if info.chr_rom_size == 0 {
            report.info("KS-CHR-0001", "CHR-RAM cartridge", "CHR-RAM writes are routed through mapper write_chr; pixel-accurate rendering is still later work.");
        }
        if info.battery {
            report.info(
                "KS-SAVE-0001",
                "Battery-backed cartridge",
                "PRG-RAM state participates in mapper snapshots where supported.",
            );
        }
        for warning in &info.warnings {
            report.warn("KS-ROM-0001", "ROM header warning", warning.clone(), None);
        }
        report
    }
}
