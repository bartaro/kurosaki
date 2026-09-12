//! KUROSAKI core library.
//!
//! Clean-room Rust implementation scaffold for a KITAQFC-aware NES/Famicom
//! observation emulator. Phase 0-3 focuses on ROM inspection, official 2A03
//! opcode coverage, NROM/MMC1/MMC3/FDS mapper scaffolds, PPU A12-driven MMC3
//! IRQ observation, scanline/pixel-oriented PPU diagnostics, CPU-cycle-clocked
//! 2A03 audio with DMC playback, FDS/VRC6/VRC7 expansion-audio scaffolds,
//! trace/report data models, and Python/CLI-friendly APIs. Phase 0-4 adds a full
//! NES 2.0 mapper registry, probe-only mapper fallback, mapper support reports,
//! and CLI mapper-list output.

pub mod apu;
pub mod battery;
pub mod board_audit;
pub mod bus;
pub mod cart;
pub mod cpu;
pub mod decompile;
pub mod diagnostic_events;
pub mod diagnostics;
pub mod disasm;
pub mod emulator;
pub mod error;
pub mod fds;
pub mod hash;
pub mod kitaqfc;
pub mod mapper;
pub mod mapper_db;
pub mod mapper_scaffolds;
pub mod ppu;
pub mod replay;
pub mod report;
pub mod screen;
pub mod snapshot;
pub mod snapshot_rebase;
pub mod trace;

pub use board_audit::{audit_board, BoardAuditDiagnostic, BoardAuditReport};
pub use cart::{Cartridge, HeaderKind, Mirroring, RomInfo};
pub use decompile::{
    analyze_cartridge, analyze_emulator, apply_annotations, apply_trace_events, render_markdown,
    render_text, DecompileAddress, DecompileAnnotationFile, DecompileArtifact, DecompileBasicBlock,
    DecompileFunction, DecompileFunctionOverride, DecompileInstruction, DecompileLabel,
    DecompileLabelOverride, DecompileOptions, DecompileReport, DecompileTracePcHit,
    DecompileTraceSummary, DecompileXref,
};
pub use diagnostic_events::{diagnostic_events_from_trace_and_report, DiagnosticEvent};
pub use diagnostics::{Diagnostic, DiagnosticReport, Severity};
pub use emulator::{Emulator, RunOptions, RunSummary};
pub use error::{KurosakiError, Result};
pub use kitaqfc::KitaqfcDebugBundle;
pub use mapper_db::{
    all_mapper_specs, implemented_mapper_specs, mapper_spec, MapperFamily, MapperSpec,
    MapperSupportLevel,
};
pub use replay::{Replay, ReplayFrame};
pub use snapshot::Snapshot;
pub use snapshot_rebase::{
    rebase_snapshot, CpuRamPatch, CpuRamPatchDigest, MapperStateTransfer, PrgChangeDigest,
    PrgRamPatch, PrgRamPatchDigest, SnapshotRebaseContract, SnapshotRebaseReport,
    TargetPrgAppendDigest, SNAPSHOT_REBASE_CONTRACT_FORMAT, SNAPSHOT_REBASE_REPORT_FORMAT,
};
pub use trace::{TraceConfig, TraceEvent, TraceSink};
