use crate::bus::Bus;
use crate::cart::Cartridge;
use crate::cpu::CpuState;
use crate::diagnostics::DiagnosticReport;
use crate::error::{KurosakiError, Result};
use crate::hash::sha256_hex;
use crate::hash::sha256_hex_chunks;
use crate::kitaqfc::KitaqfcDebugBundle;
use crate::mapper::create_mapper;
use crate::mapper::MapperDebugState;
use crate::replay::ReplayFrame;
use crate::snapshot::Snapshot;
use crate::trace::{TraceConfig, TraceEvent, TraceSink};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const NTSC_CPU_CYCLES_PER_FRAME: u64 = 29_780;
const MAX_NTSC_CPU_CYCLES_PER_FRAME: u64 = NTSC_CPU_CYCLES_PER_FRAME + 120;

#[derive(Debug, Clone, Serialize, Deserialize)]
// One run request. max_instructions is an absolute cumulative limit;
// replay intervals use the current software frame count. The debug bundle
// is carried by the API but run_current does not resolve source locations.
pub struct RunOptions {
    pub frames: u64,
    pub max_instructions: Option<u64>,
    pub trace: TraceConfig,
    pub allow_unimplemented_opcode: bool,
    pub kitaqfc_debug: Option<KitaqfcDebugBundle>,
    pub pad1: u8,
    pub pad2: u8,
    pub replay_frames: Vec<ReplayFrame>,
}

impl Default for RunOptions {
    // Default to one frame with tracing off, no replay inputs and strict handling
    // of unimplemented opcodes.
    fn default() -> Self {
        Self {
            frames: 1,
            max_instructions: None,
            trace: TraceConfig::none(),
            allow_unimplemented_opcode: false,
            kitaqfc_debug: None,
            pad1: 0,
            pad2: 0,
            replay_frames: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// Snapshot of stored PPU registers, selected memory summaries and cumulative
// counters. A nonzero count or stable hash alone does not establish visual accuracy.
pub struct PpuObservation {
    pub ctrl: u8,
    pub mask: u8,
    pub status: u8,
    pub vram_addr: u16,
    pub nametable0_sha256: String,
    pub nametable0_nonzero: usize,
    pub nametable0_unique_tiles: usize,
    pub palette_sha256: String,
    pub oam_sha256: String,
    pub visible_sprites: usize,
    pub data_writes: u64,
    pub data_writes_outside_vblank: u64,
    pub ctrl_writes_while_rendering: u64,
    pub mask_writes_while_rendering: u64,
    pub scroll_writes_while_rendering: u64,
    pub addr_writes_while_rendering: u64,
    pub data_writes_while_rendering: u64,
    pub addr_writes: u64,
    pub scroll_writes: u64,
    pub rendered_frames: u64,
    pub rendered_scanlines: u64,
    pub last_pixel_hash: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// End-state report with cumulative counters and a per-call stop reason.
// An instruction-limit stop can have a reason while stopped remains false,
// because that field reflects the CPU halt state.
pub struct RunSummary {
    pub format: String,
    pub rom_sha256: String,
    pub frames: u64,
    pub cpu_cycles: u64,
    pub instructions: u64,
    pub stopped: bool,
    pub stop_reason: Option<String>,
    pub final_pc: u16,
    pub final_state_hash: String,
    pub mapper: MapperDebugState,
    pub ppu: PpuObservation,
    pub pc_hotspots: Vec<PcHotspot>,
    pub diagnostics: DiagnosticReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// Execution-step count grouped by CPU address, combining all physical banks
// that were mapped there during the retained counting interval.
pub struct PcHotspot {
    pub pc: u16,
    pub count: u64,
}

// Cartridge, core devices, execution counters and unbounded trace storage.
// Construction, reset, snapshot restore and continuation have distinct effects.
pub struct Emulator {
    pub cartridge: Cartridge,
    pub cpu: CpuState,
    pub bus: Bus,
    pub frame: u64,
    pub instructions: u64,
    pub trace: TraceSink,
    pub pc_counts: BTreeMap<u16, u64>,
}

// Serialize PCM samples explicitly as little-endian bytes so observation hashes
// do not depend on the host's native byte order.
fn i16_samples_as_le_bytes(samples: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

impl Emulator {
    // Create the cartridge mapper and bus, preload any trainer through mapper RAM,
    // and initialize execution counters. CPU reset is a separate operation.
    pub fn from_cartridge(cartridge: Cartridge) -> Result<Self> {
        let mut mapper = create_mapper(&cartridge)?;
        if let Some(trainer) = &cartridge.trainer {
            // iNES trainers are copied into CPU $7000-$71FF before reset.  Keep
            // the preload on the normal mapper path so mapper-owned PRG-RAM is
            // also captured by snapshots and state hashes.
            let mut sink = TraceSink::default();
            for (offset, value) in trainer.iter().copied().enumerate() {
                mapper.write_prg(
                    0x7000 + offset as u16,
                    value,
                    0,
                    0,
                    TraceConfig::none(),
                    &mut sink,
                );
            }
        }
        Ok(Self {
            cartridge,
            cpu: CpuState::default(),
            bus: Bus::new(mapper),
            frame: 0,
            instructions: 0,
            trace: TraceSink::default(),
            pc_counts: BTreeMap::new(),
        })
    }

    // Build a fresh emulator for the cartridge, then validate and restore its checkpoint.
    pub fn from_snapshot(cartridge: Cartridge, snapshot: &Snapshot) -> Result<Self> {
        let mut emulator = Self::from_cartridge(cartridge)?;
        emulator.restore_snapshot(snapshot)?;
        Ok(emulator)
    }

    // Restore a versioned checkpoint only when it belongs to the loaded ROM.
    pub fn restore_snapshot(&mut self, snapshot: &Snapshot) -> Result<()> {
        self.restore_snapshot_inner(snapshot, false)
    }

    // Use the dedicated rebase restore path, which permits a changed ROM fingerprint
    // while retaining format, mapper and private-state integrity checks.
    pub(crate) fn restore_snapshot_for_rebase(&mut self, snapshot: &Snapshot) -> Result<()> {
        self.restore_snapshot_inner(snapshot, true)
    }

    // Validate checkpoint identity and mapper payload before restoring mapper/bus
    // state and execution counters. Successful restoration discards transient trace
    // and hotspot history; errors from individual restoration stages propagate.
    fn restore_snapshot_inner(&mut self, snapshot: &Snapshot, rebase: bool) -> Result<()> {
        if snapshot.format != "kurosaki-snapshot-v2" {
            return Err(KurosakiError::SnapshotFormat(format!(
                "{} is inspect-only; resumable snapshots require kurosaki-snapshot-v2",
                snapshot.format
            )));
        }
        if !rebase && snapshot.rom_sha256 != self.cartridge.info.sha256 {
            return Err(KurosakiError::SnapshotFormat(
                "snapshot ROM fingerprint does not match the loaded ROM".to_string(),
            ));
        }
        if snapshot.mapper.mapper != self.bus.mapper.mapper_id() {
            return Err(KurosakiError::SnapshotFormat(format!(
                "mapper mismatch: snapshot {}, loaded ROM {}",
                snapshot.mapper.mapper,
                self.bus.mapper.mapper_id()
            )));
        }
        if snapshot.mapper_private.is_empty() {
            return Err(KurosakiError::SnapshotFormat(
                "snapshot has no restorable mapper-private state".to_string(),
            ));
        }
        let actual_mapper_hash = sha256_hex(&snapshot.mapper_private);
        if actual_mapper_hash != snapshot.mapper_private_sha256 {
            return Err(KurosakiError::SnapshotFormat(
                "mapper-private state hash mismatch".to_string(),
            ));
        }

        if rebase {
            self.bus
                .mapper
                .restore_rebased_snapshot_bytes(&snapshot.mapper_private)?;
        } else {
            self.bus
                .mapper
                .restore_snapshot_bytes(&snapshot.mapper_private)?;
        }
        // Mapper state has already been restored. If bus validation fails here,
        // the existing emulator can retain a partially restored mapper; callers
        // requiring isolation should construct through from_snapshot.
        self.bus.restore_snapshot(&snapshot.bus)?;
        self.cpu = snapshot.cpu;
        self.frame = snapshot.frame;
        self.instructions = snapshot.instructions;
        self.trace = TraceSink::default();
        self.pc_counts.clear();
        Ok(())
    }

    // Delegate a snapshot RAM patch to the mapper, which owns PRG-RAM layout validation.
    pub(crate) fn patch_snapshot_prg_ram(&mut self, cpu_address: u16, bytes: &[u8]) -> Result<()> {
        self.bus.mapper.patch_prg_ram_bytes(cpu_address, bytes)
    }

    // Patch only physical CPU RAM, not its mirrored address windows. Validate the
    // complete range before copying, including checked end-address arithmetic.
    pub(crate) fn patch_snapshot_cpu_ram(&mut self, cpu_address: u16, bytes: &[u8]) -> Result<()> {
        let start = usize::from(cpu_address);
        let end = start.checked_add(bytes.len()).ok_or_else(|| {
            KurosakiError::SnapshotFormat("CPU-RAM patch range overflow".to_string())
        })?;
        let destination = self.bus.ram.get_mut(start..end).ok_or_else(|| {
            KurosakiError::SnapshotFormat(
                "CPU-RAM patch must remain inside physical $0000-$07FF".to_string(),
            )
        })?;
        destination.copy_from_slice(bytes);
        Ok(())
    }

    // Reset the CPU through the bus, optionally install the FDS fast-boot entry,
    // and clear hotspot counts. This is not construction of an entirely new machine.
    pub fn reset(&mut self, trace_cfg: TraceConfig) {
        self.trace
            .push(TraceEvent::new("emu.reset", self.frame, self.cpu.cycles));
        self.cpu.reset(&mut self.bus, trace_cfg, &mut self.trace);
        if let Some(pc) = self.cartridge.fds_fast_boot_pc {
            self.cpu.pc = pc;
            let mut event = TraceEvent::new("fds.fast_boot", self.frame, self.cpu.cycles);
            event.pc = Some(pc);
            event.message = Some(format!(
                "FDS boot files preloaded; starting at ${pc:04X} from $DFFC"
            ));
            self.trace.push(event);
        }
        self.pc_counts.clear();
    }

    // Count the current PC, execute one CPU step, then advance attached devices by
    // the consumed cycles. Allowing an unimplemented opcode converts it to a traced
    // stop; it does not invent instruction behavior or continue past the opcode.
    pub fn step_instruction(
        &mut self,
        trace_cfg: TraceConfig,
        allow_unimplemented: bool,
    ) -> Result<()> {
        *self.pc_counts.entry(self.cpu.pc).or_insert(0) += 1;
        match self
            .cpu
            .step(&mut self.bus, self.frame, trace_cfg, &mut self.trace)
        {
            Ok(cpu_cycles) => {
                self.instructions += 1;
                self.bus.clock_devices(
                    cpu_cycles as u64,
                    self.frame,
                    self.cpu.cycles,
                    trace_cfg,
                    &mut self.trace,
                );
                Ok(())
            }
            Err(KurosakiError::UnimplementedOpcode { opcode, pc }) if allow_unimplemented => {
                let mut event =
                    TraceEvent::new("cpu.unimplemented_opcode", self.frame, self.cpu.cycles);
                event.pc = Some(pc);
                event.prg_bank = self.bus.mapper.physical_prg_bank_8k(pc);
                event.opcode = Some(opcode);
                event.severity = Some("warn".to_string());
                event.message = Some("Execution stopped because this opcode is non-official or not implemented in this clean-room CPU core".to_string());
                self.trace.push(event);
                self.cpu.stopped = true;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    // Execute until the PPU completes a frame, the CPU stops, or the bounded cycle
    // budget is reached. The software frame counter advances even after an early break.
    pub fn step_frame(&mut self, trace_cfg: TraceConfig, allow_unimplemented: bool) -> Result<()> {
        // The cycle budget is checked between instructions. Reaching the budget
        // or finding a stopped CPU still proceeds to the software frame increment;
        // a propagated instruction error exits before that increment.
        let frame_start = self.cpu.cycles;
        let ppu_frame_start = self.bus.ppu.rendered_frames;
        while self.bus.ppu.rendered_frames == ppu_frame_start {
            if self.cpu.stopped {
                break;
            }
            if self.cpu.cycles.saturating_sub(frame_start) >= MAX_NTSC_CPU_CYCLES_PER_FRAME {
                break;
            }
            self.step_instruction(trace_cfg, allow_unimplemented)?;
        }
        self.frame += 1;
        Ok(())
    }

    // Reset before executing the requested run. Use run_current to continue restored or existing state.
    pub fn run(&mut self, options: RunOptions) -> RunSummary {
        self.reset(options.trace);
        self.run_current(options)
    }

    // Continue from the current state with frame-based inputs and collect a run summary.
    // Instruction limits are checked between frames, so a frame can overshoot the limit.
    // Generated diagnostic candidates describe observations rather than confirmed bugs.
    pub fn run_current(&mut self, options: RunOptions) -> RunSummary {
        // This call does not clear trace, device counters or hotspots. Even a
        // zero-frame request produces diagnostics from the existing state.
        let mut stop_reason = None;
        for _ in 0..options.frames {
            let mut pad1 = options.pad1;
            let mut pad2 = options.pad2;
            // Search replay entries from the end: the last covering entry wins,
            // regardless of chronological ordering. Only buttons and reset are consumed
            // here; disk-side and expected-frame-hash fields are not applied.
            if let Some(input) = options
                .replay_frames
                .iter()
                .rev()
                .find(|input| input.covers(self.frame))
            {
                pad1 = input.pad1;
                pad2 = input.pad2;
                // Reset is an edge, not a held input repeated for duration frames.
                if input.reset && input.frame == self.frame {
                    self.reset(options.trace);
                }
            }
            self.bus.set_controller_state(pad1, pad2);
            // Input and any replay reset are applied before this absolute instruction
            // limit is checked. The limit does not constrain an individual frame step.
            if let Some(max) = options.max_instructions {
                if self.instructions >= max {
                    stop_reason = Some(format!("max instruction limit {max} reached"));
                    break;
                }
            }
            if let Err(e) = self.step_frame(options.trace, options.allow_unimplemented_opcode) {
                stop_reason = Some(e.to_string());
                self.cpu.stopped = true;
                break;
            }
            if self.cpu.stopped {
                stop_reason = Some("CPU stopped".to_string());
                break;
            }
        }
        // Combine cartridge compatibility notes with accumulated runtime observations;
        // heuristic PPU warnings do not establish the cause of a rendering defect.
        let mut diagnostics = DiagnosticReport::from_rom_info(&self.cartridge.info);
        if self.bus.ppu.data_writes_outside_vblank > 0 {
            diagnostics.push(crate::diagnostics::Diagnostic {
                code: "KS-PPU-0002".to_string(),
                severity: crate::diagnostics::Severity::Warn,
                title: "PPUDATA writes outside VBlank candidate".to_string(),
                message: format!("{} PPUDATA writes occurred during rendering scanlines with rendering enabled.", self.bus.ppu.data_writes_outside_vblank),
                recommendation: Some("Move large VRAM transfers into NMI/VBlank, or split them through a KITAQFC VRAM queue.".to_string()),
                frame: Some(self.frame), pc: Some(self.cpu.pc), function: None, source: None,
            });
        }
        let rendering_register_writes = self.bus.ppu.ctrl_writes_while_rendering
            + self.bus.ppu.mask_writes_while_rendering
            + self.bus.ppu.scroll_writes_while_rendering
            + self.bus.ppu.addr_writes_while_rendering
            + self.bus.ppu.data_writes_while_rendering;
        if rendering_register_writes > 0 {
            diagnostics.push(crate::diagnostics::Diagnostic {
                code: "KS-PPU-0006".to_string(),
                severity: crate::diagnostics::Severity::Warn,
                title: "PPU register writes during visible rendering candidate".to_string(),
                message: format!(
                    "{} rendering-sensitive PPU register writes occurred while background/sprite rendering was active on visible scanlines (ctrl={}, mask={}, scroll={}, addr={}, data={}).",
                    rendering_register_writes,
                    self.bus.ppu.ctrl_writes_while_rendering,
                    self.bus.ppu.mask_writes_while_rendering,
                    self.bus.ppu.scroll_writes_while_rendering,
                    self.bus.ppu.addr_writes_while_rendering,
                    self.bus.ppu.data_writes_while_rendering
                ),
                recommendation: Some("Reduce per-frame VRAM/OAM-side work, split queued VRAM writes across frames, or perform bulk updates with rendering disabled.".to_string()),
                frame: Some(self.frame),
                pc: Some(self.cpu.pc),
                function: None,
                source: None,
            });
        }
        // Lifetime write parity is a diagnostic heuristic; status reads can reset
        // the actual shared latch, so parity alone does not prove a broken sequence.
        if !self.bus.ppu.addr_writes.is_multiple_of(2) {
            diagnostics.push(crate::diagnostics::Diagnostic {
                code: "KS-PPU-0003".to_string(),
                severity: crate::diagnostics::Severity::Warn,
                title: "Odd PPUADDR write count".to_string(),
                message: format!("PPUADDR was written {} times; an odd count can indicate a broken address latch sequence.", self.bus.ppu.addr_writes),
                recommendation: Some("Check $2006 high/low write pairs and ensure PPUSTATUS is read intentionally when resetting the latch.".to_string()),
                frame: Some(self.frame), pc: Some(self.cpu.pc), function: None, source: None,
            });
        }
        if !self.bus.ppu.scroll_writes.is_multiple_of(2) {
            diagnostics.push(crate::diagnostics::Diagnostic {
                code: "KS-PPU-0004".to_string(),
                severity: crate::diagnostics::Severity::Warn,
                title: "Odd PPUSCROLL write count".to_string(),
                message: format!("PPUSCROLL was written {} times; an odd count can leave the scroll latch half-written.", self.bus.ppu.scroll_writes),
                recommendation: Some("Write PPUSCROLL in X/Y pairs, or reset the latch with PPUSTATUS before the next pair.".to_string()),
                frame: Some(self.frame), pc: Some(self.cpu.pc), function: None, source: None,
            });
        }
        let ppu_observation = self.ppu_observation();
        // These thresholds inspect the complete stored 1 KiB nametable slice,
        // including attributes. They do not identify the source of copied bytes.
        let dense_high_entropy_nt = ppu_observation.nametable0_nonzero > 890
            && ppu_observation.nametable0_unique_tiles > 80;
        let sparse_code_like_nt = ppu_observation.nametable0_nonzero > 700
            && ppu_observation.nametable0_unique_tiles > 96;
        if dense_high_entropy_nt || sparse_code_like_nt {
            diagnostics.push(crate::diagnostics::Diagnostic {
                code: "KS-PPU-0200".to_string(),
                severity: crate::diagnostics::Severity::Warn,
                title: "High-entropy nametable candidate".to_string(),
                message: format!(
                    "Nametable 0 has {} non-zero bytes and {} unique tile IDs. This often indicates that code or unrelated PRG data was copied into VRAM instead of a prepared nametable.",
                    ppu_observation.nametable0_nonzero,
                    ppu_observation.nametable0_unique_tiles
                ),
                recommendation: Some("Check banked PRG-ROM pointers passed into VRAM copy routines; the routine must read data from the caller's selected bank or from fixed/common data.".to_string()),
                frame: Some(self.frame), pc: Some(self.cpu.pc), function: None, source: None,
            });
        }
        if self.bus.ppu.mask & 0x18 != 0 && self.bus.ppu.vblank_count == 0 {
            diagnostics.push(crate::diagnostics::Diagnostic {
                code: "KS-PPU-0005".to_string(),
                severity: crate::diagnostics::Severity::Info,
                title: "Rendering enabled before observed VBlank".to_string(),
                message: "PPUMASK has background/sprite rendering enabled before any modeled VBlank event.".to_string(),
                recommendation: Some("For startup-sensitive tests, add a power-on/vblank warmup fixture.".to_string()),
                frame: Some(self.frame), pc: Some(self.cpu.pc), function: None, source: None,
            });
        }
        if self.bus.ppu.rendered_scanlines > 0 {
            diagnostics.push(crate::diagnostics::Diagnostic {
                code: "KS-PPU-0100".to_string(),
                severity: crate::diagnostics::Severity::Info,
                title: "Pixel-oriented PPU pipeline active".to_string(),
                message: format!("Rendered {} visible scanlines, {} pattern fetches, last pixel hash {:016X}.", self.bus.ppu.rendered_scanlines, self.bus.ppu.pattern_fetches, self.bus.ppu.last_pixel_hash),
                recommendation: Some("Use this hash for deterministic golden tests; do not treat it as pixel-perfect validation until fixture comparisons are added.".to_string()),
                frame: Some(self.frame), pc: Some(self.cpu.pc), function: None, source: None,
            });
        }
        if self.bus.ppu.sprite_overflow_candidates > 0 {
            diagnostics.push(crate::diagnostics::Diagnostic {
                code: "KS-PPU-0101".to_string(),
                severity: crate::diagnostics::Severity::Warn,
                title: "Sprite overflow candidate observed".to_string(),
                message: format!("{} scanlines had more than 8 sprite candidates.", self.bus.ppu.sprite_overflow_candidates),
                recommendation: Some("Check OAM ordering and sprite multiplexing. KUROSAKI currently reports conservative candidates.".to_string()),
                frame: Some(self.frame), pc: Some(self.cpu.pc), function: None, source: None,
            });
        }
        if self.bus.apu.generated_samples > 0 {
            diagnostics.push(crate::diagnostics::Diagnostic {
                code: "KS-APU-0100".to_string(),
                severity: crate::diagnostics::Severity::Info,
                title: "APU sample generation active".to_string(),
                message: format!("Generated {} audio samples at {} Hz; recent buffer has {} samples; DMC fetches={} DMC bits={} DMC silence_ticks={} DMC IRQ events={} frame IRQ events={}", self.bus.apu.generated_samples, self.bus.apu.sample_rate, self.bus.apu.sample_buffer.len(), self.bus.apu.dmc_sample_fetches, self.bus.apu.dmc_bits_output, self.bus.apu.dmc_silence_ticks, self.bus.apu.dmc_irq_events, self.bus.apu.frame_irq_events),
                recommendation: Some("The current mixer uses CPU-cycle-clocked 2A03 channel sequencers, a NES-like non-linear pulse/TND approximation, DMC sample playback, and mapper expansion audio; waveform-accurate fixture comparison remains a later task.".to_string()),
                frame: Some(self.frame), pc: Some(self.cpu.pc), function: None, source: None,
            });
        }
        if self.bus.apu.dmc_joypad_conflicts > 0 {
            diagnostics.push(crate::diagnostics::Diagnostic {
                code: "KS-APU-0201".to_string(),
                severity: crate::diagnostics::Severity::Warn,
                title: "DMC DMA / joypad read conflict candidate".to_string(),
                message: format!("{} joypad reads occurred while DMC was enabled.", self.bus.apu.dmc_joypad_conflicts),
                recommendation: Some("Use a DMC-safe pad read routine or disable DMC around controller reads; validate on hardware later.".to_string()),
                frame: Some(self.frame), pc: Some(self.cpu.pc), function: None, source: None,
            });
        }
        // The shared stall counter also includes DMC fetches. This legacy OAM
        // diagnostic label does not distinguish the sources of all counted cycles.
        if self.bus.dma_stall_cycles > 0 {
            diagnostics.push(crate::diagnostics::Diagnostic {
                code: "KS-DMA-0001".to_string(),
                severity: crate::diagnostics::Severity::Info,
                title: "OAM DMA observed".to_string(),
                message: format!(
                    "OAM DMA accounted for approximately {} CPU stall cycles.",
                    self.bus.dma_stall_cycles
                ),
                recommendation: None,
                frame: Some(self.frame),
                pc: Some(self.cpu.pc),
                function: None,
                source: None,
            });
        }
        // Only an instruction-limit stop with a sufficiently frequent top PC
        // produces the loop candidate; intentional waits can satisfy the same rule.
        let pc_hotspots = self.pc_hotspots(8);
        if let Some(top_pc) = pc_hotspots.first() {
            let stopped_by_limit = stop_reason
                .as_ref()
                .map(|reason| reason.starts_with("max instruction limit"))
                .unwrap_or(false);
            if stopped_by_limit && top_pc.count > 1_000 {
                diagnostics.push(crate::diagnostics::Diagnostic {
                    code: "KS-CPU-0200".to_string(),
                    severity: crate::diagnostics::Severity::Warn,
                    title: "PC hotspot near instruction limit".to_string(),
                    message: format!(
                        "PC ${:04X} executed {} times before the run stopped. Current PRG windows: {:?}.",
                        top_pc.pc,
                        top_pc.count,
                        self.bus.mapper.debug_state().prg_bank_window
                    ),
                    recommendation: Some("Use a CPU+mapper trace around this PC to distinguish an intentional vblank wait from an unintended bank/loop stall.".to_string()),
                    frame: Some(self.frame),
                    pc: Some(top_pc.pc),
                    function: None,
                    source: None,
                });
            }
        }
        RunSummary {
            format: "kurosaki-run-summary-v1".to_string(),
            rom_sha256: self.cartridge.info.sha256.clone(),
            frames: self.frame,
            cpu_cycles: self.cpu.cycles,
            instructions: self.instructions,
            stopped: self.cpu.stopped,
            stop_reason,
            final_pc: self.cpu.pc,
            final_state_hash: self.state_hash(),
            mapper: self.bus.mapper.debug_state(),
            ppu: ppu_observation,
            pc_hotspots,
            diagnostics,
        }
    }

    // Sort sampled instruction addresses by descending execution count, breaking ties
    // by address for deterministic output. Counts are keyed by CPU address, not ROM bank.
    pub fn pc_hotspots(&self, limit: usize) -> Vec<PcHotspot> {
        let mut hotspots = self
            .pc_counts
            .iter()
            .map(|(pc, count)| PcHotspot {
                pc: *pc,
                count: *count,
            })
            .collect::<Vec<_>>();
        hotspots.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.pc.cmp(&b.pc)));
        hotspots.truncate(limit);
        hotspots
    }

    // Summarize stored nametable bytes, palette/OAM hashes and PPU counters. The
    // nametable slice includes attribute bytes, and the sprite count is a Y-range
    // heuristic rather than proof that every counted sprite produced visible pixels.
    pub fn ppu_observation(&self) -> PpuObservation {
        let nametable0 = &self.bus.ppu.vram[0x2000..0x2400];
        let unique = nametable0.iter().copied().collect::<BTreeSet<_>>().len();
        let nonzero = nametable0.iter().filter(|value| **value != 0).count();
        let visible_sprites = self
            .bus
            .ppu
            .oam
            .chunks_exact(4)
            .filter(|sprite| sprite[0] < 0xEF)
            .count();
        PpuObservation {
            ctrl: self.bus.ppu.ctrl,
            mask: self.bus.ppu.mask,
            status: self.bus.ppu.status,
            vram_addr: self.bus.ppu.vram_addr,
            nametable0_sha256: sha256_hex(nametable0),
            nametable0_nonzero: nonzero,
            nametable0_unique_tiles: unique,
            palette_sha256: sha256_hex(&self.bus.ppu.palette),
            oam_sha256: sha256_hex(&self.bus.ppu.oam),
            visible_sprites,
            data_writes: self.bus.ppu.data_writes,
            data_writes_outside_vblank: self.bus.ppu.data_writes_outside_vblank,
            ctrl_writes_while_rendering: self.bus.ppu.ctrl_writes_while_rendering,
            mask_writes_while_rendering: self.bus.ppu.mask_writes_while_rendering,
            scroll_writes_while_rendering: self.bus.ppu.scroll_writes_while_rendering,
            addr_writes_while_rendering: self.bus.ppu.addr_writes_while_rendering,
            data_writes_while_rendering: self.bus.ppu.data_writes_while_rendering,
            addr_writes: self.bus.ppu.addr_writes,
            scroll_writes: self.bus.ppu.scroll_writes,
            rendered_frames: self.bus.ppu.rendered_frames,
            rendered_scanlines: self.bus.ppu.rendered_scanlines,
            last_pixel_hash: self.bus.ppu.last_pixel_hash,
        }
    }

    // Hash selected memory, rendering, audio-buffer and mapper state for comparisons.
    // CPU registers and all timing fields are not included: this is an observation
    // fingerprint, not a complete serialized-checkpoint identity.
    pub fn state_hash(&self) -> String {
        let mapper_bytes = self.bus.mapper.snapshot_bytes();
        let audio_bytes = i16_samples_as_le_bytes(&self.bus.apu.sample_buffer);
        sha256_hex_chunks([
            &self.bus.ram[..],
            &self.bus.ppu.palette[..],
            &self.bus.ppu.oam[..],
            &self.bus.ppu.vram[..],
            &self.bus.ppu.frame_buffer[..],
            &self.bus.apu.regs[..],
            audio_bytes.as_slice(),
            &mapper_bytes[..],
        ])
    }

    // Capture CPU/bus/mapper state and execution counters in snapshot-v2 format,
    // including a hash of the mapper-private payload for restoration checks.
    pub fn snapshot(&self) -> Snapshot {
        let mapper_private = self.bus.mapper.snapshot_bytes();
        Snapshot {
            format: "kurosaki-snapshot-v2".to_string(),
            emulator_version: env!("CARGO_PKG_VERSION").to_string(),
            rom_sha256: self.cartridge.info.sha256.clone(),
            frame: self.frame,
            instructions: self.instructions,
            cpu: self.cpu,
            bus: self.bus.to_snapshot(),
            mapper: self.bus.mapper.debug_state(),
            mapper_private: mapper_private.clone(),
            mapper_private_sha256: crate::hash::sha256_hex(&mapper_private),
            // Record how many events existed, but do not embed their payloads.
            // Normal restoration starts with an empty trace and hotspot map.
            last_event_count: self.trace.events.len(),
        }
    }
}
