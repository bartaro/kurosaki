use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use kurosaki_core::cart::Cartridge;
use kurosaki_core::decompile::{
    analyze_emulator, apply_annotations, apply_trace_events,
    render_markdown as render_decompile_markdown, render_text as render_decompile_text,
    DecompileAnnotationFile, DecompileOptions,
};
use kurosaki_core::diagnostic_events::{diagnostic_events_from_trace_and_report, DiagnosticEvent};
use kurosaki_core::diagnostics::DiagnosticReport;
use kurosaki_core::disasm;
use kurosaki_core::emulator::{Emulator, RunOptions};
use kurosaki_core::fds::{FdsDiskFile, FdsDiskImage};
use kurosaki_core::hash::sha256_hex;
use kurosaki_core::kitaqfc::KitaqfcDebugBundle;
use kurosaki_core::replay::{Replay, ReplayFrame};
use kurosaki_core::report;
use kurosaki_core::screen;
use kurosaki_core::snapshot::Snapshot;
use kurosaki_core::snapshot_rebase::{
    rebase_snapshot as rebase_snapshot_state, CpuRamPatch, PrgRamPatch, SnapshotRebaseContract,
    SnapshotRebaseReport,
};
use kurosaki_core::trace::{TraceConfig, TraceEvent};
use kurosaki_core::{all_mapper_specs, audit_board, implemented_mapper_specs, mapper_spec};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::{write::FileOptions, ZipWriter};

#[derive(Debug, Parser)]
#[command(name = "kurosaki")]
#[command(about = "KITAQFC-aware clean-room NES observation emulator CLI")]
#[command(version)]
// Top-level Clap command container. Parsing validates argument shape;
// file contents and execution conditions are checked in handlers.
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Clone)]
// Parsed starting address and file path; payload loading is deferred until rebase.
struct PrgRamPatchSpec {
    cpu_address: u16,
    path: PathBuf,
}

#[derive(Debug, Clone)]
// Parsed physical CPU address and owned bytes for a contract-bound RAM patch.
struct CpuRamPatchSpec {
    cpu_address: u16,
    bytes: Vec<u8>,
}

// Split ADDRESS=FILE once, accepting decimal or 0x/0X/$ addresses in
// $6000-$7FFF and a nonempty path. File bytes and the complete range are
// validated later against the rebase contract; the path is not opened here.
fn parse_prg_ram_patch_spec(value: &str) -> std::result::Result<PrgRamPatchSpec, String> {
    let (address, path) = value
        .split_once('=')
        .ok_or_else(|| "expected ADDRESS=FILE".to_string())?;
    if path.is_empty() {
        return Err("PRG-RAM patch file path cannot be empty".to_string());
    }
    let address = if let Some(hex) = address
        .strip_prefix("0x")
        .or_else(|| address.strip_prefix("0X"))
        .or_else(|| address.strip_prefix('$'))
    {
        u16::from_str_radix(hex, 16)
            .map_err(|_| format!("invalid hexadecimal PRG-RAM address {address}"))?
    } else {
        address
            .parse::<u16>()
            .map_err(|_| format!("invalid decimal PRG-RAM address {address}"))?
    };
    if !(0x6000..=0x7FFF).contains(&address) {
        return Err(format!(
            "PRG-RAM patch address must be inside $6000-$7FFF, got ${address:04X}"
        ));
    }
    Ok(PrgRamPatchSpec {
        cpu_address: address,
        path: PathBuf::from(path),
    })
}

// Parse ADDRESS=HEXBYTES for physical CPU RAM and reject empty, odd-length
// or out-of-range patches. Hex text is sliced by byte pairs and assumes
// ASCII input; non-ASCII text can violate UTF-8 slice boundaries.
fn parse_cpu_ram_patch_spec(value: &str) -> std::result::Result<CpuRamPatchSpec, String> {
    let (address, hex_bytes) = value
        .split_once('=')
        .ok_or_else(|| "expected ADDRESS=HEXBYTES".to_string())?;
    let address = if let Some(hex) = address
        .strip_prefix("0x")
        .or_else(|| address.strip_prefix("0X"))
        .or_else(|| address.strip_prefix('$'))
    {
        u16::from_str_radix(hex, 16)
            .map_err(|_| format!("invalid hexadecimal CPU-RAM address {address}"))?
    } else {
        address
            .parse::<u16>()
            .map_err(|_| format!("invalid decimal CPU-RAM address {address}"))?
    };
    if hex_bytes.is_empty() || !hex_bytes.len().is_multiple_of(2) {
        return Err("CPU-RAM patch HEXBYTES must contain complete bytes".to_string());
    }
    let bytes = (0..hex_bytes.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&hex_bytes[index..index + 2], 16)
                .map_err(|_| "CPU-RAM patch contains non-hexadecimal bytes".to_string())
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let end = usize::from(address)
        .checked_add(bytes.len())
        .ok_or_else(|| "CPU-RAM patch range overflow".to_string())?;
    if end > 0x0800 {
        return Err(format!(
            "CPU-RAM patch must remain inside physical $0000-$07FF, got ${address:04X}"
        ));
    }
    Ok(CpuRamPatchSpec {
        cpu_address: address,
        bytes,
    })
}

#[derive(Debug, Subcommand)]
// Public command argument shapes and defaults. Plain comments here do
// not alter Clap help output or option names.
enum Commands {
    /// Export raw battery RAM from a matching snapshot (not a snapshot file).
    BatteryExport {
        rom: PathBuf,
        snapshot: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Cold boot with an explicit battery file; no implicit save-file writes.
    BatteryRun {
        rom: PathBuf,
        #[arg(long)]
        sav: PathBuf,
        #[arg(long, default_value_t = 600)]
        frames: u64,
        #[arg(long)]
        replay: Option<PathBuf>,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long)]
        snapshot: Option<PathBuf>,
        #[arg(long)]
        png: Option<PathBuf>,
        #[arg(long)]
        save_out: Option<PathBuf>,
    },
    InspectRom {
        rom: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long)]
        md: Option<PathBuf>,
    },
    AuditBoard {
        rom: PathBuf,
        #[arg(long, default_value = "surom512")]
        expected_board: String,
        #[arg(long)]
        metadata: Option<PathBuf>,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    Run {
        rom: PathBuf,
        #[arg(long, default_value_t = 1)]
        frames: u64,
        #[arg(long)]
        max_instructions: Option<u64>,
        #[arg(long)]
        headless: bool,
        #[arg(long)]
        state_hash: Option<PathBuf>,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long)]
        md: Option<PathBuf>,
        #[arg(long)]
        png: Option<PathBuf>,
        #[arg(long)]
        snapshot: Option<PathBuf>,
        #[arg(long)]
        observe_json: Option<PathBuf>,
        #[arg(long = "trace-jsonl")]
        trace_jsonl: Option<PathBuf>,
        #[arg(long = "emit-diagnostics", alias = "diagnostics-jsonl")]
        emit_diagnostics: Option<PathBuf>,
        #[arg(long = "repro-bundle")]
        repro_bundle: Option<PathBuf>,
        #[arg(long = "break-on-diagnostic")]
        break_on_diagnostic: Option<String>,
        #[arg(long = "png-on-diagnostic")]
        png_on_diagnostic: Option<PathBuf>,
        #[arg(long = "snapshot-on-diagnostic")]
        snapshot_on_diagnostic: Option<PathBuf>,
        #[arg(long, alias = "metadata")]
        kitaqfc_debug: Option<PathBuf>,
        #[arg(long)]
        allow_unimplemented: bool,
        #[arg(long, default_value_t = 0)]
        pad1: u8,
        #[arg(long, default_value_t = 0)]
        pad2: u8,
    },
    Trace {
        rom: PathBuf,
        #[arg(long, default_value_t = 1)]
        frames: u64,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        cpu: bool,
        #[arg(long)]
        mem_read: bool,
        #[arg(long)]
        mem_write: bool,
        #[arg(long)]
        ppu: bool,
        #[arg(long)]
        apu: bool,
        #[arg(long)]
        mapper: bool,
        #[arg(long)]
        nmi: bool,
        #[arg(long)]
        dma: bool,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        kitaqfc_debug: Option<PathBuf>,
        #[arg(long)]
        allow_unimplemented: bool,
        #[arg(long, default_value_t = 0)]
        pad1: u8,
        #[arg(long, default_value_t = 0)]
        pad2: u8,
    },
    AudioExport {
        rom: PathBuf,
        #[arg(long, default_value_t = 60)]
        frames: u64,
        #[arg(long)]
        wav: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long)]
        allow_unimplemented: bool,
        #[arg(long)]
        trace_audio: bool,
        #[arg(long, default_value_t = 0)]
        pad1: u8,
        #[arg(long, default_value_t = 0)]
        pad2: u8,
    },
    Profile {
        rom: PathBuf,
        #[arg(long, default_value_t = 60)]
        frames: u64,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        md: Option<PathBuf>,
        #[arg(long)]
        kitaqfc_debug: Option<PathBuf>,
        #[arg(long)]
        allow_unimplemented: bool,
    },
    Diagnose {
        rom: PathBuf,
        #[arg(long, default_value_t = 0)]
        frames: u64,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        md: Option<PathBuf>,
        #[arg(long = "emit-diagnostics", alias = "diagnostics-jsonl")]
        emit_diagnostics: Option<PathBuf>,
        #[arg(long)]
        check: Vec<CheckKind>,
        #[arg(long, alias = "metadata")]
        kitaqfc_debug: Option<PathBuf>,
        #[arg(long)]
        allow_unimplemented: bool,
        #[arg(long, default_value_t = 0)]
        pad1: u8,
        #[arg(long, default_value_t = 0)]
        pad2: u8,
    },
    ReplayRecord {
        rom: PathBuf,
        #[arg(long, default_value_t = 60)]
        frames: u64,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        allow_unimplemented: bool,
    },
    ReplayRun {
        rom: PathBuf,
        replay: PathBuf,
        /// Restore this version-2 snapshot before applying the replay timeline.
        /// With this option, --frames and trace bounds are absolute emulator frames.
        #[arg(long = "load-snapshot")]
        load_snapshot: Option<PathBuf>,
        #[arg(long)]
        frames: Option<u64>,
        #[arg(long)]
        verify: bool,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long)]
        png: Option<PathBuf>,
        #[arg(long)]
        snapshot: Option<PathBuf>,
        #[arg(long)]
        observe_json: Option<PathBuf>,
        #[arg(long)]
        wav: Option<PathBuf>,
        #[arg(long = "trace-jsonl")]
        trace_jsonl: Option<PathBuf>,
        #[arg(long, requires = "trace_jsonl")]
        trace_start_frame: Option<u64>,
        #[arg(long, requires = "trace_jsonl")]
        trace_end_frame_exclusive: Option<u64>,
        #[arg(long, requires = "trace_jsonl")]
        cpu: bool,
        #[arg(long, requires = "trace_jsonl")]
        mem_read: bool,
        #[arg(long, requires = "trace_jsonl")]
        mem_write: bool,
        #[arg(long, requires = "trace_jsonl")]
        ppu: bool,
        #[arg(long, requires = "trace_jsonl")]
        apu: bool,
        #[arg(long, requires = "trace_jsonl")]
        mapper: bool,
        #[arg(long, requires = "trace_jsonl")]
        nmi: bool,
        #[arg(long, requires = "trace_jsonl")]
        dma: bool,
        #[arg(long, requires = "trace_jsonl")]
        full: bool,
        #[arg(long)]
        allow_unimplemented: bool,
    },
    SnapshotSave {
        rom: PathBuf,
        #[arg(long, default_value_t = 1)]
        frames: u64,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        allow_unimplemented: bool,
        #[arg(long, default_value_t = 0)]
        pad1: u8,
        #[arg(long, default_value_t = 0)]
        pad2: u8,
    },
    SnapshotLoad {
        snapshot: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Rebind a reviewed v2 snapshot to an explicitly allowlisted compatible ROM.
    SnapshotRebase {
        source_rom: PathBuf,
        target_rom: PathBuf,
        snapshot: PathBuf,
        #[arg(long)]
        contract: PathBuf,
        #[arg(
            long = "prg-ram-patch",
            value_name = "ADDRESS=FILE",
            value_parser = parse_prg_ram_patch_spec
        )]
        prg_ram_patches: Vec<PrgRamPatchSpec>,
        #[arg(
            long = "cpu-ram-patch",
            value_name = "ADDRESS=HEXBYTES",
            value_parser = parse_cpu_ram_patch_spec
        )]
        cpu_ram_patches: Vec<CpuRamPatchSpec>,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    SnapshotResume {
        rom: PathBuf,
        snapshot: PathBuf,
        /// Reset the CPU after restoring the snapshot while retaining mapper RAM.
        #[arg(long)]
        reset_at_start: bool,
        #[arg(long, default_value_t = 1)]
        frames: u64,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long)]
        png: Option<PathBuf>,
        #[arg(long)]
        observe_json: Option<PathBuf>,
        #[arg(long = "trace-jsonl")]
        trace_jsonl: Option<PathBuf>,
        #[arg(long)]
        cpu: bool,
        #[arg(long)]
        mem_read: bool,
        #[arg(long)]
        mem_write: bool,
        #[arg(long)]
        ppu: bool,
        #[arg(long)]
        apu: bool,
        #[arg(long)]
        mapper: bool,
        #[arg(long)]
        nmi: bool,
        #[arg(long)]
        dma: bool,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        allow_unimplemented: bool,
        #[arg(long, default_value_t = 0)]
        pad1: u8,
        #[arg(long, default_value_t = 0)]
        pad2: u8,
    },
    Diff {
        #[arg(long)]
        before: PathBuf,
        #[arg(long)]
        after: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long)]
        md: Option<PathBuf>,
    },
    /// Recover bounded function CFGs and readable pseudocode from a mapper context.
    Decompile {
        rom: PathBuf,
        /// Write the rendered report here; stdout is used when omitted.
        #[arg(long, alias = "decompile-out")]
        out: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = DecompileFormat::Json, alias = "decompile-format")]
        format: DecompileFormat,
        /// Function name or Bnnn:$AAAA physical PRG-bank selector. Repeatable.
        #[arg(long = "function", alias = "decompile-function")]
        functions: Vec<String>,
        /// Also start a bounded candidate analysis at $8000 in every physical 8 KiB PRG bank.
        #[arg(long, alias = "decompile-all")]
        all: bool,
        /// JSON annotation sidecar. ROM.decompile.json is loaded automatically when present.
        #[arg(long, alias = "decompile-annotations")]
        annotations: Option<PathBuf>,
        /// Restore a version-2 snapshot before capturing the PRG-bank windows.
        #[arg(long)]
        snapshot: Option<PathBuf>,
        /// KUROSAKI trace JSONL to overlay exact physical-bank/PC hit counts.
        #[arg(long = "trace-jsonl", alias = "decompile-trace")]
        trace_jsonl: Option<PathBuf>,
        /// Per-function static-analysis cap, preventing data from causing unbounded output.
        #[arg(long, default_value_t = 2048)]
        max_instructions: usize,
    },
    Disasm {
        rom: PathBuf,
        #[arg(long, default_value_t = 256)]
        bytes: usize,
        /// CPU address in decimal or with a 0x/$ hexadecimal prefix.
        #[arg(long)]
        start: Option<String>,
        /// Restore a version 2 snapshot before reading mapped CPU bytes.
        #[arg(long, conflicts_with = "prg_bank_8k")]
        snapshot: Option<PathBuf>,
        /// Read one exact physical 8 KiB PRG-ROM bank, independent of mapper state.
        #[arg(long, conflicts_with = "snapshot")]
        prg_bank_8k: Option<u16>,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long)]
        text: Option<PathBuf>,
    },
    MapperTest {
        rom: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    FdsInspect {
        disk: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    ExportAssets {
        rom: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    Report {
        input: PathBuf,
        #[arg(long)]
        md: PathBuf,
    },
    MapperList {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long)]
        md: Option<PathBuf>,
    },
    MapperInfo {
        mapper: u16,
        #[arg(long)]
        json: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, ValueEnum)]
// Accepted diagnostic category labels. The dispatcher currently ignores
// --check selections; this enum does not activate separate checks.
enum CheckKind {
    Ppu,
    Nmi,
    Mapper,
    Apu,
    Abi,
    Input,
    Fds,
    Asset,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
// Available renderings of the same bounded static-analysis report.
enum DecompileFormat {
    Json,
    Markdown,
    Text,
}

#[derive(Debug, Serialize)]
// End-state registers and selected memory ranges for inspection, not a
// resumable checkpoint or proof of a particular gameplay path.
struct ObservationDump {
    format: &'static str,
    frame: u64,
    cpu_cycles: u64,
    instructions: u64,
    cpu: CpuObservationDump,
    ppu: PpuObservationDump,
    ranges: Vec<MemoryRangeDump>,
}

#[derive(Debug, Serialize)]
// Selected CPU fields copied after execution; cycles are in the outer record.
struct CpuObservationDump {
    pc: u16,
    a: u8,
    x: u8,
    y: u8,
    sp: u8,
    p: u8,
    stopped: bool,
}

#[derive(Debug, Serialize)]
// Stored PPU state with pixel-index statistics and rendering-write counters.
struct PpuObservationDump {
    ctrl: u8,
    mask: u8,
    status: u8,
    scanline: i16,
    dot: u16,
    vram_addr: u16,
    temp_addr: u16,
    rendered_frames: u64,
    last_pixel_hash: u64,
    frame_buffer_sha256: String,
    frame_buffer_unique_colors: usize,
    frame_buffer_non_backdrop_pixels: usize,
    ctrl_writes_while_rendering: u64,
    mask_writes_while_rendering: u64,
    scroll_writes_while_rendering: u64,
    addr_writes_while_rendering: u64,
    data_writes_while_rendering: u64,
}

#[derive(Debug, Serialize)]
// Complete hexadecimal byte dump plus digest/counts for one labelled range.
struct MemoryRangeDump {
    name: &'static str,
    address_space: &'static str,
    start: u16,
    len: usize,
    nonzero: usize,
    sha256: String,
    hex: String,
}

// Run Clap parsing and dispatch on an explicitly sized worker stack and
// propagate its result. Convert a worker panic into a CLI error.
fn main() -> Result<()> {
    // The CLI links the static decompiler and report renderers. On Windows the
    // executable entry thread can have a much smaller stack than Rust test
    // threads, so run the parser/dispatcher on an explicit working stack.
    let worker = std::thread::Builder::new()
        .name("kurosaki-cli".to_string())
        .stack_size(8 * 1024 * 1024)
        .spawn(run_cli)
        .context("failed to start KUROSAKI CLI worker thread")?;
    worker
        .join()
        .map_err(|_| anyhow::anyhow!("KUROSAKI CLI worker thread panicked"))?
}

// Dispatch the parsed command and construct its trace selectors. Handler
// return values determine exit status; several inspection/run handlers
// report a stopped emulation in output without returning a CLI failure.
fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::BatteryExport { rom, snapshot, out } => {
            let cart = Cartridge::load_file(rom)?;
            let snapshot: Snapshot = serde_json::from_str(&fs::read_to_string(snapshot)?)?;
            Emulator::from_snapshot(cart, &snapshot)?.save_battery_file(out)?;
            Ok(())
        }
        Commands::BatteryRun {
            rom,
            sav,
            frames,
            replay,
            json,
            snapshot,
            png,
            save_out,
        } => {
            let cart = Cartridge::load_file(rom)?;
            let mut emulator = Emulator::from_cartridge(cart)?;
            emulator.load_battery_file(sav)?;
            let replay: Option<Replay> = replay
                .map(|path| -> Result<Replay> {
                    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
                })
                .transpose()?;
            if let Some(replay) = &replay {
                if replay.rom_sha256 != emulator.cartridge.info.sha256
                    || replay.format != "kurosaki-replay-v1"
                {
                    bail!("battery replay ROM or format mismatch");
                }
            }
            emulator.reset(TraceConfig::none());
            let summary = emulator.run_current(RunOptions {
                frames,
                replay_frames: replay.map_or_else(Vec::new, |r| r.frames),
                ..RunOptions::default()
            });
            write_json_or_stdout(&summary, json)?;
            // Battery-run emits its summary first, then rejects a stopped run before
            // writing snapshot, PNG or a new save sidecar.
            if summary.stopped {
                bail!("battery run stopped: {:?}", summary.stop_reason);
            }
            if let Some(path) = snapshot {
                fs::write(path, serde_json::to_vec_pretty(&emulator.snapshot())?)?;
            }
            if let Some(path) = png {
                screen::write_emulator_png(&mut emulator, path)?;
            }
            if let Some(path) = save_out {
                emulator.save_battery_file(path)?;
            }
            Ok(())
        }
        Commands::InspectRom { rom, json, md } => inspect_rom(rom, json, md),
        Commands::AuditBoard {
            rom,
            expected_board,
            metadata,
            json,
        } => audit_board_command(rom, expected_board, metadata, json),
        Commands::Run {
            rom,
            frames,
            max_instructions,
            // The CLI always uses the headless core; this accepted switch does not
            // select an alternate execution path.
            headless: _,
            state_hash,
            json,
            md,
            png,
            snapshot,
            observe_json,
            trace_jsonl,
            emit_diagnostics,
            repro_bundle,
            break_on_diagnostic,
            png_on_diagnostic,
            snapshot_on_diagnostic,
            kitaqfc_debug,
            allow_unimplemented,
            pad1,
            pad2,
        } => run_rom(
            rom,
            frames,
            max_instructions,
            state_hash,
            json,
            md,
            png,
            snapshot,
            observe_json,
            trace_jsonl,
            emit_diagnostics,
            repro_bundle,
            break_on_diagnostic,
            png_on_diagnostic,
            snapshot_on_diagnostic,
            kitaqfc_debug,
            allow_unimplemented,
            pad1,
            pad2,
        ),
        Commands::Trace {
            rom,
            frames,
            out,
            cpu,
            mem_read,
            mem_write,
            ppu,
            apu,
            mapper,
            nmi,
            dma,
            full,
            kitaqfc_debug,
            allow_unimplemented,
            pad1,
            pad2,
        } => trace_rom(
            rom,
            frames,
            out,
            TraceConfig {
                cpu: cpu || full,
                mem_read: mem_read || full,
                mem_write: mem_write || full,
                ppu: ppu || full,
                apu: apu || full,
                mapper: mapper || full,
                nmi: nmi || full,
                dma: dma || full,
                source: full,
                compact: false,
            },
            kitaqfc_debug,
            allow_unimplemented,
            pad1,
            pad2,
        ),
        Commands::AudioExport {
            rom,
            frames,
            wav,
            json,
            allow_unimplemented,
            trace_audio,
            pad1,
            pad2,
        } => audio_export(
            rom,
            frames,
            wav,
            json,
            allow_unimplemented,
            trace_audio,
            pad1,
            pad2,
        ),
        Commands::Profile {
            rom,
            frames,
            out,
            md,
            kitaqfc_debug,
            allow_unimplemented,
        } => profile_rom(rom, frames, out, md, kitaqfc_debug, allow_unimplemented),
        Commands::Diagnose {
            rom,
            frames,
            out,
            md,
            emit_diagnostics,
            // Accepted check categories are currently discarded, so diagnose emits
            // the complete implemented report rather than a filtered subset.
            check: _,
            kitaqfc_debug,
            allow_unimplemented,
            pad1,
            pad2,
        } => diagnose_rom(
            rom,
            frames,
            out,
            md,
            emit_diagnostics,
            kitaqfc_debug,
            allow_unimplemented,
            pad1,
            pad2,
        ),
        Commands::ReplayRecord {
            rom,
            frames,
            out,
            allow_unimplemented,
        } => replay_record(rom, frames, out, allow_unimplemented),
        Commands::ReplayRun {
            rom,
            replay,
            load_snapshot,
            frames,
            verify,
            json,
            png,
            snapshot,
            observe_json,
            wav,
            trace_jsonl,
            trace_start_frame,
            trace_end_frame_exclusive,
            cpu,
            mem_read,
            mem_write,
            ppu,
            apu,
            mapper,
            nmi,
            dma,
            full,
            allow_unimplemented,
        } => replay_run(
            rom,
            replay,
            load_snapshot,
            frames,
            verify,
            json,
            png,
            snapshot,
            observe_json,
            wav,
            trace_jsonl,
            trace_start_frame,
            trace_end_frame_exclusive,
            TraceConfig {
                cpu: cpu || full,
                mem_read: mem_read || full,
                mem_write: mem_write || full,
                ppu: ppu || full,
                apu: apu || full,
                mapper: mapper || full,
                nmi: nmi || full,
                dma: dma || full,
                source: full,
                compact: false,
            },
            allow_unimplemented,
        ),
        Commands::SnapshotSave {
            rom,
            frames,
            out,
            allow_unimplemented,
            pad1,
            pad2,
        } => snapshot_save(rom, frames, out, allow_unimplemented, pad1, pad2),
        Commands::SnapshotLoad { snapshot, json } => snapshot_load(snapshot, json),
        Commands::SnapshotRebase {
            source_rom,
            target_rom,
            snapshot,
            contract,
            prg_ram_patches,
            cpu_ram_patches,
            out,
            json,
        } => snapshot_rebase(
            source_rom,
            target_rom,
            snapshot,
            contract,
            prg_ram_patches,
            cpu_ram_patches,
            out,
            json,
        ),
        Commands::SnapshotResume {
            rom,
            snapshot,
            reset_at_start,
            frames,
            out,
            json,
            png,
            observe_json,
            trace_jsonl,
            cpu,
            mem_read,
            mem_write,
            ppu,
            apu,
            mapper,
            nmi,
            dma,
            full,
            allow_unimplemented,
            pad1,
            pad2,
        } => snapshot_resume(
            rom,
            snapshot,
            reset_at_start,
            frames,
            out,
            json,
            png,
            observe_json,
            trace_jsonl,
            TraceConfig {
                cpu: cpu || full,
                mem_read: mem_read || full,
                mem_write: mem_write || full,
                ppu: ppu || full,
                apu: apu || full,
                mapper: mapper || full,
                nmi: nmi || full,
                dma: dma || full,
                source: full,
                compact: false,
            },
            allow_unimplemented,
            pad1,
            pad2,
        ),
        Commands::Diff {
            before,
            after,
            json,
            md,
        } => diff_snapshots(before, after, json, md),
        Commands::Decompile {
            rom,
            out,
            format,
            functions,
            all,
            annotations,
            snapshot,
            trace_jsonl,
            max_instructions,
        } => decompile_rom(
            rom,
            out,
            format,
            functions,
            all,
            annotations,
            snapshot,
            trace_jsonl,
            max_instructions,
        ),
        Commands::Disasm {
            rom,
            bytes,
            start,
            snapshot,
            prg_bank_8k,
            json,
            text,
        } => disasm_rom(rom, bytes, start, snapshot, prg_bank_8k, json, text),
        Commands::MapperTest { rom, json } => mapper_test(rom, json),
        Commands::FdsInspect { disk, json } => fds_inspect(disk, json),
        Commands::ExportAssets { rom, out } => export_assets(rom, out),
        Commands::Report { input, md } => report_file(input, md),
        Commands::MapperList { all, json, md } => mapper_list(all, json, md),
        Commands::MapperInfo { mapper, json } => mapper_info(mapper, json),
    }
}

// Load metadata, write optional JSON/Markdown, and print JSON when its
// path is absent. This performs no emulation; cartridge-level FDS loading
// can still require an external BIOS.
fn inspect_rom(rom: PathBuf, json: Option<PathBuf>, md: Option<PathBuf>) -> Result<()> {
    let cart = Cartridge::load_file(&rom)
        .with_context(|| format!("failed to load ROM {}", rom.display()))?;
    let pretty = serde_json::to_string_pretty(&cart.info)?;
    if let Some(path) = json {
        fs::write(path, &pretty)?;
    } else {
        println!("{pretty}");
    }
    if let Some(path) = md {
        fs::write(path, report::rom_info_markdown(&cart.info))?;
    }
    Ok(())
}

// Load optional metadata and emit the structural board audit before
// returning an error for a failed policy. A saved failing report is intentional.
fn audit_board_command(
    rom: PathBuf,
    expected_board: String,
    metadata: Option<PathBuf>,
    json: Option<PathBuf>,
) -> Result<()> {
    let cart = Cartridge::load_file(&rom)
        .with_context(|| format!("failed to load ROM {}", rom.display()))?;
    let metadata_value = metadata
        .as_ref()
        .map(|path| -> Result<serde_json::Value> {
            let bytes = fs::read(path)
                .with_context(|| format!("failed to read metadata {}", path.display()))?;
            serde_json::from_slice(&bytes)
                .with_context(|| format!("invalid metadata JSON {}", path.display()))
        })
        .transpose()?;
    let report = audit_board(&cart, &expected_board, metadata_value.as_ref());
    let passed = report.pass;
    write_json_or_stdout(&report, json)?;
    if !passed {
        bail!("board audit failed for expected profile {expected_board}");
    }
    Ok(())
}

// Capture selected CPU/PPU state and memory bytes, reading the currently
// mapped CHR window directly. The fixed queue range is only a guess; the
// first framebuffer pixel supplies the comparison backdrop, not palette zero.
fn observe_dump(emu: &mut Emulator) -> ObservationDump {
    let chr = (0..0x2000)
        .map(|address| emu.bus.mapper.read_chr(address))
        .collect::<Vec<_>>();
    let ppu = &emu.bus.ppu;
    // Use pixel zero as a heuristic comparison color. It may itself contain
    // foreground graphics, so non-backdrop count is not a visibility proof.
    let backdrop = ppu.frame_buffer.first().copied().unwrap_or_default();
    let frame_buffer_unique_colors = ppu
        .frame_buffer
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let frame_buffer_non_backdrop_pixels = ppu
        .frame_buffer
        .iter()
        .filter(|pixel| **pixel != backdrop)
        .count();
    ObservationDump {
        format: "kurosaki-observation-v1",
        frame: emu.frame,
        cpu_cycles: emu.cpu.cycles,
        instructions: emu.instructions,
        cpu: CpuObservationDump {
            pc: emu.cpu.pc,
            a: emu.cpu.a,
            x: emu.cpu.x,
            y: emu.cpu.y,
            sp: emu.cpu.sp,
            p: emu.cpu.p,
            stopped: emu.cpu.stopped,
        },
        ppu: PpuObservationDump {
            ctrl: ppu.ctrl,
            mask: ppu.mask,
            status: ppu.status,
            scanline: ppu.scanline,
            dot: ppu.dot,
            vram_addr: ppu.vram_addr,
            temp_addr: ppu.temp_addr,
            rendered_frames: ppu.rendered_frames,
            last_pixel_hash: ppu.last_pixel_hash,
            frame_buffer_sha256: sha256_hex(&ppu.frame_buffer),
            frame_buffer_unique_colors,
            frame_buffer_non_backdrop_pixels,
            ctrl_writes_while_rendering: ppu.ctrl_writes_while_rendering,
            mask_writes_while_rendering: ppu.mask_writes_while_rendering,
            scroll_writes_while_rendering: ppu.scroll_writes_while_rendering,
            addr_writes_while_rendering: ppu.addr_writes_while_rendering,
            data_writes_while_rendering: ppu.data_writes_while_rendering,
        },
        ranges: vec![
            memory_range("cpu_ram", "cpu", 0x0000, &emu.bus.ram),
            memory_range(
                // This fixed address range is labelled as a guess and is not resolved
                // from compiler metadata or the loaded program's allocation map.
                "vram_queue_guess",
                "cpu",
                0x0300,
                &emu.bus.ram[0x0300..0x03A0],
            ),
            memory_range("nametable0", "ppu", 0x2000, &ppu.vram[0x2000..0x2400]),
            memory_range("attribute0", "ppu", 0x23C0, &ppu.vram[0x23C0..0x2400]),
            memory_range("palette", "ppu", 0x3F00, &ppu.palette),
            memory_range("oam", "ppu-oam", 0x0000, &ppu.oam),
            memory_range("chr", "ppu-pattern", 0x0000, &chr),
        ],
    }
}

// Describe the supplied byte slice with a label, address, nonzero count,
// SHA-256 and full hex text. This does not read an address space itself.
fn memory_range(
    name: &'static str,
    address_space: &'static str,
    start: u16,
    bytes: &[u8],
) -> MemoryRangeDump {
    MemoryRangeDump {
        name,
        address_space,
        start,
        len: bytes.len(),
        nonzero: bytes.iter().filter(|b| **b != 0).count(),
        sha256: sha256_hex(bytes),
        hex: hex_bytes(bytes),
    }
}

// Format uppercase byte pairs separated by single spaces with no trailing space.
fn hex_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02X}");
    }
    out
}

#[allow(clippy::too_many_arguments)]
// Cold-run the requested frames, then export selected end-state artifacts
// and the summary. Diagnostic break/capture selectors are evaluated after
// the run; they do not interrupt execution at the first matching event.
// A stopped summary alone is not converted to a CLI error here.
fn run_rom(
    rom: PathBuf,
    frames: u64,
    max_instructions: Option<u64>,
    state_hash: Option<PathBuf>,
    json: Option<PathBuf>,
    md: Option<PathBuf>,
    png: Option<PathBuf>,
    snapshot: Option<PathBuf>,
    observe_json: Option<PathBuf>,
    trace_jsonl: Option<PathBuf>,
    emit_diagnostics: Option<PathBuf>,
    repro_bundle: Option<PathBuf>,
    break_on_diagnostic: Option<String>,
    png_on_diagnostic: Option<PathBuf>,
    snapshot_on_diagnostic: Option<PathBuf>,
    debug: Option<PathBuf>,
    allow_unimplemented: bool,
    pad1: u8,
    pad2: u8,
) -> Result<()> {
    let mut emu = load_emu(&rom)?;
    let wants_diagnostic_trace =
        emit_diagnostics.is_some() || repro_bundle.is_some() || break_on_diagnostic.is_some();
    let trace = if trace_jsonl.is_some() || repro_bundle.is_some() {
        TraceConfig::full()
    } else {
        diagnostic_trace_config(wants_diagnostic_trace)
    };
    let summary = emu.run(RunOptions {
        frames,
        max_instructions,
        trace,
        allow_unimplemented_opcode: allow_unimplemented,
        kitaqfc_debug: load_debug(debug)?,
        pad1,
        pad2,
        replay_frames: Vec::new(),
    });
    if let Some(path) = state_hash {
        fs::write(path, format!("{}\n", summary.final_state_hash))?;
    }
    if let Some(path) = png {
        screen::write_emulator_png(&mut emu, path)?;
    }
    if let Some(path) = snapshot {
        fs::write(path, serde_json::to_string_pretty(&emu.snapshot())?)?;
    }
    if let Some(path) = observe_json {
        fs::write(path, serde_json::to_string_pretty(&observe_dump(&mut emu))?)?;
    }
    if let Some(path) = trace_jsonl {
        fs::write(path, emu.trace.to_jsonl()?)?;
    }
    let diagnostic_events =
        diagnostic_events_from_trace_and_report(&emu.trace, &summary.diagnostics);
    if let Some(path) = emit_diagnostics {
        write_diagnostic_events_jsonl(&path, &diagnostic_events)?;
    }
    // Diagnostic capture is evaluated against the completed run. The selector
    // does not stop the core or preserve the state of the first matching event.
    let diagnostic_capture_requested =
        png_on_diagnostic.is_some() || snapshot_on_diagnostic.is_some();
    let diagnostic_match = break_on_diagnostic
        .as_deref()
        .map(|selector| diagnostic_break_matches(&diagnostic_events, selector))
        .unwrap_or(!diagnostic_events.is_empty());
    if diagnostic_capture_requested && diagnostic_match {
        if let Some(path) = png_on_diagnostic {
            screen::write_emulator_png(&mut emu, path)?;
        }
        if let Some(path) = snapshot_on_diagnostic {
            fs::write(path, serde_json::to_string_pretty(&emu.snapshot())?)?;
        }
    }
    if let Some(path) = repro_bundle {
        write_repro_bundle(&summary, &emu, &diagnostic_events, path)?;
    }
    write_json_or_stdout(&summary, json)?;
    if let Some(path) = md {
        fs::write(path, report::run_summary_markdown(&summary))?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
// Cold-run with the selected categories and replace the JSONL output.
// The run summary is discarded; successful file output does not certify
// that execution reached every requested frame.
fn trace_rom(
    rom: PathBuf,
    frames: u64,
    out: PathBuf,
    trace: TraceConfig,
    debug: Option<PathBuf>,
    allow_unimplemented: bool,
    pad1: u8,
    pad2: u8,
) -> Result<()> {
    let mut emu = load_emu(&rom)?;
    let _summary = emu.run(RunOptions {
        frames,
        max_instructions: None,
        trace,
        allow_unimplemented_opcode: allow_unimplemented,
        kitaqfc_debug: load_debug(debug)?,
        pad1,
        pad2,
        replay_frames: Vec::new(),
    });
    fs::write(out, emu.trace.to_jsonl()?)?;
    Ok(())
}

#[derive(Debug, Serialize)]
// Audio capture metadata from the resulting run; stopped/stop-reason fields
// are not included in this export-specific report.
struct AudioExportReport {
    format: &'static str,
    rom: PathBuf,
    frames: u64,
    sample_rate: u32,
    samples: usize,
    generated_samples: u64,
    dmc_sample_fetches: u64,
    dmc_dma_stall_cycles: u64,
    wav: PathBuf,
    final_state_hash: String,
}

#[allow(clippy::too_many_arguments)]
// Enable unbounded capture before the cold run, write the collected mono
// PCM and emit export metadata. Audio-trace selectors collect events in
// memory but this command has no separate trace-file output.
fn audio_export(
    rom: PathBuf,
    frames: u64,
    wav: PathBuf,
    json: Option<PathBuf>,
    allow_unimplemented: bool,
    trace_audio: bool,
    pad1: u8,
    pad2: u8,
) -> Result<()> {
    let mut emu = load_emu(&rom)?;
    emu.bus.apu.capture_full_audio = true;
    let summary = emu.run(RunOptions {
        frames,
        max_instructions: None,
        trace: if trace_audio {
            TraceConfig {
                apu: true,
                mapper: true,
                dma: true,
                ..TraceConfig::none()
            }
        } else {
            TraceConfig::none()
        },
        allow_unimplemented_opcode: allow_unimplemented,
        kitaqfc_debug: None,
        pad1,
        pad2,
        replay_frames: Vec::new(),
    });
    write_wav_pcm16_mono(&wav, emu.bus.apu.sample_rate, &emu.bus.apu.sample_buffer)
        .with_context(|| format!("failed to write WAV {}", wav.display()))?;
    let report_obj = AudioExportReport {
        format: "kurosaki-audio-export-v1",
        rom,
        frames: summary.frames,
        sample_rate: emu.bus.apu.sample_rate,
        samples: emu.bus.apu.sample_buffer.len(),
        generated_samples: emu.bus.apu.generated_samples,
        dmc_sample_fetches: emu.bus.apu.dmc_sample_fetches,
        dmc_dma_stall_cycles: emu.bus.apu.dmc_dma_stall_cycles,
        wav,
        final_state_hash: summary.final_state_hash,
    };
    write_json_or_stdout(&report_obj, json)
}

// Build a complete RIFF/WAVE PCM16 mono file in memory with little-endian
// lengths/samples, then replace the output. The format uses 32-bit sizes;
// this helper does not support RF64 or validate oversized capture lengths.
fn write_wav_pcm16_mono(path: &PathBuf, sample_rate: u32, samples: &[i16]) -> Result<()> {
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    let data_len = (samples.len() * 2) as u32;
    let riff_len = 36u32.saturating_add(data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_len.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    fs::write(path, out)?;
    Ok(())
}

// Cold-run with neutral inputs and tracing disabled, then report the
// summary and its core PC hotspots as JSON and optional Markdown.
fn profile_rom(
    rom: PathBuf,
    frames: u64,
    out: Option<PathBuf>,
    md: Option<PathBuf>,
    debug: Option<PathBuf>,
    allow_unimplemented: bool,
) -> Result<()> {
    let mut emu = load_emu(&rom)?;
    let summary = emu.run(RunOptions {
        frames,
        max_instructions: None,
        trace: TraceConfig::none(),
        allow_unimplemented_opcode: allow_unimplemented,
        kitaqfc_debug: load_debug(debug)?,
        pad1: 0,
        pad2: 0,
        replay_frames: Vec::new(),
    });
    write_json_or_stdout(&summary, out)?;
    if let Some(path) = md {
        fs::write(path, report::run_summary_markdown(&summary))?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
// With zero frames, inspect cartridge metadata only and skip debug-bundle
// loading. Otherwise cold-run and export the cumulative diagnostic report,
// optionally collecting events for JSONL. No diagnostic filter is applied here.
fn diagnose_rom(
    rom: PathBuf,
    frames: u64,
    out: Option<PathBuf>,
    md: Option<PathBuf>,
    emit_diagnostics: Option<PathBuf>,
    debug: Option<PathBuf>,
    allow_unimplemented: bool,
    pad1: u8,
    pad2: u8,
) -> Result<()> {
    if frames == 0 {
        let cart = Cartridge::load_file(&rom)?;
        let report_obj = DiagnosticReport::from_rom_info(&cart.info);
        if let Some(path) = &emit_diagnostics {
            write_diagnostic_events_jsonl(
                path,
                &diagnostic_events_from_trace_and_report(&Default::default(), &report_obj),
            )?;
        }
        write_json_or_stdout(&report_obj, out)?;
        if let Some(path) = md {
            fs::write(path, report::diagnostics_markdown(&report_obj))?;
        }
        return Ok(());
    }
    let mut emu = load_emu(&rom)?;
    let summary = emu.run(RunOptions {
        frames,
        max_instructions: None,
        trace: diagnostic_trace_config(emit_diagnostics.is_some()),
        allow_unimplemented_opcode: allow_unimplemented,
        kitaqfc_debug: load_debug(debug)?,
        pad1,
        pad2,
        replay_frames: Vec::new(),
    });
    if let Some(path) = &emit_diagnostics {
        write_diagnostic_events_jsonl(
            path,
            &diagnostic_events_from_trace_and_report(&emu.trace, &summary.diagnostics),
        )?;
    }
    write_json_or_stdout(&summary.diagnostics, out)?;
    if let Some(path) = md {
        fs::write(path, report::diagnostics_markdown(&summary.diagnostics))?;
    }
    Ok(())
}

// Cold-run neutral input and generate one neutral replay record for every
// requested frame, attaching the resulting observation hash. This command
// does not record live controller input or shorten entries after an early stop.
fn replay_record(rom: PathBuf, frames: u64, out: PathBuf, allow_unimplemented: bool) -> Result<()> {
    let mut emu = load_emu(&rom)?;
    let summary = emu.run(RunOptions {
        frames,
        max_instructions: None,
        trace: TraceConfig::none(),
        allow_unimplemented_opcode: allow_unimplemented,
        kitaqfc_debug: None,
        pad1: 0,
        pad2: 0,
        replay_frames: Vec::new(),
    });
    let mut replay = Replay::empty(summary.rom_sha256);
    replay.frames = (0..frames)
        .map(|frame| ReplayFrame {
            frame,
            pad1: 0,
            pad2: 0,
            reset: false,
            disk_side: None,
            expected_frame_hash: None,
            duration: None,
        })
        .collect();
    replay.expected_final_state_hash = Some(summary.final_state_hash);
    fs::write(out, serde_json::to_string_pretty(&replay)?)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
// Apply a replay to a cold or strictly restored emulator using an absolute
// final-frame target. Optionally capture a bounded trace window and verify
// a supplied final observation hash. Unlike battery-run, this handler does
// not validate replay format/ROM identity, and --verify is conditional on
// an expected final hash being present.
fn replay_run(
    rom: PathBuf,
    replay_path: PathBuf,
    load_snapshot: Option<PathBuf>,
    frames: Option<u64>,
    verify: bool,
    json: Option<PathBuf>,
    png: Option<PathBuf>,
    snapshot: Option<PathBuf>,
    observe_json: Option<PathBuf>,
    wav: Option<PathBuf>,
    trace_jsonl: Option<PathBuf>,
    trace_start_frame: Option<u64>,
    trace_end_frame_exclusive: Option<u64>,
    trace: TraceConfig,
    allow_unimplemented: bool,
) -> Result<()> {
    let replay_source = fs::read(&replay_path)?;
    let replay_sha256 = sha256_hex(&replay_source);
    let replay: Replay = serde_json::from_slice(&replay_source)?;
    let replay_frame_count = replay
        .frames
        .iter()
        .map(|frame| frame.end_frame_exclusive())
        .max()
        .unwrap_or(0);
    let total_frames = frames.unwrap_or(replay_frame_count);
    let restored_from_snapshot = load_snapshot.is_some();
    let mut emu = match load_snapshot {
        Some(snapshot_path) => {
            let cart = Cartridge::load_file(&rom)
                .with_context(|| format!("failed to load ROM {}", rom.display()))?;
            let snapshot: Snapshot =
                serde_json::from_str(&fs::read_to_string(&snapshot_path).with_context(|| {
                    format!("failed to read snapshot {}", snapshot_path.display())
                })?)
                .with_context(|| format!("failed to parse snapshot {}", snapshot_path.display()))?;
            Emulator::from_snapshot(cart, &snapshot)?
        }
        None => load_emu(&rom)?,
    };
    // Replay --frames is the absolute target, unlike snapshot-resume
    // where --frames is an additional duration.
    let initial_frame = emu.frame;
    emu.bus.apu.capture_full_audio = wav.is_some();
    if total_frames < initial_frame {
        bail!(
            "replay target frame {} precedes restored snapshot frame {}",
            total_frames,
            initial_frame
        );
    }
    let summary = if let Some(trace_path) = trace_jsonl {
        let (start_frame, end_frame_exclusive) = validate_replay_trace_window(
            total_frames,
            initial_frame,
            trace_start_frame,
            trace_end_frame_exclusive,
            trace,
        )?;

        if !restored_from_snapshot {
            emu.reset(TraceConfig::none());
        }
        if start_frame > initial_frame {
            let prefix = emu.run_current(replay_run_options(
                start_frame - initial_frame,
                TraceConfig::none(),
                allow_unimplemented,
                &replay,
            ));
            if prefix.stopped || emu.frame != start_frame {
                bail!(
                    "replay stopped before trace window at frame {}; requested start frame {}",
                    emu.frame,
                    start_frame
                );
            }
        }

        // Discard untraced prefix/reset history so the serialized capture starts
        // with the explicit window marker.
        emu.trace.events.clear();
        push_replay_trace_window_marker(
            &mut emu,
            "replay.trace_window_start",
            start_frame,
            end_frame_exclusive,
            total_frames,
            &replay_sha256,
            replay.frames.len(),
            trace,
            None,
        );
        let window = emu.run_current(replay_run_options(
            end_frame_exclusive - start_frame,
            trace,
            allow_unimplemented,
            &replay,
        ));
        if window.stopped || emu.frame != end_frame_exclusive {
            bail!(
                "replay stopped inside trace window at frame {}; requested end frame {}",
                emu.frame,
                end_frame_exclusive
            );
        }
        let captured_events = emu.trace.events.len().saturating_sub(1);
        push_replay_trace_window_marker(
            &mut emu,
            "replay.trace_window_end",
            start_frame,
            end_frame_exclusive,
            total_frames,
            &replay_sha256,
            replay.frames.len(),
            trace,
            Some(captured_events),
        );
        // Freeze the bounded window before running its untraced suffix; later
        // events retained in the emulator cannot leak into this saved string.
        let trace_jsonl = emu.trace.to_jsonl()?;

        let suffix_frames = total_frames - end_frame_exclusive;
        let final_summary = if suffix_frames > 0 {
            emu.run_current(replay_run_options(
                suffix_frames,
                TraceConfig::none(),
                allow_unimplemented,
                &replay,
            ))
        } else {
            window
        };
        fs::write(trace_path, trace_jsonl)?;
        final_summary
    } else {
        if trace_start_frame.is_some() || trace_end_frame_exclusive.is_some() {
            bail!("trace frame bounds require --trace-jsonl");
        }
        if restored_from_snapshot {
            emu.run_current(replay_run_options(
                total_frames - initial_frame,
                TraceConfig::none(),
                allow_unimplemented,
                &replay,
            ))
        } else {
            emu.run(replay_run_options(
                total_frames,
                TraceConfig::none(),
                allow_unimplemented,
                &replay,
            ))
        }
    };
    // Verify only the optional final observation hash. Expected per-frame
    // and trace hashes are not checked by this path.
    if verify {
        if let Some(expected) = replay.expected_final_state_hash {
            if expected != summary.final_state_hash {
                anyhow::bail!(
                    "replay verification failed: expected {}, got {}",
                    expected,
                    summary.final_state_hash
                );
            }
        }
    }
    if let Some(path) = png {
        screen::write_emulator_png(&mut emu, path)?;
    }
    if let Some(path) = snapshot {
        fs::write(path, serde_json::to_string_pretty(&emu.snapshot())?)?;
    }
    if let Some(path) = observe_json {
        fs::write(path, serde_json::to_string_pretty(&observe_dump(&mut emu))?)?;
    }
    if let Some(path) = wav {
        write_wav_pcm16_mono(&path, emu.bus.apu.sample_rate, &emu.bus.apu.sample_buffer)
            .with_context(|| format!("failed to write WAV {}", path.display()))?;
    }
    write_json_or_stdout(&summary, json)
}

// Clone the replay intervals into a core continuation request with neutral
// fallback inputs, selected tracing and no instruction cap or debug bundle.
fn replay_run_options(
    frames: u64,
    trace: TraceConfig,
    allow_unimplemented: bool,
    replay: &Replay,
) -> RunOptions {
    RunOptions {
        frames,
        max_instructions: None,
        trace,
        allow_unimplemented_opcode: allow_unimplemented,
        kitaqfc_debug: None,
        pad1: 0,
        pad2: 0,
        replay_frames: replay.frames.clone(),
    }
}

// Require explicit nonempty bounds within [initial_frame,total_frames]
// and at least one trace category. Return absolute start/exclusive-end values.
fn validate_replay_trace_window(
    total_frames: u64,
    initial_frame: u64,
    start_frame: Option<u64>,
    end_frame_exclusive: Option<u64>,
    trace: TraceConfig,
) -> Result<(u64, u64)> {
    let start_frame = start_frame.context("--trace-start-frame is required with --trace-jsonl")?;
    let end_frame_exclusive = end_frame_exclusive
        .context("--trace-end-frame-exclusive is required with --trace-jsonl")?;
    if start_frame >= end_frame_exclusive {
        bail!(
            "trace window must be non-empty: start {} must be less than end {}",
            start_frame,
            end_frame_exclusive
        );
    }
    if start_frame < initial_frame {
        bail!(
            "trace window start {} precedes restored snapshot frame {}",
            start_frame,
            initial_frame
        );
    }
    if end_frame_exclusive > total_frames {
        bail!(
            "trace window end {} exceeds replay run length {}",
            end_frame_exclusive,
            total_frames
        );
    }
    if !trace.cpu
        && !trace.mem_read
        && !trace.mem_write
        && !trace.ppu
        && !trace.apu
        && !trace.mapper
        && !trace.nmi
        && !trace.dma
        && !trace.source
    {
        bail!("at least one replay trace category or --full is required");
    }
    Ok((start_frame, end_frame_exclusive))
}

#[allow(clippy::too_many_arguments)]
// Append a boundary event carrying the replay file hash, interval, entry
// count and selected categories. The optional captured count excludes the
// start marker; this marker does not alter execution.
fn push_replay_trace_window_marker(
    emu: &mut Emulator,
    kind: &str,
    start_frame: u64,
    end_frame_exclusive: u64,
    total_frames: u64,
    replay_sha256: &str,
    replay_entries: usize,
    trace: TraceConfig,
    captured_events: Option<usize>,
) {
    let mut marker = TraceEvent::new(kind, emu.frame, emu.cpu.cycles);
    let mut details = BTreeMap::new();
    details.insert(
        "schema".to_owned(),
        Value::String("kurosaki.replay_trace_window.v1".to_owned()),
    );
    details.insert("start_frame".to_owned(), Value::from(start_frame));
    details.insert(
        "end_frame_exclusive".to_owned(),
        Value::from(end_frame_exclusive),
    );
    details.insert("total_frames".to_owned(), Value::from(total_frames));
    details.insert(
        "replay_sha256".to_owned(),
        Value::String(replay_sha256.to_owned()),
    );
    details.insert("replay_entries".to_owned(), Value::from(replay_entries));
    details.insert(
        "trace_categories".to_owned(),
        serde_json::to_value(trace).expect("TraceConfig serialization cannot fail"),
    );
    if let Some(captured_events) = captured_events {
        details.insert("captured_events".to_owned(), Value::from(captured_events));
    }
    marker.details = Some(details);
    emu.trace.push(marker);
}

// Cold-run and export the resulting v2 state even if execution stopped
// early. This handler does not emit or validate the run summary.
fn snapshot_save(
    rom: PathBuf,
    frames: u64,
    out: PathBuf,
    allow_unimplemented: bool,
    pad1: u8,
    pad2: u8,
) -> Result<()> {
    let mut emu = load_emu(&rom)?;
    let _ = emu.run(RunOptions {
        frames,
        max_instructions: None,
        trace: TraceConfig::none(),
        allow_unimplemented_opcode: allow_unimplemented,
        kitaqfc_debug: None,
        pad1,
        pad2,
        replay_frames: Vec::new(),
    });
    fs::write(out, serde_json::to_string_pretty(&emu.snapshot())?)?;
    Ok(())
}

// Parse and print/copy a v2 snapshot after checking only its format label.
// Without a cartridge this is inspection, not fingerprint-strict restoration.
fn snapshot_load(snapshot: PathBuf, json: Option<PathBuf>) -> Result<()> {
    let snap: Snapshot = serde_json::from_str(&fs::read_to_string(snapshot)?)?;
    if snap.format != "kurosaki-snapshot-v2" {
        bail!(
            "unsupported snapshot format {}; expected kurosaki-snapshot-v2",
            snap.format
        );
    }
    write_json_or_stdout(&snap, json)
}

#[derive(Debug, Serialize)]
// Flatten the successful core contract report and add the exact serialized
// output-file digest, distinct from the mapper-private payload digest.
struct SnapshotRebaseCliReport {
    #[serde(flatten)]
    rebase: SnapshotRebaseReport,
    output_snapshot_file_sha256: String,
}

#[allow(clippy::too_many_arguments)]
// Read both ROMs, exact snapshot bytes, contract and supplied patches; bind
// the file digest to those parsed snapshot bytes and call the strict rebase
// API. Create outputs exclusively, and remove the new snapshot best-effort
// if creation/writing of the requested report fails.
fn snapshot_rebase(
    source_rom: PathBuf,
    target_rom: PathBuf,
    snapshot: PathBuf,
    contract: PathBuf,
    patch_specs: Vec<PrgRamPatchSpec>,
    cpu_patch_specs: Vec<CpuRamPatchSpec>,
    out: PathBuf,
    json: Option<PathBuf>,
) -> Result<()> {
    if json.as_ref() == Some(&out) {
        bail!("snapshot output and JSON report paths must be different");
    }
    let source_cart = Cartridge::load_file(&source_rom)
        .with_context(|| format!("failed to load source ROM {}", source_rom.display()))?;
    let target_cart = Cartridge::load_file(&target_rom)
        .with_context(|| format!("failed to load target ROM {}", target_rom.display()))?;
    let snapshot_bytes = fs::read(&snapshot)
        .with_context(|| format!("failed to read source snapshot {}", snapshot.display()))?;
    let source_snapshot: Snapshot = serde_json::from_slice(&snapshot_bytes)
        .with_context(|| format!("failed to parse source snapshot {}", snapshot.display()))?;
    let contract_bytes = fs::read(&contract)
        .with_context(|| format!("failed to read rebase contract {}", contract.display()))?;
    let contract: SnapshotRebaseContract = serde_json::from_slice(&contract_bytes)
        .with_context(|| format!("failed to parse rebase contract {}", contract.display()))?;

    let patches = patch_specs
        .into_iter()
        .map(|spec| {
            let bytes = fs::read(&spec.path)
                .with_context(|| format!("failed to read PRG-RAM patch {}", spec.path.display()))?;
            Ok(PrgRamPatch {
                cpu_address: spec.cpu_address,
                bytes,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let cpu_patches = cpu_patch_specs
        .into_iter()
        .map(|spec| CpuRamPatch {
            cpu_address: spec.cpu_address,
            bytes: spec.bytes,
        })
        .collect::<Vec<_>>();

    let (rebased, rebase_report) = rebase_snapshot_state(
        source_cart,
        target_cart,
        &source_snapshot,
        &sha256_hex(&snapshot_bytes),
        &contract,
        &patches,
        &cpu_patches,
    )?;
    let output_bytes = serde_json::to_vec_pretty(&rebased)?;
    let report = SnapshotRebaseCliReport {
        rebase: rebase_report,
        output_snapshot_file_sha256: sha256_hex(&output_bytes),
    };

    // Create the snapshot first. Report serialization/write failure can
    // leave different partial effects; only write_create_new failure triggers
    // the following best-effort removal of the new snapshot.
    write_create_new_bytes(&out, &output_bytes)?;
    if let Some(path) = json {
        let report_bytes = serde_json::to_vec_pretty(&report)?;
        if let Err(error) = write_create_new_bytes(&path, &report_bytes) {
            let _ = fs::remove_file(&out);
            return Err(error);
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
// Strictly restore the supplied ROM/snapshot, optionally reset the CPU,
// then run an additional frame count with explicit input masks. Export the
// resulting trace, image, observations/state and cumulative summary.
fn snapshot_resume(
    rom: PathBuf,
    snapshot: PathBuf,
    reset_at_start: bool,
    frames: u64,
    out: Option<PathBuf>,
    json: Option<PathBuf>,
    png: Option<PathBuf>,
    observe_json: Option<PathBuf>,
    trace_jsonl: Option<PathBuf>,
    trace: TraceConfig,
    allow_unimplemented: bool,
    pad1: u8,
    pad2: u8,
) -> Result<()> {
    let cart = Cartridge::load_file(&rom)
        .with_context(|| format!("failed to load ROM {}", rom.display()))?;
    let snapshot: Snapshot = serde_json::from_str(
        &fs::read_to_string(&snapshot)
            .with_context(|| format!("failed to read snapshot {}", snapshot.display()))?,
    )?;
    let mut emu = Emulator::from_snapshot(cart, &snapshot)?;
    if reset_at_start {
        emu.reset(trace);
    }
    let summary = emu.run_current(RunOptions {
        frames,
        max_instructions: None,
        trace,
        allow_unimplemented_opcode: allow_unimplemented,
        kitaqfc_debug: None,
        pad1,
        pad2,
        replay_frames: Vec::new(),
    });
    if let Some(path) = trace_jsonl {
        fs::write(path, emu.trace.to_jsonl()?)?;
    }
    if let Some(path) = png {
        screen::write_emulator_png(&mut emu, path)?;
    }
    if let Some(path) = observe_json {
        fs::write(path, serde_json::to_string_pretty(&observe_dump(&mut emu))?)?;
    }
    if let Some(path) = out {
        fs::write(path, serde_json::to_string_pretty(&emu.snapshot())?)?;
    }
    write_json_or_stdout(&summary, json)
}

#[derive(Debug, Serialize)]
// Selected identity-string comparison, intentionally smaller than a full state diff.
struct SnapshotDiffReport {
    format: &'static str,
    before_rom_sha256: String,
    after_rom_sha256: String,
    same_rom: bool,
    before_frame: u64,
    after_frame: u64,
    before_mapper_private_sha256: String,
    after_mapper_private_sha256: String,
    same_mapper_private: bool,
}

// Compare stored ROM and mapper-payload hash strings plus frame labels.
// This is not a full CPU/bus diff and does not recompute hashes or validate
// restoration; equal reported digests can conceal inconsistent input payloads.
fn diff_snapshots(
    before: PathBuf,
    after: PathBuf,
    json: Option<PathBuf>,
    md: Option<PathBuf>,
) -> Result<()> {
    let a: Snapshot = serde_json::from_str(&fs::read_to_string(before)?)?;
    let b: Snapshot = serde_json::from_str(&fs::read_to_string(after)?)?;
    let report_obj = SnapshotDiffReport {
        format: "kurosaki-snapshot-diff-v1",
        before_rom_sha256: a.rom_sha256.clone(),
        after_rom_sha256: b.rom_sha256.clone(),
        same_rom: a.rom_sha256 == b.rom_sha256,
        before_frame: a.frame,
        after_frame: b.frame,
        before_mapper_private_sha256: a.mapper_private_sha256.clone(),
        after_mapper_private_sha256: b.mapper_private_sha256.clone(),
        same_mapper_private: a.mapper_private_sha256 == b.mapper_private_sha256,
    };
    write_json_or_stdout(&report_obj, json)?;
    if let Some(path) = md {
        fs::write(path, format!("# KUROSAKI Snapshot Diff\n\n- Same ROM: `{}`\n- Before frame: `{}`\n- After frame: `{}`\n- Same mapper private state: `{}`\n", report_obj.same_rom, report_obj.before_frame, report_obj.after_frame, report_obj.same_mapper_private))?;
    }
    Ok(())
}

// Choose exact physical-bank disassembly first, mapped disassembly for
// a start address or snapshot, otherwise the legacy reset-window path.
// The legacy path still prints text with --json alone; mapped/physical paths
// suppress stdout when a JSON output was requested.
fn disasm_rom(
    rom: PathBuf,
    bytes: usize,
    start: Option<String>,
    snapshot: Option<PathBuf>,
    prg_bank_8k: Option<u16>,
    json: Option<PathBuf>,
    text: Option<PathBuf>,
) -> Result<()> {
    let cart = Cartridge::load_file(&rom)?;
    if let Some(prg_bank_8k) = prg_bank_8k {
        let start = match start {
            Some(value) => parse_cpu_address(&value)?,
            None => 0x8000,
        };
        let lines = disasm::disassemble_physical_prg_bank_8k(&cart, prg_bank_8k, start, bytes)?;
        if let Some(path) = &json {
            fs::write(path, serde_json::to_string_pretty(&lines)?)?;
        }
        let rendered = lines
            .iter()
            .map(|line| {
                format!(
                    "B{:03} {:04X}: {:<10} {}",
                    line.prg_bank_8k,
                    line.cpu_addr,
                    line.bytes
                        .iter()
                        .map(|byte| format!("{byte:02X}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                    line.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        if let Some(path) = &text {
            fs::write(path, rendered)?;
        } else if json.is_none() {
            print!("{rendered}");
        }
        return Ok(());
    }
    if start.is_some() || snapshot.is_some() {
        let snapshot_value = match snapshot {
            Some(path) => Some(
                serde_json::from_str::<Snapshot>(
                    &fs::read_to_string(&path)
                        .with_context(|| format!("failed to read snapshot {}", path.display()))?,
                )
                .with_context(|| format!("failed to parse snapshot {}", path.display()))?,
            ),
            None => None,
        };
        let emulator = match snapshot_value {
            Some(snapshot) => Emulator::from_snapshot(cart, &snapshot)?,
            None => {
                let mut emulator = Emulator::from_cartridge(cart)?;
                emulator.reset(TraceConfig::none());
                emulator
            }
        };
        let start = match start {
            Some(value) => parse_cpu_address(&value)?,
            None => emulator.cpu.pc,
        };
        let lines = disasm::disassemble_mapped_range(&emulator, start, bytes)?;
        if let Some(path) = &json {
            fs::write(path, serde_json::to_string_pretty(&lines)?)?;
        }
        let rendered = lines
            .iter()
            .map(|line| {
                format!(
                    "{:04X}: {:<10} {}",
                    line.cpu_addr,
                    line.bytes
                        .iter()
                        .map(|byte| format!("{byte:02X}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                    line.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        if let Some(path) = &text {
            fs::write(path, rendered)?;
        } else if json.is_none() {
            print!("{rendered}");
        }
        return Ok(());
    }
    let lines = disasm::disassemble_reset_window(&cart, bytes);
    if let Some(path) = json {
        fs::write(path, serde_json::to_string_pretty(&lines)?)?;
    }
    let rendered = lines
        .iter()
        .map(|l| {
            format!(
                "{:04X}: {:<10} {}",
                l.cpu_addr,
                l.bytes
                    .iter()
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join(" "),
                l.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    if let Some(path) = text {
        fs::write(path, rendered)?;
    } else {
        print!("{rendered}");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
// Build the initial or restored mapper context, run bounded static analysis,
// then overlay optional annotations and parsed trace events before rendering.
// Automatic annotation lookup replaces the ROM extension with .decompile.json;
// these inputs describe analysis context, not newly executed verification.
fn decompile_rom(
    rom: PathBuf,
    out: Option<PathBuf>,
    format: DecompileFormat,
    functions: Vec<String>,
    all: bool,
    annotations: Option<PathBuf>,
    snapshot: Option<PathBuf>,
    trace_jsonl: Option<PathBuf>,
    max_instructions: usize,
) -> Result<()> {
    let cart = Cartridge::load_file(&rom)
        .with_context(|| format!("failed to load ROM {}", rom.display()))?;
    let emulator = match snapshot {
        Some(path) => {
            let snapshot: Snapshot = serde_json::from_str(
                &fs::read_to_string(&path)
                    .with_context(|| format!("failed to read snapshot {}", path.display()))?,
            )
            .with_context(|| format!("failed to parse snapshot {}", path.display()))?;
            Emulator::from_snapshot(cart, &snapshot)?
        }
        None => {
            let mut emulator = Emulator::from_cartridge(cart)?;
            emulator.reset(TraceConfig::none());
            emulator
        }
    };
    let mut report = analyze_emulator(
        &emulator,
        &DecompileOptions {
            selected_functions: functions,
            include_all_prg_banks: all,
            max_instructions_per_function: max_instructions,
        },
    )?;

    // An explicit sidecar wins; otherwise auto-load ROM-with-replaced-extension
    // when present. Annotation content is applied after initial function selection.
    let annotation_path = annotations.or_else(|| {
        let sidecar = rom.with_extension("decompile.json");
        sidecar.exists().then_some(sidecar)
    });
    if let Some(path) = annotation_path {
        let annotations: DecompileAnnotationFile = serde_json::from_str(
            &fs::read_to_string(&path)
                .with_context(|| format!("failed to read annotations {}", path.display()))?,
        )
        .with_context(|| format!("failed to parse annotations {}", path.display()))?;
        let applied = apply_annotations(&mut report, &annotations);
        report.notes.push(format!(
            "Applied {applied} annotation override(s) from {}.",
            path.display()
        ));
    }
    if let Some(path) = trace_jsonl {
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read trace JSONL {}", path.display()))?;
        let events = text
            .lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .map(|(line_number, line)| {
                serde_json::from_str::<TraceEvent>(line).with_context(|| {
                    format!(
                        "failed to parse trace JSONL {} at line {}",
                        path.display(),
                        line_number + 1
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let matched = apply_trace_events(&mut report, &events);
        report.notes.push(format!(
            "Trace overlay matched {matched} CPU instruction event(s) from {}.",
            path.display()
        ));
    }

    let rendered = match format {
        DecompileFormat::Json => serde_json::to_string_pretty(&report)?,
        DecompileFormat::Markdown => render_decompile_markdown(&report),
        DecompileFormat::Text => render_decompile_text(&report),
    };
    if let Some(path) = out {
        fs::write(&path, rendered)
            .with_context(|| format!("failed to write decompile report {}", path.display()))?;
    } else {
        println!("{rendered}");
    }
    Ok(())
}

// Trim outer whitespace and parse a 16-bit decimal address or 0x/0X/$
// hexadecimal address, returning a contextual error on malformed input.
fn parse_cpu_address(value: &str) -> Result<u16> {
    let trimmed = value.trim();
    let (radix, digits) = if let Some(hex) = trimmed.strip_prefix("0x") {
        (16, hex)
    } else if let Some(hex) = trimmed.strip_prefix("0X") {
        (16, hex)
    } else if let Some(hex) = trimmed.strip_prefix('$') {
        (16, hex)
    } else {
        (10, trimmed)
    };
    u16::from_str_radix(digits, radix)
        .with_context(|| format!("invalid CPU address {value}; expected 0..65535 or hexadecimal"))
}

// Emit header/registry diagnostics only. Despite the command name, this
// does not execute mapper-register tests or CPU instructions.
fn mapper_test(rom: PathBuf, json: Option<PathBuf>) -> Result<()> {
    let cart = Cartridge::load_file(&rom)?;
    let report_obj = DiagnosticReport::from_rom_info(&cart.info);
    write_json_or_stdout(&report_obj, json)
}

#[derive(Debug, Serialize)]
// Disk identity and parsed-file inventory; no BIOS or raw file payload is embedded.
struct FdsInspectReport {
    format: &'static str,
    path: PathBuf,
    file_size: usize,
    sha256: String,
    has_header: bool,
    side_count: u8,
    file_count: usize,
    fast_boot_pc: Option<u16>,
    files: Vec<FdsDiskFile>,
    warnings: Vec<String>,
    note: String,
}

// Parse disk blocks directly and report their metadata and inferred boot
// vector without requiring a BIOS. File payload bytes are omitted by their
// serialization policy.
fn fds_inspect(disk: PathBuf, json: Option<PathBuf>) -> Result<()> {
    let bytes = fs::read(&disk)?;
    let parsed = FdsDiskImage::from_bytes(&bytes)?;
    let report_obj = FdsInspectReport {
        format: "kurosaki-fds-inspect-v1",
        path: disk,
        file_size: bytes.len(),
        sha256: kurosaki_core::hash::sha256_hex(&bytes),
        has_header: parsed.has_header,
        side_count: parsed.side_count,
        file_count: parsed.files.len(),
        fast_boot_pc: parsed.fast_boot_pc(),
        files: parsed.files,
        warnings: parsed.warnings,
        note: "FDS inspect parses disk blocks and direct-boot metadata; run/load uses disksys.rom plus boot-file preloading for current KITAQFC FDS images.".to_string(),
    };
    write_json_or_stdout(&report_obj, json)
}

// Create the output directory and replace raw CHR-ROM, metadata and a
// brief README. CHR-RAM-only cartridges produce an empty chr.bin; this
// does not capture runtime patterns, nametables or palettes.
fn export_assets(rom: PathBuf, out: PathBuf) -> Result<()> {
    let cart = Cartridge::load_file(&rom)?;
    fs::create_dir_all(&out)?;
    fs::write(
        out.join("rom_info.json"),
        serde_json::to_string_pretty(&cart.info)?,
    )?;
    fs::write(out.join("chr.bin"), &cart.chr_rom)?;
    fs::write(out.join("README.md"), "# KUROSAKI Asset Export\n\nPhase 0-3 exports raw CHR data and ROM metadata only. Structured CHR/nametable/palette extraction is scheduled for later phases.\n")?;
    Ok(())
}

// Wrap arbitrary parsed JSON in a Markdown code block with a best-effort
// format label. This does not validate a report schema or escape Markdown
// delimiters embedded in the label or payload.
fn report_file(input: PathBuf, md: PathBuf) -> Result<()> {
    let text = fs::read_to_string(input)?;
    let value: serde_json::Value = serde_json::from_str(&text)?;
    let format = value
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    fs::write(
        md,
        format!(
            "# KUROSAKI Report\n\nInput format: `{format}`\n\n```json\n{}\n```\n",
            serde_json::to_string_pretty(&value)?
        ),
    )?;
    Ok(())
}

// Load the cartridge and construct its mapper/bus without resetting the CPU.
fn load_emu(rom: &PathBuf) -> Result<Emulator> {
    let cart = Cartridge::load_file(rom)?;
    Ok(Emulator::from_cartridge(cart)?)
}

// Read an optional generic compiler-debug JSON bundle; return None when absent.
fn load_debug(path: Option<PathBuf>) -> Result<Option<KitaqfcDebugBundle>> {
    match path {
        Some(p) => Ok(Some(KitaqfcDebugBundle::load(p)?)),
        None => Ok(None),
    }
}

// Select PPU/APU/mapper/NMI/DMA observations when requested, leaving CPU
// and generic memory tracing off.
fn diagnostic_trace_config(enabled: bool) -> TraceConfig {
    if enabled {
        TraceConfig {
            ppu: true,
            apu: true,
            mapper: true,
            nmi: true,
            dma: true,
            ..TraceConfig::none()
        }
    } else {
        TraceConfig::none()
    }
}

// Trim and ASCII-fold the selector, then match event type or summary-key
// substrings. Empty/ALL/* means any existing event; unlike the Python helper,
// an empty event list never matches these wildcard selectors.
fn diagnostic_break_matches(events: &[DiagnosticEvent], selector: &str) -> bool {
    let normalized = selector.trim().to_ascii_uppercase();
    if normalized.is_empty() || normalized == "ALL" || normalized == "*" {
        return !events.is_empty();
    }
    events.iter().any(|event| {
        event.event_type.to_ascii_uppercase().contains(&normalized)
            || event.summary_key.to_ascii_uppercase().contains(&normalized)
    })
}

// Create parent directories, serialize one event per line and replace the
// output. Empty events produce an empty file.
fn write_diagnostic_events_jsonl(path: &PathBuf, events: &[DiagnosticEvent]) -> Result<()> {
    ensure_parent(path)?;
    let mut out = String::new();
    for event in events {
        out.push_str(&serde_json::to_string(event)?);
        out.push('\n');
    }
    fs::write(path, out)?;
    Ok(())
}

// Create a deflated ZIP of supplied end-state data and descriptive plans.
// The retest command is a template and no plan is executed; there is no
// starting checkpoint or full replay in this archive. Direct output can
// remain partial if a later archive member fails.
fn write_repro_bundle(
    summary: &kurosaki_core::emulator::RunSummary,
    emu: &Emulator,
    diagnostic_events: &[DiagnosticEvent],
    path: PathBuf,
) -> Result<()> {
    ensure_parent(&path)?;
    let file = fs::File::create(&path)?;
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zip_json(&mut zip, options, "run_summary.json", summary)?;
    zip_json(&mut zip, options, "snapshot.json", &emu.snapshot())?;
    zip_json(
        &mut zip,
        options,
        "ai_diagnostics.json",
        &summary.diagnostics,
    )?;
    zip_json(
        &mut zip,
        options,
        "diagnostic_summary.json",
        &serde_json::json!({
            "schema": "kurosaki-diagnostic-summary",
            "schema_version": 1,
            "rom_sha256": &summary.rom_sha256,
            "diagnostic_events": diagnostic_events.len(),
            "diagnostics": summary.diagnostics.items.len(),
            "errors": summary.diagnostics.errors,
            "warnings": summary.diagnostics.warnings,
            "infos": summary.diagnostics.infos,
        }),
    )?;
    zip_json(
        &mut zip,
        options,
        "retest_plan.json",
        &serde_json::json!({
            "schema": "kurosaki-retest-plan",
            "schema_version": 1,
            "command": "kurosaki run <rom> --emit-diagnostics diagnostics.jsonl --repro-bundle repro_bundle.zip",
            "frames": summary.frames,
        }),
    )?;
    zip_json(
        &mut zip,
        options,
        "repair_plan.json",
        &serde_json::json!({
            "schema": "kurosaki-repair-plan",
            "schema_version": 1,
            "source": "KUROSAKI runtime diagnostics",
            "diagnostics": &summary.diagnostics.items,
        }),
    )?;
    zip_json(
        &mut zip,
        options,
        "automation_plan.json",
        &serde_json::json!({
            "schema": "kurosaki-automation-plan",
            "schema_version": 1,
            "capabilities": {
                "png": true,
                "snapshot": true,
                "trace_jsonl": true,
                "emit_diagnostics": true,
                "repro_bundle": true,
                "break_on_diagnostic": true
            }
        }),
    )?;

    let mut events_jsonl = String::new();
    for event in diagnostic_events {
        events_jsonl.push_str(&serde_json::to_string(event)?);
        events_jsonl.push('\n');
    }
    zip_text(&mut zip, options, "diagnostics.jsonl", &events_jsonl)?;
    zip_text(&mut zip, options, "trace.jsonl", &emu.trace.to_jsonl()?)?;

    zip_json(
        &mut zip,
        options,
        "manifest.json",
        &serde_json::json!({
            "schema": "sarakura-repro-bundle-manifest",
            "schema_version": 1,
            "producer": "kurosaki",
            "platform": "fc",
            "rom": {
                "hash": &summary.rom_sha256,
                "target": "nes"
            },
            "build_id": &summary.rom_sha256,
            "diagnostics_total": diagnostic_events.len(),
            "diagnostics": "ai_diagnostics.json",
            "diagnostic_summary": "diagnostic_summary.json",
            "retest_plan": "retest_plan.json",
            "repair_plan": "repair_plan.json",
            "automation_plan": "automation_plan.json",
            "snapshot": "snapshot.json",
            "trace": "trace.jsonl",
            "diagnostic_events": "diagnostics.jsonl"
        }),
    )?;
    zip.finish()?;
    Ok(())
}

// Start a ZIP member and write indented JSON without a trailing newline.
fn zip_json<T: Serialize>(
    zip: &mut ZipWriter<fs::File>,
    options: FileOptions,
    name: &str,
    value: &T,
) -> Result<()> {
    zip.start_file(name, options)?;
    zip.write_all(serde_json::to_string_pretty(value)?.as_bytes())?;
    Ok(())
}

// Start a ZIP member and write the provided UTF-8 text unchanged.
fn zip_text(
    zip: &mut ZipWriter<fs::File>,
    options: FileOptions,
    name: &str,
    text: &str,
) -> Result<()> {
    zip.start_file(name, options)?;
    zip.write_all(text.as_bytes())?;
    Ok(())
}

// Create missing parent directories, ignoring the empty parent of a bare filename.
fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

// Exclusively create an output after ensuring its parent. On a write error
// close and remove this newly created file best-effort; existing files are
// never opened for replacement. No explicit durability sync is performed.
fn write_create_new_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure_parent(path)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("refusing to overwrite output {}", path.display()))?;
    if let Err(error) = file.write_all(bytes) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error.into());
    }
    Ok(())
}

// Serialize indented JSON and replace the requested file after creating
// parents, or print it with a newline when no path is supplied.
fn write_json_or_stdout<T: Serialize>(value: &T, path: Option<PathBuf>) -> Result<()> {
    let text = serde_json::to_string_pretty(value)?;
    if let Some(path) = path {
        ensure_parent(&path)?;
        fs::write(path, text)?;
    } else {
        println!("{text}");
    }
    Ok(())
}

// List every registry ID or only Implemented/Scaffold entries. Emit JSON
// or console rows plus optional Markdown; only note-column pipes receive
// explicit table escaping.
fn mapper_list(all: bool, json: Option<PathBuf>, md: Option<PathBuf>) -> Result<()> {
    let specs = if all {
        all_mapper_specs()
    } else {
        implemented_mapper_specs()
    };
    if let Some(path) = json {
        fs::write(path, serde_json::to_string_pretty(&specs)?)?;
    } else {
        for s in &specs {
            println!(
                "{:4}  {:24}  {:?}  {:?}",
                s.mapper, s.name, s.family, s.support
            );
        }
    }
    if let Some(path) = md {
        let mut out = String::new();
        out.push_str("# KUROSAKI Mapper Support Registry\n\n");
        out.push_str("| Mapper | Name | Family | Support | IRQ | Expansion Audio | Notes |\n");
        out.push_str("|---:|---|---|---|---:|---:|---|\n");
        for s in &specs {
            out.push_str(&format!(
                "| {} | {} | {:?} | {:?} | {} | {} | {} |\n",
                s.mapper,
                s.name,
                s.family,
                s.support,
                s.has_irq,
                s.has_expansion_audio,
                s.notes.replace('|', "\\|")
            ));
        }
        fs::write(path, out)?;
    }
    Ok(())
}

// Emit the registry descriptor for the requested mapper number without loading a ROM.
fn mapper_info(mapper: u16, json: Option<PathBuf>) -> Result<()> {
    let spec = mapper_spec(mapper);
    write_json_or_stdout(&spec, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Construct the minimal CPU-only category selection for trace-window tests.
    fn cpu_trace() -> TraceConfig {
        TraceConfig {
            cpu: true,
            ..TraceConfig::none()
        }
    }

    #[test]
    // Reject missing, equal and reversed trace bounds through the validation helper.
    fn replay_trace_window_requires_explicit_non_empty_bounds() {
        assert!(validate_replay_trace_window(20, 0, None, Some(10), cpu_trace()).is_err());
        assert!(validate_replay_trace_window(20, 0, Some(4), None, cpu_trace()).is_err());
        assert!(validate_replay_trace_window(20, 0, Some(4), Some(4), cpu_trace()).is_err());
        assert!(validate_replay_trace_window(20, 0, Some(8), Some(4), cpu_trace()).is_err());
    }

    #[test]
    // Reject windows beyond the target or before restored state and an empty
    // category selection, then check two valid absolute intervals.
    fn replay_trace_window_stays_inside_run_and_requires_a_category() {
        assert!(validate_replay_trace_window(20, 0, Some(4), Some(21), cpu_trace()).is_err());
        assert!(
            validate_replay_trace_window(20, 0, Some(4), Some(10), TraceConfig::none()).is_err()
        );
        assert_eq!(
            validate_replay_trace_window(20, 0, Some(4), Some(10), cpu_trace()).unwrap(),
            (4, 10)
        );
        assert!(validate_replay_trace_window(20, 6, Some(4), Some(10), cpu_trace()).is_err());
        assert_eq!(
            validate_replay_trace_window(20, 6, Some(6), Some(10), cpu_trace()).unwrap(),
            (6, 10)
        );
    }

    #[test]
    // Check Clap rejects replay trace bounds/categories without --trace-jsonl.
    fn replay_trace_cli_rejects_trace_flags_without_output() {
        let parsed = Cli::try_parse_from([
            "kurosaki",
            "replay-run",
            "fixture.nes",
            "fixture.replay.json",
            "--trace-start-frame",
            "4",
            "--trace-end-frame-exclusive",
            "8",
            "--cpu",
        ]);
        assert!(parsed.is_err());
    }

    #[test]
    // Check argument parsing accepts explicit bounds with memory-write/NMI
    // selectors. No replay or trace file is opened by this test.
    fn replay_trace_cli_accepts_bounded_category_selection() {
        let parsed = Cli::try_parse_from([
            "kurosaki",
            "replay-run",
            "fixture.nes",
            "fixture.replay.json",
            "--frames",
            "20",
            "--trace-jsonl",
            "trace.jsonl",
            "--trace-start-frame",
            "4",
            "--trace-end-frame-exclusive",
            "8",
            "--mem-write",
            "--nmi",
        ]);
        assert!(parsed.is_ok());
    }

    #[test]
    // Check the replay snapshot-restoration option parses with a target frame.
    fn replay_trace_cli_accepts_snapshot_restore() {
        let parsed = Cli::try_parse_from([
            "kurosaki",
            "replay-run",
            "fixture.nes",
            "fixture.replay.json",
            "--load-snapshot",
            "fixture.kss.json",
            "--frames",
            "20",
        ]);
        assert!(parsed.is_ok());
    }

    #[test]
    // Check the optional CPU-reset switch parses for snapshot continuation.
    fn snapshot_resume_cli_accepts_reset_at_start() {
        let parsed = Cli::try_parse_from([
            "kurosaki",
            "snapshot-resume",
            "fixture.nes",
            "fixture.kss.json",
            "--reset-at-start",
            "--frames",
            "60",
        ]);
        assert!(parsed.is_ok());
    }

    #[test]
    // Reject a missing contract and accept a fully specified rebase with
    // CPU/PRG patches. Both cases supply --out; output omission is not exercised.
    fn snapshot_rebase_cli_requires_explicit_contract_and_output() {
        let missing_contract = Cli::try_parse_from([
            "kurosaki",
            "snapshot-rebase",
            "source.nes",
            "target.nes",
            "source.kss.json",
            "--out",
            "target.kss.json",
        ]);
        assert!(missing_contract.is_err());

        let parsed = Cli::try_parse_from([
            "kurosaki",
            "snapshot-rebase",
            "source.nes",
            "target.nes",
            "source.kss.json",
            "--contract",
            "contract.json",
            "--prg-ram-patch",
            "0x7080=runtime.bin",
            "--cpu-ram-patch",
            "0x009A=0046",
            "--out",
            "target.kss.json",
            "--json",
            "report.json",
        ]);
        assert!(parsed.is_ok());
    }

    #[test]
    // Require argument parsing to reject a PRG-RAM patch starting at $8000.
    fn snapshot_rebase_cli_rejects_patch_outside_prg_ram() {
        let parsed = Cli::try_parse_from([
            "kurosaki",
            "snapshot-rebase",
            "source.nes",
            "target.nes",
            "source.kss.json",
            "--contract",
            "contract.json",
            "--prg-ram-patch",
            "0x8000=runtime.bin",
            "--out",
            "target.kss.json",
        ]);
        assert!(parsed.is_err());
    }

    #[test]
    // Reject a two-byte CPU patch that starts at $07FF and crosses the physical range.
    fn snapshot_rebase_cli_rejects_cpu_ram_patch_outside_physical_ram() {
        let parsed = Cli::try_parse_from([
            "kurosaki",
            "snapshot-rebase",
            "source.nes",
            "target.nes",
            "source.kss.json",
            "--contract",
            "contract.json",
            "--cpu-ram-patch",
            "0x07FF=0046",
            "--out",
            "target.kss.json",
        ]);
        assert!(parsed.is_err());
    }

    #[test]
    // Accept an exact-bank selector with a start address and reject its
    // combination with snapshot-based mapper context.
    fn disasm_cli_accepts_physical_bank_and_rejects_snapshot_mix() {
        let physical = Cli::try_parse_from([
            "kurosaki",
            "disasm",
            "fixture.nes",
            "--prg-bank-8k",
            "28",
            "--start",
            "0x8800",
        ]);
        assert!(physical.is_ok());

        let conflicting = Cli::try_parse_from([
            "kurosaki",
            "disasm",
            "fixture.nes",
            "--prg-bank-8k",
            "28",
            "--snapshot",
            "fixture.kss.json",
        ]);
        assert!(conflicting.is_err());
    }
}
