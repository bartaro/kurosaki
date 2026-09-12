//! Static, mapper-context-aware 2A03 decompilation support.
//!
//! This module deliberately reports uncertainty instead of pretending that a
//! mapper write can be resolved statically.  A report is tied to the PRG-bank
//! windows visible at reset or in a supplied snapshot; trace overlays can then
//! show which physical-bank/PC pairs actually ran.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::cart::Cartridge;
use crate::cpu::opcode_mnemonic;
use crate::emulator::Emulator;
use crate::error::Result;
use crate::trace::{TraceConfig, TraceEvent};

const PRG_BANK_SIZE: usize = 8 * 1024;
const DEFAULT_MAX_INSTRUCTIONS_PER_FUNCTION: usize = 2_048;

#[derive(Debug, Clone, Copy, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct DecompileAddress {
    pub prg_bank_8k: u16,
    pub cpu_addr: u16,
}

impl DecompileAddress {
    pub fn id(self) -> String {
        format!("B{:03}:${:04X}", self.prg_bank_8k, self.cpu_addr)
    }
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct DecompileLabel {
    pub prg_bank_8k: u16,
    pub cpu_addr: u16,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct DecompileXref {
    pub from: DecompileAddress,
    pub to: Option<DecompileAddress>,
    pub target_cpu_addr: u16,
    pub kind: String,
    pub confidence: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct DecompileInstruction {
    pub address: DecompileAddress,
    pub file_offset: usize,
    pub bytes: Vec<u8>,
    pub mnemonic: String,
    pub operand_text: String,
    pub text: String,
    pub flow_kind: String,
    pub target: Option<DecompileAddress>,
    pub target_cpu_addr: Option<u16>,
    pub fallthrough: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct DecompileBasicBlock {
    pub id: String,
    pub start: DecompileAddress,
    pub end: DecompileAddress,
    pub instructions: Vec<DecompileInstruction>,
    pub predecessors: Vec<String>,
    pub successors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_role: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Serialize)]
pub struct DecompileTraceSummary {
    pub hit_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_frame: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_frame: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_cpu_cycle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_cpu_cycle: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hot_pcs: Vec<DecompileTracePcHit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observed_sources: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct DecompileTracePcHit {
    pub address: DecompileAddress,
    pub hit_count: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct DecompileArtifact {
    pub disassembly_text: String,
    pub pseudocode_text: String,
    pub warnings: Vec<String>,
    pub confidence_score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_summary: Option<DecompileTraceSummary>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct DecompileFunction {
    pub id: String,
    pub start: DecompileAddress,
    pub end: DecompileAddress,
    pub name: String,
    pub canonical_name: String,
    pub source_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calling_convention_guess: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub user_notes: Vec<String>,
    pub blocks: Vec<DecompileBasicBlock>,
    pub xrefs_in: Vec<DecompileXref>,
    pub xrefs_out: Vec<DecompileXref>,
    pub warnings: Vec<String>,
    pub confidence_score: f32,
    pub artifact: DecompileArtifact,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct DecompileReport {
    pub schema_version: String,
    pub mapper: u16,
    pub mapper_name: String,
    pub prg_rom_size_bytes: usize,
    pub prg_bank_count_8k: u16,
    pub analysis_bank_windows: Vec<Option<u16>>,
    pub functions: Vec<DecompileFunction>,
    pub xrefs: Vec<DecompileXref>,
    pub labels: Vec<DecompileLabel>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DecompileOptions {
    pub selected_functions: Vec<String>,
    /// Adds a bounded, physical-bank scan in addition to vector/call roots.
    pub include_all_prg_banks: bool,
    pub max_instructions_per_function: usize,
}

impl Default for DecompileOptions {
    fn default() -> Self {
        Self {
            selected_functions: Vec::new(),
            include_all_prg_banks: false,
            max_instructions_per_function: DEFAULT_MAX_INSTRUCTIONS_PER_FUNCTION,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DecompileAnnotationFile {
    #[serde(default = "default_annotation_schema")]
    pub schema_version: String,
    #[serde(default)]
    pub function_overrides: Vec<DecompileFunctionOverride>,
    #[serde(default)]
    pub label_overrides: Vec<DecompileLabelOverride>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct DecompileFunctionOverride {
    pub selector: String,
    #[serde(default)]
    pub rename: Option<String>,
    #[serde(default)]
    pub calling_convention: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct DecompileLabelOverride {
    pub prg_bank_8k: u16,
    pub cpu_addr: u16,
    pub name: String,
    #[serde(default = "default_label_kind")]
    pub kind: String,
}

fn default_annotation_schema() -> String {
    "kurosaki-decompile-annotations-v1".to_string()
}

fn default_label_kind() -> String {
    "user".to_string()
}

#[derive(Debug, Clone)]
struct AddressMapping {
    windows: [Option<u16>; 4],
}

impl AddressMapping {
    fn from_emulator(emulator: &Emulator) -> Self {
        Self {
            windows: [0x8000, 0xA000, 0xC000, 0xE000]
                .map(|address| emulator.bus.mapper.physical_prg_bank_8k(address)),
        }
    }

    fn map_cpu_addr(&self, cpu_addr: u16) -> Option<DecompileAddress> {
        if cpu_addr < 0x8000 {
            return None;
        }
        let index = usize::from((cpu_addr - 0x8000) / 0x2000);
        self.windows
            .get(index)
            .copied()
            .flatten()
            .map(|prg_bank_8k| DecompileAddress {
                prg_bank_8k,
                cpu_addr,
            })
    }

    fn resolve_target(
        &self,
        source: DecompileAddress,
        target_cpu_addr: u16,
    ) -> Option<DecompileAddress> {
        if target_cpu_addr < 0x8000 {
            return None;
        }
        if source.cpu_addr & 0xE000 == target_cpu_addr & 0xE000 {
            return Some(DecompileAddress {
                prg_bank_8k: source.prg_bank_8k,
                cpu_addr: target_cpu_addr,
            });
        }
        self.map_cpu_addr(target_cpu_addr)
    }

    fn read_byte(&self, cart: &Cartridge, source: DecompileAddress, cpu_addr: u16) -> Option<u8> {
        let address = if source.cpu_addr & 0xE000 == cpu_addr & 0xE000 {
            DecompileAddress {
                prg_bank_8k: source.prg_bank_8k,
                cpu_addr,
            }
        } else {
            self.map_cpu_addr(cpu_addr)?
        };
        read_physical_prg_byte(cart, address)
    }
}

#[derive(Debug, Clone)]
struct FunctionAnalysis {
    start: DecompileAddress,
    name: String,
    source_kind: String,
    instructions: BTreeMap<DecompileAddress, DecompileInstruction>,
    xrefs_out: Vec<DecompileXref>,
    child_roots: Vec<DecompileAddress>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum FlowKind {
    Fallthrough,
    Call,
    Jump,
    ConditionalBranch,
    Return,
    Stop,
    Unknown,
}

impl FlowKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fallthrough => "fallthrough",
            Self::Call => "call",
            Self::Jump => "jump",
            Self::ConditionalBranch => "conditional_branch",
            Self::Return => "return",
            Self::Stop => "stop",
            Self::Unknown => "unknown",
        }
    }
}

/// Analyze a ROM using the mapper state immediately after reset.
pub fn analyze_cartridge(cart: &Cartridge, options: &DecompileOptions) -> Result<DecompileReport> {
    let mut emulator = Emulator::from_cartridge(cart.clone())?;
    emulator.reset(TraceConfig::none());
    analyze_emulator(&emulator, options)
}

/// Analyze a ROM using the mapper state restored in `emulator`.
///
/// Call this variant after `Emulator::from_snapshot` for bank-switched games.
pub fn analyze_emulator(
    emulator: &Emulator,
    options: &DecompileOptions,
) -> Result<DecompileReport> {
    let mapping = AddressMapping::from_emulator(emulator);
    Ok(analyze_with_mapping(
        &emulator.cartridge,
        &mapping,
        emulator.cpu.pc,
        emulator.bus.mapper.mapper_name(),
        options,
    ))
}

fn analyze_with_mapping(
    cart: &Cartridge,
    mapping: &AddressMapping,
    reset_pc: u16,
    mapper_name: &str,
    options: &DecompileOptions,
) -> DecompileReport {
    let mut roots: BTreeMap<DecompileAddress, (String, String)> = BTreeMap::new();
    insert_root(
        &mut roots,
        mapping.map_cpu_addr(reset_pc),
        "Reset".to_string(),
        "reset_vector".to_string(),
    );
    for (vector, name, kind) in [
        (0xFFFA, "Nmi", "nmi_vector"),
        (0xFFFC, "Reset", "reset_vector"),
        (0xFFFE, "Irq", "irq_vector"),
    ] {
        let target = mapped_u16(cart, mapping, vector);
        insert_root(
            &mut roots,
            target.and_then(|address| mapping.map_cpu_addr(address)),
            name.to_string(),
            kind.to_string(),
        );
    }
    for selector in &options.selected_functions {
        if let Some(address) = parse_address_selector(selector) {
            insert_root(
                &mut roots,
                Some(address),
                auto_function_name(address),
                "selected".to_string(),
            );
        }
    }
    if options.include_all_prg_banks {
        for prg_bank_8k in 0..prg_bank_count(cart) {
            let address = DecompileAddress {
                prg_bank_8k,
                cpu_addr: 0x8000,
            };
            insert_root(
                &mut roots,
                Some(address),
                auto_function_name(address),
                "physical_bank_scan".to_string(),
            );
        }
    }

    let mut pending: VecDeque<DecompileAddress> = roots.keys().copied().collect();
    let mut analyzed: BTreeMap<DecompileAddress, FunctionAnalysis> = BTreeMap::new();
    while let Some(start) = pending.pop_front() {
        if analyzed.contains_key(&start) || !physical_address_is_valid(cart, start) {
            continue;
        }
        let (name, source_kind) = roots
            .get(&start)
            .cloned()
            .unwrap_or_else(|| (auto_function_name(start), "inferred".to_string()));
        let analysis = analyze_function(cart, mapping, start, name, source_kind, options);
        for child in &analysis.child_roots {
            if physical_address_is_valid(cart, *child) {
                roots
                    .entry(*child)
                    .or_insert_with(|| (auto_function_name(*child), "call_target".to_string()));
                if !analyzed.contains_key(child) {
                    pending.push_back(*child);
                }
            }
        }
        analyzed.insert(start, analysis);
    }

    let mut all_analyses: Vec<FunctionAnalysis> = analyzed.into_values().collect();
    all_analyses.sort_by_key(|function| function.start);

    let mut labels = BTreeMap::<DecompileAddress, DecompileLabel>::new();
    for function in &all_analyses {
        labels.insert(
            function.start,
            DecompileLabel {
                prg_bank_8k: function.start.prg_bank_8k,
                cpu_addr: function.start.cpu_addr,
                name: function.name.clone(),
                kind: "function".to_string(),
            },
        );
        for xref in &function.xrefs_out {
            if let Some(target) = xref.to {
                labels.entry(target).or_insert_with(|| DecompileLabel {
                    prg_bank_8k: target.prg_bank_8k,
                    cpu_addr: target.cpu_addr,
                    name: auto_label_name(target),
                    kind: if xref.kind == "call" {
                        "function_candidate".to_string()
                    } else {
                        "code_target".to_string()
                    },
                });
            }
        }
    }

    let mut xrefs = all_analyses
        .iter()
        .flat_map(|function| function.xrefs_out.iter().cloned())
        .collect::<Vec<_>>();
    xrefs.sort_by_key(|xref| (xref.from, xref.to, xref.target_cpu_addr, xref.kind.clone()));
    xrefs.dedup();
    let xrefs_in = build_xrefs_in(&xrefs);

    let mut functions = all_analyses
        .into_iter()
        .map(|analysis| build_function(analysis, &xrefs_in, &labels))
        .collect::<Vec<_>>();
    functions.sort_by_key(|function| function.start);
    if !options.selected_functions.is_empty() && !options.include_all_prg_banks {
        functions.retain(|function| {
            options
                .selected_functions
                .iter()
                .any(|selector| function_selector_matches(function, selector))
        });
    }

    let mut notes = vec![
        "Static 2A03 decompilation is tied to the four physical 8 KiB PRG windows visible at reset or in the supplied snapshot.".to_string(),
        "Direct branches inside one 8 KiB window are resolved exactly; cross-window calls use the captured mapper window and mapper-write-dependent targets remain uncertain.".to_string(),
        "Pseudocode is a readable control-flow reconstruction, not compilable source or a proof of original source intent.".to_string(),
    ];
    if options.include_all_prg_banks {
        notes.push("--all scanned each physical PRG bank from CPU $8000; data banks can therefore yield low-confidence candidates.".to_string());
    }
    DecompileReport {
        schema_version: "kurosaki-decompile-v1".to_string(),
        mapper: cart.info.mapper,
        mapper_name: mapper_name.to_string(),
        prg_rom_size_bytes: cart.prg_rom.len(),
        prg_bank_count_8k: prg_bank_count(cart),
        analysis_bank_windows: mapping.windows.into_iter().collect(),
        functions,
        xrefs,
        labels: labels.into_values().collect(),
        notes,
    }
}

/// Apply KOKURA-style rename, calling-convention, note, and label overrides.
pub fn apply_annotations(
    report: &mut DecompileReport,
    annotations: &DecompileAnnotationFile,
) -> usize {
    let mut applied = 0usize;
    for function in &mut report.functions {
        for override_spec in &annotations.function_overrides {
            if !function_selector_matches(function, &override_spec.selector) {
                continue;
            }
            if let Some(rename) = &override_spec.rename {
                if function.name != *rename {
                    function.name = rename.clone();
                    applied += 1;
                }
            }
            if let Some(calling_convention) = &override_spec.calling_convention {
                if function.calling_convention_guess.as_deref() != Some(calling_convention.as_str())
                {
                    function.calling_convention_guess = Some(calling_convention.clone());
                    applied += 1;
                }
            }
            for note in &override_spec.notes {
                if !function.user_notes.contains(note) {
                    function.user_notes.push(note.clone());
                    applied += 1;
                }
            }
        }
    }
    for override_label in &annotations.label_overrides {
        let address = DecompileAddress {
            prg_bank_8k: override_label.prg_bank_8k,
            cpu_addr: override_label.cpu_addr,
        };
        if let Some(existing) = report.labels.iter_mut().find(|label| {
            label.prg_bank_8k == address.prg_bank_8k && label.cpu_addr == address.cpu_addr
        }) {
            if existing.name != override_label.name || existing.kind != override_label.kind {
                existing.name = override_label.name.clone();
                existing.kind = override_label.kind.clone();
                applied += 1;
            }
        } else {
            report.labels.push(DecompileLabel {
                prg_bank_8k: address.prg_bank_8k,
                cpu_addr: address.cpu_addr,
                name: override_label.name.clone(),
                kind: override_label.kind.clone(),
            });
            applied += 1;
        }
    }
    for note in &annotations.notes {
        if !report.notes.contains(note) {
            report.notes.push(note.clone());
            applied += 1;
        }
    }
    refresh_artifacts(report);
    applied
}

/// Overlay CPU instruction events from KUROSAKI `trace` JSONL output.
pub fn apply_trace_events(report: &mut DecompileReport, events: &[TraceEvent]) -> usize {
    let mut matched = 0usize;
    for function in &mut report.functions {
        let code = function
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .map(|instruction| instruction.address)
            .collect::<BTreeSet<_>>();
        let mut summary = DecompileTraceSummary::default();
        let mut hot = BTreeMap::<DecompileAddress, u64>::new();
        let mut sources = BTreeSet::new();
        for event in events {
            if event.kind != "cpu.instruction" {
                continue;
            }
            let (Some(cpu_addr), Some(prg_bank_8k)) = (event.pc, event.prg_bank) else {
                continue;
            };
            let address = DecompileAddress {
                prg_bank_8k,
                cpu_addr,
            };
            if !code.contains(&address) {
                continue;
            }
            matched += 1;
            summary.hit_count += 1;
            summary.first_frame = Some(
                summary
                    .first_frame
                    .map_or(event.frame, |value| value.min(event.frame)),
            );
            summary.last_frame = Some(
                summary
                    .last_frame
                    .map_or(event.frame, |value| value.max(event.frame)),
            );
            summary.first_cpu_cycle = Some(
                summary
                    .first_cpu_cycle
                    .map_or(event.cpu_cycle, |value| value.min(event.cpu_cycle)),
            );
            summary.last_cpu_cycle = Some(
                summary
                    .last_cpu_cycle
                    .map_or(event.cpu_cycle, |value| value.max(event.cpu_cycle)),
            );
            *hot.entry(address).or_default() += 1;
            if let (Some(file), Some(line)) = (&event.source_file, event.source_line) {
                sources.insert(format!("{file}:{line}"));
            }
        }
        if summary.hit_count != 0 {
            summary.hot_pcs = hot
                .into_iter()
                .map(|(address, hit_count)| DecompileTracePcHit { address, hit_count })
                .collect();
            summary.observed_sources = sources.into_iter().collect();
            function.artifact.trace_summary = Some(summary);
        }
    }
    matched
}

pub fn render_markdown(report: &DecompileReport) -> String {
    let mut out = format!(
        "# KUROSAKI Decompile Report\n\n- Mapper: `{}` ({})\n- PRG: `{}` bytes / `{}` x 8 KiB bank(s)\n- Analysis windows: `{}`\n\n",
        report.mapper,
        report.mapper_name,
        report.prg_rom_size_bytes,
        report.prg_bank_count_8k,
        report
            .analysis_bank_windows
            .iter()
            .map(|bank| bank.map_or_else(|| "?".to_string(), |bank| format!("B{bank:03}")))
            .collect::<Vec<_>>()
            .join(", "),
    );
    for function in &report.functions {
        out.push_str(&format!(
            "## `{}` ({})\n\nSource: `{}`; confidence: `{:.2}`.\n\n```c\n{}\n```\n\n",
            function.name,
            function.id,
            function.source_kind,
            function.confidence_score,
            function.artifact.pseudocode_text.trim_end(),
        ));
        if !function.user_notes.is_empty() {
            out.push_str("User notes:\n\n");
            for note in &function.user_notes {
                out.push_str(&format!("- {}\n", note));
            }
            out.push('\n');
        }
        if !function.warnings.is_empty() {
            out.push_str("Warnings:\n\n");
            for warning in &function.warnings {
                out.push_str(&format!("- {}\n", warning));
            }
            out.push('\n');
        }
    }
    out
}

pub fn render_text(report: &DecompileReport) -> String {
    let mut out = format!(
        "KUROSAKI decompile: mapper {} {} / {} PRG bank(s)\n\n",
        report.mapper, report.mapper_name, report.prg_bank_count_8k
    );
    for function in &report.functions {
        out.push_str(&format!(
            "FUNCTION {} {} ({}) confidence {:.2}\n{}\nDISASSEMBLY\n{}\n",
            function.id,
            function.name,
            function.source_kind,
            function.confidence_score,
            function.artifact.pseudocode_text,
            function.artifact.disassembly_text,
        ));
    }
    out
}

fn insert_root(
    roots: &mut BTreeMap<DecompileAddress, (String, String)>,
    address: Option<DecompileAddress>,
    name: String,
    source_kind: String,
) {
    let Some(address) = address else {
        return;
    };
    match roots.get(&address) {
        Some((_, existing_kind)) if existing_kind == "reset_vector" => {}
        _ => {
            roots.insert(address, (name, source_kind));
        }
    }
}

fn analyze_function(
    cart: &Cartridge,
    mapping: &AddressMapping,
    start: DecompileAddress,
    name: String,
    source_kind: String,
    options: &DecompileOptions,
) -> FunctionAnalysis {
    let mut analysis = FunctionAnalysis {
        start,
        name,
        source_kind,
        instructions: BTreeMap::new(),
        xrefs_out: Vec::new(),
        child_roots: Vec::new(),
        warnings: Vec::new(),
    };
    let mut pending = VecDeque::from([start]);
    let limit = options.max_instructions_per_function.max(1);
    while let Some(address) = pending.pop_front() {
        if analysis.instructions.contains_key(&address) {
            continue;
        }
        if analysis.instructions.len() >= limit {
            push_unique(
                &mut analysis.warnings,
                format!(
                    "Instruction limit ({limit}) reached; function is intentionally truncated."
                ),
            );
            break;
        }
        let Some(instruction) = decode_instruction(cart, mapping, address) else {
            push_unique(
                &mut analysis.warnings,
                format!(
                    "Cannot decode {} from the captured PRG-bank mapping.",
                    address.id()
                ),
            );
            continue;
        };
        let next = next_address(mapping, address, instruction.bytes.len());
        let flow = flow_kind(&instruction);
        if is_mapper_write(&instruction) {
            push_unique(
                &mut analysis.warnings,
                "This function writes a mapper register; targets after that write may require a snapshot or trace from the desired bank state.".to_string(),
            );
        }
        match flow {
            FlowKind::Call => {
                if let Some(target) = instruction.target {
                    analysis.xrefs_out.push(make_xref(
                        &instruction,
                        "call",
                        Some(target),
                        "captured_mapper_window",
                    ));
                    analysis.child_roots.push(target);
                } else if let Some(target_cpu_addr) = instruction.target_cpu_addr {
                    analysis.xrefs_out.push(make_xref(
                        &instruction,
                        "call",
                        None,
                        "mapper_ambiguous",
                    ));
                    push_unique(
                        &mut analysis.warnings,
                        format!(
                            "Call to ${target_cpu_addr:04X} is outside the captured PRG windows."
                        ),
                    );
                }
                if let Some(next) = next {
                    pending.push_back(next);
                }
            }
            FlowKind::Jump => {
                if let Some(target) = instruction.target {
                    analysis.xrefs_out.push(make_xref(
                        &instruction,
                        "jump",
                        Some(target),
                        "captured_mapper_window",
                    ));
                    pending.push_back(target);
                } else if let Some(target_cpu_addr) = instruction.target_cpu_addr {
                    analysis.xrefs_out.push(make_xref(
                        &instruction,
                        "jump",
                        None,
                        "mapper_ambiguous",
                    ));
                    push_unique(
                        &mut analysis.warnings,
                        format!(
                            "Jump to ${target_cpu_addr:04X} is outside the captured PRG windows."
                        ),
                    );
                } else {
                    push_unique(
                        &mut analysis.warnings,
                        "Indirect JMP target is data-dependent and cannot be recovered statically."
                            .to_string(),
                    );
                }
            }
            FlowKind::ConditionalBranch => {
                if let Some(target) = instruction.target {
                    analysis.xrefs_out.push(make_xref(
                        &instruction,
                        "branch",
                        Some(target),
                        "exact_relative",
                    ));
                    pending.push_back(target);
                }
                if let Some(next) = next {
                    pending.push_back(next);
                }
            }
            FlowKind::Fallthrough => {
                if let Some(next) = next {
                    pending.push_back(next);
                }
            }
            FlowKind::Return | FlowKind::Stop => {}
            FlowKind::Unknown => {
                push_unique(
                    &mut analysis.warnings,
                    format!(
                        "Unsupported or unofficial opcode at {} ends this static path.",
                        address.id()
                    ),
                );
            }
        }
        analysis.instructions.insert(address, instruction);
    }
    analysis
        .xrefs_out
        .sort_by_key(|xref| (xref.from, xref.to, xref.kind.clone()));
    analysis.xrefs_out.dedup();
    analysis.child_roots.sort();
    analysis.child_roots.dedup();
    analysis
}

fn build_function(
    analysis: FunctionAnalysis,
    xrefs_in: &BTreeMap<DecompileAddress, Vec<DecompileXref>>,
    labels: &BTreeMap<DecompileAddress, DecompileLabel>,
) -> DecompileFunction {
    let blocks = build_blocks(&analysis.instructions);
    let end = analysis
        .instructions
        .keys()
        .copied()
        .max()
        .unwrap_or(analysis.start);
    let confidence_score = (0.90 - analysis.warnings.len() as f32 * 0.08).max(0.20);
    let artifact = render_artifact(
        &analysis.name,
        &blocks,
        &analysis.warnings,
        confidence_score,
        labels,
    );
    DecompileFunction {
        id: analysis.start.id(),
        start: analysis.start,
        end,
        name: analysis.name.clone(),
        canonical_name: analysis.name,
        source_kind: analysis.source_kind,
        calling_convention_guess: Some("register_a_x_y".to_string()),
        user_notes: Vec::new(),
        blocks,
        xrefs_in: xrefs_in.get(&analysis.start).cloned().unwrap_or_default(),
        xrefs_out: analysis.xrefs_out,
        warnings: analysis.warnings,
        confidence_score,
        artifact,
    }
}

fn build_blocks(
    instructions: &BTreeMap<DecompileAddress, DecompileInstruction>,
) -> Vec<DecompileBasicBlock> {
    if instructions.is_empty() {
        return Vec::new();
    }
    let ordered = instructions.keys().copied().collect::<Vec<_>>();
    let mut leaders = BTreeSet::from([ordered[0]]);
    for instruction in instructions.values() {
        match flow_kind(instruction) {
            FlowKind::ConditionalBranch => {
                if let Some(target) = instruction.target {
                    if instructions.contains_key(&target) {
                        leaders.insert(target);
                    }
                }
                if let Some(next) = next_in_instruction_map(instructions, instruction.address) {
                    leaders.insert(next);
                }
            }
            FlowKind::Jump => {
                if let Some(target) = instruction.target {
                    if instructions.contains_key(&target) {
                        leaders.insert(target);
                    }
                }
            }
            _ => {}
        }
    }
    let mut blocks = Vec::new();
    let mut instruction_to_block = BTreeMap::<DecompileAddress, String>::new();
    let mut index = 0usize;
    while index < ordered.len() {
        let start = ordered[index];
        let mut lines = Vec::new();
        loop {
            if index >= ordered.len() || (!lines.is_empty() && leaders.contains(&ordered[index])) {
                break;
            }
            let instruction = instructions
                .get(&ordered[index])
                .cloned()
                .expect("ordered key exists");
            let flow = flow_kind(&instruction);
            lines.push(instruction);
            index += 1;
            if matches!(
                flow,
                FlowKind::Jump
                    | FlowKind::ConditionalBranch
                    | FlowKind::Return
                    | FlowKind::Stop
                    | FlowKind::Unknown
            ) {
                break;
            }
        }
        let end = lines.last().map(|line| line.address).unwrap_or(start);
        let id = start.id();
        for line in &lines {
            instruction_to_block.insert(line.address, id.clone());
        }
        blocks.push(DecompileBasicBlock {
            id,
            start,
            end,
            instructions: lines,
            predecessors: Vec::new(),
            successors: Vec::new(),
            loop_role: None,
        });
    }
    let mut block_indices = BTreeMap::new();
    for (index, block) in blocks.iter().enumerate() {
        block_indices.insert(block.id.clone(), index);
    }
    let mut predecessor_pairs = Vec::new();
    for block in &mut blocks {
        let Some(last) = block.instructions.last() else {
            continue;
        };
        let mut successor_addresses = Vec::new();
        match flow_kind(last) {
            FlowKind::Jump => last
                .target
                .into_iter()
                .for_each(|target| successor_addresses.push(target)),
            FlowKind::ConditionalBranch => {
                last.target
                    .into_iter()
                    .for_each(|target| successor_addresses.push(target));
                next_in_instruction_map(instructions, last.address)
                    .into_iter()
                    .for_each(|next| successor_addresses.push(next));
            }
            FlowKind::Fallthrough | FlowKind::Call => {
                next_in_instruction_map(instructions, last.address)
                    .into_iter()
                    .for_each(|next| successor_addresses.push(next))
            }
            FlowKind::Return | FlowKind::Stop | FlowKind::Unknown => {}
        }
        for address in successor_addresses {
            let Some(id) = instruction_to_block.get(&address).cloned() else {
                continue;
            };
            if !block.successors.contains(&id) {
                if address <= block.start {
                    block.loop_role = Some("loop_latch".to_string());
                }
                block.successors.push(id.clone());
                predecessor_pairs.push((id, block.id.clone()));
            }
        }
    }
    for (successor, predecessor) in predecessor_pairs {
        if let Some(index) = block_indices.get(&successor).copied() {
            if !blocks[index].predecessors.contains(&predecessor) {
                blocks[index].predecessors.push(predecessor);
            }
            if blocks[index].loop_role.is_none() && blocks[index].start <= blocks[index].end {
                let is_loop_header = blocks[index]
                    .predecessors
                    .iter()
                    .filter_map(|id| block_indices.get(id).copied())
                    .any(|predecessor_index| {
                        blocks[predecessor_index].start >= blocks[index].start
                    });
                if is_loop_header {
                    blocks[index].loop_role = Some("loop_header".to_string());
                }
            }
        }
    }
    blocks
}

fn render_artifact(
    name: &str,
    blocks: &[DecompileBasicBlock],
    warnings: &[String],
    confidence_score: f32,
    labels: &BTreeMap<DecompileAddress, DecompileLabel>,
) -> DecompileArtifact {
    let mut disassembly = String::new();
    let mut pseudocode = format!("void {}(void) {{\n", sanitize_identifier(name));
    for block in blocks {
        let label = label_name(labels, block.start);
        disassembly.push_str(&format!("{}:\n", block.id));
        pseudocode.push_str(&format!("{}:\n", label));
        for instruction in &block.instructions {
            disassembly.push_str(&format!(
                "  {}: {:<10} {}\n",
                instruction.address.id(),
                instruction
                    .bytes
                    .iter()
                    .map(|byte| format!("{byte:02X}"))
                    .collect::<Vec<_>>()
                    .join(" "),
                instruction.text
            ));
            pseudocode.push_str("  ");
            pseudocode.push_str(&pseudocode_line(instruction, labels));
            pseudocode.push('\n');
        }
    }
    pseudocode.push_str("}\n");
    DecompileArtifact {
        disassembly_text: disassembly,
        pseudocode_text: pseudocode,
        warnings: warnings.to_vec(),
        confidence_score,
        trace_summary: None,
    }
}

fn pseudocode_line(
    instruction: &DecompileInstruction,
    labels: &BTreeMap<DecompileAddress, DecompileLabel>,
) -> String {
    let operand = instruction.operand_text.as_str();
    let target = instruction
        .target
        .map(|address| label_name(labels, address))
        .or_else(|| {
            instruction
                .target_cpu_addr
                .map(|address| format!("unresolved_{address:04X}"))
        });
    match instruction.mnemonic.as_str() {
        "LDA" => format!("a = {operand};"),
        "LDX" => format!("x = {operand};"),
        "LDY" => format!("y = {operand};"),
        "STA" => format!("mem[{operand}] = a;"),
        "STX" => format!("mem[{operand}] = x;"),
        "STY" => format!("mem[{operand}] = y;"),
        "TAX" => "x = a;".to_string(),
        "TAY" => "y = a;".to_string(),
        "TXA" => "a = x;".to_string(),
        "TYA" => "a = y;".to_string(),
        "TSX" => "x = sp;".to_string(),
        "TXS" => "sp = x;".to_string(),
        "INX" => "x++;".to_string(),
        "INY" => "y++;".to_string(),
        "DEX" => "x--;".to_string(),
        "DEY" => "y--;".to_string(),
        "JSR" => format!(
            "{}();",
            target.unwrap_or_else(|| "indirect_call".to_string())
        ),
        "JMP" => target.map_or_else(
            || format!("/* {} */", instruction.text),
            |target| format!("goto {target};"),
        ),
        "RTS" | "RTI" => "return;".to_string(),
        "BRK" => "interrupt(); return;".to_string(),
        branch if branch_condition(branch).is_some() => target.map_or_else(
            || format!("/* {} */", instruction.text),
            |target| format!("if ({}) goto {target};", branch_condition(branch).unwrap()),
        ),
        _ => {
            if operand.is_empty() {
                format!("/* {} */", instruction.mnemonic)
            } else {
                format!("/* {} {} */", instruction.mnemonic, operand)
            }
        }
    }
}

fn branch_condition(mnemonic: &str) -> Option<&'static str> {
    match mnemonic {
        "BPL" => Some("!negative"),
        "BMI" => Some("negative"),
        "BVC" => Some("!overflow"),
        "BVS" => Some("overflow"),
        "BCC" => Some("!carry"),
        "BCS" => Some("carry"),
        "BNE" => Some("!zero"),
        "BEQ" => Some("zero"),
        _ => None,
    }
}

fn decode_instruction(
    cart: &Cartridge,
    mapping: &AddressMapping,
    address: DecompileAddress,
) -> Option<DecompileInstruction> {
    let opcode = read_physical_prg_byte(cart, address)?;
    let mnemonic_template = opcode_mnemonic(opcode);
    let len = instruction_len(opcode, mnemonic_template);
    let mut bytes = Vec::with_capacity(len);
    for offset in 0..len {
        let cpu_addr = address.cpu_addr.checked_add(offset as u16)?;
        bytes.push(mapping.read_byte(cart, address, cpu_addr)?);
    }
    let mnemonic = mnemonic_template
        .split_whitespace()
        .next()
        .unwrap_or(mnemonic_template)
        .trim_start_matches('*')
        .split('/')
        .next()
        .unwrap_or(mnemonic_template)
        .to_string();
    let flow = flow_from_opcode(opcode, mnemonic_template);
    let target_cpu_addr = match flow {
        FlowKind::Call | FlowKind::Jump if opcode != 0x6C && bytes.len() >= 3 => {
            Some(u16::from_le_bytes([bytes[1], bytes[2]]))
        }
        FlowKind::ConditionalBranch if bytes.len() >= 2 => {
            Some(((address.cpu_addr.wrapping_add(2) as i32) + i32::from(bytes[1] as i8)) as u16)
        }
        _ => None,
    };
    let target = target_cpu_addr.and_then(|target| mapping.resolve_target(address, target));
    let text = render_instruction(mnemonic_template, address.cpu_addr, &bytes);
    let operand_text = text
        .split_once(char::is_whitespace)
        .map(|(_, operand)| operand.to_string())
        .unwrap_or_default();
    Some(DecompileInstruction {
        address,
        file_offset: physical_file_offset(cart, address),
        bytes,
        mnemonic,
        operand_text,
        text,
        flow_kind: flow.as_str().to_string(),
        target,
        target_cpu_addr,
        fallthrough: matches!(
            flow,
            FlowKind::Fallthrough | FlowKind::Call | FlowKind::ConditionalBranch
        ),
    })
}

fn instruction_len(opcode: u8, mnemonic: &str) -> usize {
    if matches!(
        opcode,
        0x10 | 0x30 | 0x50 | 0x70 | 0x90 | 0xB0 | 0xD0 | 0xF0
    ) {
        return 2;
    }
    if mnemonic.starts_with('*') {
        return 1;
    }
    if mnemonic.contains(" a") || mnemonic.contains("(a)") {
        3
    } else if mnemonic.contains(" d") || mnemonic.contains("(d") || mnemonic.contains('#') {
        2
    } else {
        1
    }
}

fn flow_from_opcode(opcode: u8, mnemonic: &str) -> FlowKind {
    if mnemonic.starts_with('*') {
        return FlowKind::Unknown;
    }
    match opcode {
        0x20 => FlowKind::Call,
        0x4C | 0x6C => FlowKind::Jump,
        0x10 | 0x30 | 0x50 | 0x70 | 0x90 | 0xB0 | 0xD0 | 0xF0 => FlowKind::ConditionalBranch,
        0x40 | 0x60 => FlowKind::Return,
        0x00 => FlowKind::Stop,
        _ => FlowKind::Fallthrough,
    }
}

fn flow_kind(instruction: &DecompileInstruction) -> FlowKind {
    match instruction.flow_kind.as_str() {
        "call" => FlowKind::Call,
        "jump" => FlowKind::Jump,
        "conditional_branch" => FlowKind::ConditionalBranch,
        "return" => FlowKind::Return,
        "stop" => FlowKind::Stop,
        "unknown" => FlowKind::Unknown,
        _ => FlowKind::Fallthrough,
    }
}

fn render_instruction(mnemonic: &str, cpu_addr: u16, bytes: &[u8]) -> String {
    if bytes.len() == 2
        && matches!(
            bytes[0],
            0x10 | 0x30 | 0x50 | 0x70 | 0x90 | 0xB0 | 0xD0 | 0xF0
        )
    {
        let target = ((cpu_addr.wrapping_add(2) as i32) + i32::from(bytes[1] as i8)) as u16;
        return format!("{mnemonic} ${target:04X}");
    }
    match bytes {
        [_, operand] if mnemonic.contains('#') => {
            mnemonic.replacen('#', &format!("#${operand:02X}"), 1)
        }
        [_, operand] if mnemonic.contains('d') => {
            mnemonic.replacen('d', &format!("${operand:02X}"), 1)
        }
        [_, low, high] => {
            let operand = u16::from_le_bytes([*low, *high]);
            if mnemonic.contains("(a)") {
                mnemonic.replacen("(a)", &format!("(${operand:04X})"), 1)
            } else if mnemonic.contains(" a") {
                mnemonic.replacen(" a", &format!(" ${operand:04X}"), 1)
            } else {
                mnemonic.to_string()
            }
        }
        _ => mnemonic.to_string(),
    }
}

fn mapped_u16(cart: &Cartridge, mapping: &AddressMapping, cpu_addr: u16) -> Option<u16> {
    let low = mapping.map_cpu_addr(cpu_addr)?;
    let high_cpu = cpu_addr.checked_add(1)?;
    let low = mapping.read_byte(cart, low, cpu_addr)?;
    let high_source = mapping.map_cpu_addr(high_cpu)?;
    let high = mapping.read_byte(cart, high_source, high_cpu)?;
    Some(u16::from_le_bytes([low, high]))
}

fn next_address(
    mapping: &AddressMapping,
    address: DecompileAddress,
    len: usize,
) -> Option<DecompileAddress> {
    let next_cpu_addr = address.cpu_addr.checked_add(len as u16)?;
    mapping.resolve_target(address, next_cpu_addr)
}

fn next_in_instruction_map(
    instructions: &BTreeMap<DecompileAddress, DecompileInstruction>,
    address: DecompileAddress,
) -> Option<DecompileAddress> {
    instructions
        .range((
            std::ops::Bound::Excluded(address),
            std::ops::Bound::Unbounded,
        ))
        .next()
        .map(|(address, _)| *address)
}

fn make_xref(
    instruction: &DecompileInstruction,
    kind: &str,
    target: Option<DecompileAddress>,
    confidence: &str,
) -> DecompileXref {
    DecompileXref {
        from: instruction.address,
        to: target,
        target_cpu_addr: instruction
            .target_cpu_addr
            .unwrap_or(instruction.address.cpu_addr),
        kind: kind.to_string(),
        confidence: confidence.to_string(),
    }
}

fn build_xrefs_in(xrefs: &[DecompileXref]) -> BTreeMap<DecompileAddress, Vec<DecompileXref>> {
    let mut incoming = BTreeMap::new();
    for xref in xrefs {
        if let Some(target) = xref.to {
            incoming
                .entry(target)
                .or_insert_with(Vec::new)
                .push(xref.clone());
        }
    }
    incoming
}

fn refresh_artifacts(report: &mut DecompileReport) {
    let labels = report
        .labels
        .iter()
        .cloned()
        .map(|label| {
            (
                DecompileAddress {
                    prg_bank_8k: label.prg_bank_8k,
                    cpu_addr: label.cpu_addr,
                },
                label,
            )
        })
        .collect::<BTreeMap<_, _>>();
    for function in &mut report.functions {
        let trace_summary = function.artifact.trace_summary.clone();
        function.artifact = render_artifact(
            &function.name,
            &function.blocks,
            &function.warnings,
            function.confidence_score,
            &labels,
        );
        function.artifact.trace_summary = trace_summary;
    }
}

fn function_selector_matches(function: &DecompileFunction, selector: &str) -> bool {
    let selector = selector.trim();
    parse_address_selector(selector).is_some_and(|address| address == function.start)
        || selector.eq_ignore_ascii_case(&function.id)
        || selector.eq_ignore_ascii_case(&function.name)
        || selector.eq_ignore_ascii_case(&function.canonical_name)
}

fn parse_address_selector(selector: &str) -> Option<DecompileAddress> {
    let (bank, address) = selector
        .trim()
        .strip_prefix('B')
        .unwrap_or(selector.trim())
        .split_once(':')?;
    let prg_bank_8k = bank.parse::<u16>().ok()?;
    let cpu_addr = parse_u16(address)?;
    Some(DecompileAddress {
        prg_bank_8k,
        cpu_addr,
    })
}

fn parse_u16(value: &str) -> Option<u16> {
    let value = value.trim();
    if let Some(hex) = value
        .strip_prefix('$')
        .or_else(|| value.strip_prefix("0x"))
        .or_else(|| value.strip_prefix("0X"))
    {
        u16::from_str_radix(hex, 16).ok()
    } else {
        value.parse::<u16>().ok()
    }
}

fn label_name(
    labels: &BTreeMap<DecompileAddress, DecompileLabel>,
    address: DecompileAddress,
) -> String {
    labels
        .get(&address)
        .map(|label| sanitize_identifier(&label.name))
        .unwrap_or_else(|| sanitize_identifier(&auto_label_name(address)))
}

fn auto_function_name(address: DecompileAddress) -> String {
    format!("sub_B{:03}_{:04X}", address.prg_bank_8k, address.cpu_addr)
}

fn auto_label_name(address: DecompileAddress) -> String {
    format!("L_B{:03}_{:04X}", address.prg_bank_8k, address.cpu_addr)
}

fn sanitize_identifier(value: &str) -> String {
    let mut out = String::new();
    for (index, character) in value.chars().enumerate() {
        if character.is_ascii_alphanumeric() || character == '_' {
            if index == 0 && character.is_ascii_digit() {
                out.push('_');
            }
            out.push(character);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "unnamed".to_string()
    } else {
        out
    }
}

fn physical_address_is_valid(cart: &Cartridge, address: DecompileAddress) -> bool {
    address.cpu_addr >= 0x8000
        && usize::from(address.prg_bank_8k) * PRG_BANK_SIZE + usize::from(address.cpu_addr & 0x1FFF)
            < cart.prg_rom.len()
}

fn read_physical_prg_byte(cart: &Cartridge, address: DecompileAddress) -> Option<u8> {
    let index = usize::from(address.prg_bank_8k)
        .checked_mul(PRG_BANK_SIZE)?
        .checked_add(usize::from(address.cpu_addr & 0x1FFF))?;
    cart.prg_rom.get(index).copied()
}

fn physical_file_offset(cart: &Cartridge, address: DecompileAddress) -> usize {
    16 + cart.trainer.as_ref().map_or(0, |_| 512)
        + usize::from(address.prg_bank_8k) * PRG_BANK_SIZE
        + usize::from(address.cpu_addr & 0x1FFF)
}

fn prg_bank_count(cart: &Cartridge) -> u16 {
    cart.prg_rom.len().div_ceil(PRG_BANK_SIZE) as u16
}

fn is_mapper_write(instruction: &DecompileInstruction) -> bool {
    matches!(instruction.mnemonic.as_str(), "STA" | "STX" | "STY")
        && parse_absolute_operand(&instruction.operand_text)
            .is_some_and(|address| address >= 0x8000)
}

fn parse_absolute_operand(operand: &str) -> Option<u16> {
    let value = operand.trim_start_matches('(').trim_start_matches('$');
    let digits = value
        .chars()
        .take_while(|character| character.is_ascii_hexdigit())
        .collect::<String>();
    (digits.len() == 4)
        .then(|| u16::from_str_radix(&digits, 16).ok())
        .flatten()
}

fn push_unique(items: &mut Vec<String>, item: String) {
    if !items.contains(&item) {
        items.push(item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_nrom(prg: &[u8], reset: u16) -> Cartridge {
        let mut bytes = vec![0_u8; 16 + 16 * 1024];
        bytes[0..4].copy_from_slice(b"NES\x1A");
        bytes[4] = 1;
        bytes[16..16 + prg.len()].copy_from_slice(prg);
        let vector = 16 + 0x3FFC;
        bytes[vector..vector + 2].copy_from_slice(&reset.to_le_bytes());
        bytes[vector - 2..vector].copy_from_slice(&reset.to_le_bytes());
        bytes[vector + 2..vector + 4].copy_from_slice(&reset.to_le_bytes());
        Cartridge::from_bytes(&bytes).unwrap()
    }

    #[test]
    fn analyzer_recovers_call_cfg_and_readable_pseudocode() {
        let mut prg = vec![0xEA; 16 * 1024];
        // $C000: LDA #$12; JSR $C010; BNE $C000; RTS
        prg[0] = 0xA9;
        prg[1] = 0x12;
        prg[2] = 0x20;
        prg[3] = 0x10;
        prg[4] = 0xC0;
        prg[5] = 0xD0;
        prg[6] = 0xF9;
        prg[7] = 0x60;
        prg[0x10] = 0x60;
        let report =
            analyze_cartridge(&synthetic_nrom(&prg, 0xC000), &DecompileOptions::default()).unwrap();
        let reset = report
            .functions
            .iter()
            .find(|function| function.name == "Reset")
            .unwrap();
        assert!(reset.xrefs_out.iter().any(|xref| xref.kind == "call"));
        assert!(reset.artifact.pseudocode_text.contains("a = #$12;"));
        assert!(reset.artifact.pseudocode_text.contains("if (!zero) goto"));
        assert!(reset.blocks.iter().any(|block| block.loop_role.is_some()));
    }

    #[test]
    fn analyzer_keeps_indexed_indirect_instruction_boundaries() {
        let mut prg = vec![0xEA; 16 * 1024];
        // $C000: LDA ($10,X); LDA ($20),Y; INC $30,X; RTS
        prg[0..7].copy_from_slice(&[0xA1, 0x10, 0xB1, 0x20, 0xF6, 0x30, 0x60]);
        let report =
            analyze_cartridge(&synthetic_nrom(&prg, 0xC000), &DecompileOptions::default()).unwrap();
        let reset = report
            .functions
            .iter()
            .find(|function| function.name == "Reset")
            .unwrap();
        let instructions = reset
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .collect::<Vec<_>>();
        assert_eq!(
            instructions
                .iter()
                .map(|instruction| instruction.address.cpu_addr)
                .collect::<Vec<_>>(),
            vec![0xC000, 0xC002, 0xC004, 0xC006]
        );
        assert_eq!(instructions[0].bytes, vec![0xA1, 0x10]);
        assert_eq!(instructions[1].bytes, vec![0xB1, 0x20]);
        assert_eq!(instructions[2].bytes, vec![0xF6, 0x30]);
        assert_eq!(instructions[3].bytes, vec![0x60]);
    }

    #[test]
    fn annotations_rename_functions_and_rebuild_pseudocode() {
        let prg = vec![0x60; 16 * 1024];
        let mut report =
            analyze_cartridge(&synthetic_nrom(&prg, 0xC000), &DecompileOptions::default()).unwrap();
        let reset = report
            .functions
            .iter()
            .find(|function| function.name == "Reset")
            .unwrap()
            .id
            .clone();
        let annotations = DecompileAnnotationFile {
            function_overrides: vec![DecompileFunctionOverride {
                selector: reset,
                rename: Some("BootMain".to_string()),
                calling_convention: Some("custom".to_string()),
                notes: vec!["verified root".to_string()],
            }],
            ..Default::default()
        };
        assert_eq!(apply_annotations(&mut report, &annotations), 3);
        let reset = report
            .functions
            .iter()
            .find(|function| function.name == "BootMain")
            .unwrap();
        assert!(reset.artifact.pseudocode_text.starts_with("void BootMain"));
        assert_eq!(reset.calling_convention_guess.as_deref(), Some("custom"));
    }

    #[test]
    fn trace_overlay_counts_exact_physical_bank_and_pc() {
        let prg = vec![0x60; 16 * 1024];
        let mut report =
            analyze_cartridge(&synthetic_nrom(&prg, 0xC000), &DecompileOptions::default()).unwrap();
        let reset = report
            .functions
            .iter()
            .find(|function| function.name == "Reset")
            .unwrap();
        let event = TraceEvent {
            kind: "cpu.instruction".to_string(),
            frame: 4,
            cpu_cycle: 12,
            scanline: None,
            dot: None,
            pc: Some(reset.start.cpu_addr),
            prg_bank: Some(reset.start.prg_bank_8k),
            addr: None,
            value: None,
            opcode: Some(0x60),
            mnemonic: Some("RTS".to_string()),
            message: None,
            severity: None,
            source_file: None,
            source_line: None,
            source_function: None,
            details: None,
        };
        assert_eq!(apply_trace_events(&mut report, &[event]), 1);
        assert_eq!(
            report.functions[0]
                .artifact
                .trace_summary
                .as_ref()
                .unwrap()
                .hit_count,
            1
        );
    }
}
