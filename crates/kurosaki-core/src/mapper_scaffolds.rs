//! Clean-room mapper scaffolds for complex NES/Famicom boards.
//!
//! These implementations are intentionally conservative. They turn important
//! commercial mapper families from probe-only into board-specific observable
//! scaffolds: bank registers, IRQ registers, expansion-audio register writes,
//! and private state now appear in trace/debug/snapshot output. Fixture-driven
//! edge-case accuracy remains a later phase.

use crate::cart::Mirroring;
use crate::mapper::{
    bank_count, ensure_chr, mapper_debug_state, mirroring_name, physical_bank_8k, read_bank,
    trace_mapper_write, write_bank, Mapper, MapperDebugState,
};
use crate::mapper_db::{mapper_spec, MapperSpec};
use crate::trace::{TraceConfig, TraceEvent, TraceSink};

#[derive(Debug, Clone)]
pub struct Mmc5Mapper {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    exram: Vec<u8>,
    mirroring: Mirroring,
    battery: bool,
    prg_mode: u8,
    chr_mode: u8,
    prg_regs: [u8; 4],
    chr_regs: [u8; 12],
    nametable_mode: u8,
    irq_scanline: u8,
    irq_enabled: bool,
    irq_pending_flag: bool,
    scanline_counter: u8,
    last_a12: bool,
    mul_a: u8,
    mul_b: u8,
    audio_events: u64,
}

impl Mmc5Mapper {
    // Allocate PRG RAM, ExRAM and CHR backing, initialize four PRG slots
    // near the first/final banks, and clear mapper/IRQ/audio observations.
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring, battery: bool) -> Self {
        let (chr, chr_ram) = ensure_chr(chr_rom);
        let prg_count = bank_count(prg_rom.len(), 8 * 1024);
        let last = prg_count.saturating_sub(1) as u8;
        Self {
            prg_rom,
            prg_ram: vec![0; 64 * 1024],
            chr,
            chr_ram,
            exram: vec![0; 1024],
            mirroring,
            battery,
            prg_mode: 3,
            chr_mode: 3,
            prg_regs: [0, 1, last.saturating_sub(1), last],
            chr_regs: [0; 12],
            nametable_mode: 0,
            irq_scanline: 0,
            irq_enabled: false,
            irq_pending_flag: false,
            scanline_counter: 0,
            last_a12: false,
            mul_a: 0,
            mul_b: 0,
            audio_events: 0,
        }
    }

    // Resolve one of four 8 KiB ROM slots according to the modeled PRG mode.
    // All results reference ROM; PRG register bits are not used to select RAM.
    fn prg_bank_for_slot(&self, slot: usize) -> usize {
        let count = bank_count(self.prg_rom.len(), 8 * 1024);
        match self.prg_mode & 3 {
            0 => ((self.prg_regs[3] as usize) & !3).saturating_add(slot) % count,
            1 => {
                if slot < 2 {
                    ((self.prg_regs[1] as usize) & !1).saturating_add(slot) % count
                } else {
                    ((self.prg_regs[3] as usize) & !1).saturating_add(slot - 2) % count
                }
            }
            // Mode two currently maps two independent lower 8 KiB slots and one
            // aligned upper 16 KiB pair; describe this implementation explicitly.
            2 => {
                if slot == 0 {
                    self.prg_regs[1] as usize % count
                } else if slot == 1 {
                    self.prg_regs[2] as usize % count
                } else {
                    ((self.prg_regs[3] as usize) & !1).saturating_add(slot - 2) % count
                }
            }
            _ => self.prg_regs[slot] as usize % count,
        }
    }

    // Use the first eight CHR registers as 1 KiB selectors with bank wrapping.
    // The stored CHR mode and registers 8-11 do not affect this read mapping.
    fn chr_bank_for_addr(&self, addr: u16) -> usize {
        let count = bank_count(self.chr.len(), 1024);
        let slot = ((addr as usize) / 1024) & 7;
        self.chr_regs[slot] as usize % count
    }

    // Describe the nametable-mode register and header mirroring for debug
    // output; this type inherits the runtime four-screen mirroring fallback.
    fn mmc5_mirroring_name(&self) -> String {
        format!(
            "mmc5_nt_mode_{:02X}/{}",
            self.nametable_mode,
            mirroring_name(self.mirroring)
        )
    }
}

impl Mapper for Mmc5Mapper {
    // Identify the MMC5-specific scaffold as mapper 5.
    fn mapper_id(&self) -> u16 {
        5
    }
    // Return a label that identifies the implementation as a scaffold.
    fn mapper_name(&self) -> &'static str {
        "MMC5/ExROM scaffold"
    }

    // Resolve a CPU ROM address through the modeled 8 KiB PRG slots, leaving
    // registers, ExRAM and work RAM without a physical ROM label.
    fn physical_prg_bank_8k(&self, addr: u16) -> Option<u16> {
        if !(0x8000..=0xFFFF).contains(&addr) {
            return None;
        }
        let slot = ((addr - 0x8000) / 0x2000) as usize;
        physical_bank_8k(
            self.prg_rom.len(),
            8 * 1024,
            self.prg_bank_for_slot(slot),
            addr as usize & 0x1FFF,
        )
    }

    // Route multiply-result, ExRAM and work-RAM reads or selected PRG ROM.
    // Reading IRQ status clears the pending flag; audio-register reads return
    // zero. Only the first 8 KiB of allocated work RAM is CPU-mapped here.
    fn read_prg(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u8 {
        let value = match addr {
            0x5000..=0x5015 => 0,
            0x5204 => {
                let v = if self.irq_pending_flag { 0x80 } else { 0x00 };
                self.irq_pending_flag = false;
                v
            }
            0x5205 => self.mul_a.wrapping_mul(self.mul_b),
            0x5206 => ((self.mul_a as u16 * self.mul_b as u16) >> 8) as u8,
            0x5C00..=0x5FFF => self.exram[(addr as usize - 0x5C00) % self.exram.len()],
            0x6000..=0x7FFF => self.prg_ram[(addr as usize - 0x6000) % self.prg_ram.len()],
            0x8000..=0xFFFF => {
                let slot = ((addr - 0x8000) / 0x2000) as usize;
                read_bank(
                    &self.prg_rom,
                    8 * 1024,
                    self.prg_bank_for_slot(slot),
                    addr as usize & 0x1FFF,
                )
            }
            _ => 0,
        };
        if cfg.mapper && (0x5000..=0x5FFF).contains(&addr) {
            let mut event = TraceEvent::new("mapper.mmc5_read", frame, cycle);
            event.addr = Some(addr);
            event.value = Some(value);
            event.message = Some("MMC5 register/ExRAM read scaffold".to_string());
            sink.push(event);
        }
        value
    }

    // Latch bank/mode/IRQ/multiply fields and write ExRAM/work RAM. Audio
    // addresses count and trace writes only; no MMC5 mixer override generates
    // samples. Other selected writes may be traced without changing state.
    fn write_prg(
        &mut self,
        addr: u16,
        value: u8,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        match addr {
            0x5000..=0x5015 => {
                self.audio_events = self.audio_events.saturating_add(1);
                trace_mapper_write(
                    "mapper.expansion_audio_write",
                    addr,
                    value,
                    frame,
                    cycle,
                    cfg,
                    sink,
                    "MMC5 pulse/PCM audio register write observed",
                );
                return;
            }
            0x5100 => self.prg_mode = value & 0x03,
            0x5101 => self.chr_mode = value & 0x03,
            0x5105 => self.nametable_mode = value,
            0x5114..=0x5117 => self.prg_regs[(addr - 0x5114) as usize] = value,
            0x5120..=0x5127 => self.chr_regs[(addr - 0x5120) as usize] = value,
            0x5128..=0x512B => self.chr_regs[8 + (addr - 0x5128) as usize] = value,
            0x5203 => self.irq_scanline = value,
            0x5204 => {
                self.irq_enabled = value & 0x80 != 0;
                if !self.irq_enabled {
                    self.irq_pending_flag = false;
                }
            }
            0x5205 => self.mul_a = value,
            0x5206 => self.mul_b = value,
            0x5C00..=0x5FFF => {
                let idx = (addr as usize - 0x5C00) % self.exram.len();
                self.exram[idx] = value;
            }
            0x6000..=0x7FFF => {
                let idx = (addr as usize - 0x6000) % self.prg_ram.len();
                self.prg_ram[idx] = value;
            }
            _ => {}
        }
        if (0x5000..=0x5FFF).contains(&addr) || (0x8000..=0xFFFF).contains(&addr) {
            trace_mapper_write(
                "mapper.mmc5_write",
                addr,
                value,
                frame,
                cycle,
                cfg,
                sink,
                format!(
                    "MMC5 write; prg_mode={} chr_mode={} PRG={:?} IRQ scanline={} enabled={}",
                    self.prg_mode,
                    self.chr_mode,
                    self.prg_regs,
                    self.irq_scanline,
                    self.irq_enabled
                ),
            );
        }
    }

    // Read the selected wrapped 1 KiB CHR bank.
    fn read_chr(&mut self, addr: u16) -> u8 {
        read_bank(
            &self.chr,
            1024,
            self.chr_bank_for_addr(addr),
            addr as usize & 0x03FF,
        )
    }
    // Write through the same 1 KiB mapping only when CHR RAM is present.
    fn write_chr(&mut self, addr: u16, value: u8) {
        if self.chr_ram {
            let bank = self.chr_bank_for_addr(addr);
            write_bank(&mut self.chr, 1024, bank, addr as usize & 0x03FF, value);
        }
    }
    // Approximate a scanline counter using every observed rising A12 edge.
    // The eight-bit counter wraps and is not reset per frame in this method;
    // a matching enabled target latches IRQ.
    fn notify_ppu_addr(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        let a12 = addr & 0x1000 != 0;
        if a12 && !self.last_a12 {
            self.scanline_counter = self.scanline_counter.wrapping_add(1);
            if self.scanline_counter == self.irq_scanline && self.irq_enabled {
                self.irq_pending_flag = true;
                if cfg.mapper || cfg.nmi {
                    let mut event = TraceEvent::new("mapper.mmc5_irq", frame, cycle);
                    event.message = Some(format!(
                        "MMC5 scanline IRQ scaffold at scanline counter {}",
                        self.scanline_counter
                    ));
                    sink.push(event);
                }
            }
        }
        self.last_a12 = a12;
    }
    // Expose the pending mapper interrupt latch.
    fn irq_pending(&self) -> bool {
        self.irq_pending_flag
    }
    // Clear pending IRQ without resetting the counter or enable flag.
    fn clear_irq(&mut self) {
        self.irq_pending_flag = false;
    }
    // Report effective PRG banks, raw CHR registers and stored mode/counter
    // metadata for the scaffold.
    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            5,
            format!(
                "MMC5 scaffold prg_mode={} chr_mode={} audio_events={}",
                self.prg_mode, self.chr_mode, self.audio_events
            ),
            false,
            (0..4).map(|s| self.prg_bank_for_slot(s) as u16).collect(),
            self.chr_regs.iter().take(8).map(|v| *v as u16).collect(),
            self.mmc5_mirroring_name(),
            self.irq_pending(),
        )
    }
    // Append register/IRQ fields, write count and PRG/ExRAM backing, adding
    // CHR only when writable. This type inherits unsupported restore and
    // battery-RAM export methods despite retaining a battery flag.
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut v = vec![
            self.prg_mode,
            self.chr_mode,
            self.nametable_mode,
            self.irq_scanline,
            self.irq_enabled as u8,
            self.irq_pending_flag as u8,
            self.scanline_counter,
            self.last_a12 as u8,
            self.mul_a,
            self.mul_b,
            self.battery as u8,
        ];
        v.extend_from_slice(&self.audio_events.to_le_bytes());
        v.extend_from_slice(&self.prg_regs);
        v.extend_from_slice(&self.chr_regs);
        v.extend_from_slice(&self.prg_ram);
        v.extend_from_slice(&self.exram);
        if self.chr_ram {
            v.extend_from_slice(&self.chr);
        }
        v
    }
}

const CPU_HZ_NTSC: f64 = 1_789_773.0;

#[derive(Debug, Clone)]
struct Vrc6PulseState {
    control: u8,
    timer: u16,
    enabled: bool,
    phase: f64,
}

impl Default for Vrc6PulseState {
    // Start the pulse disabled with zero timer, volume and fractional phase.
    fn default() -> Self {
        Self {
            control: 0,
            timer: 0,
            enabled: false,
            phase: 0.0,
        }
    }
}

impl Vrc6PulseState {
    // Decode control and 12-bit timer fields through the low two address
    // bits; disabling the pulse also resets its phase.
    fn write(&mut self, reg: u16, value: u8) {
        match reg & 0x03 {
            0 => self.control = value,
            1 => self.timer = (self.timer & 0x0F00) | value as u16,
            2 => {
                self.timer = (self.timer & 0x00FF) | (((value & 0x0F) as u16) << 8);
                self.enabled = value & 0x80 != 0;
                if !self.enabled {
                    self.phase = 0.0;
                }
            }
            _ => {}
        }
    }

    // Gate disabled/very-short-timer/zero-volume output, then advance phase
    // and emit a duty pulse or constant-volume mode. Silent gating freezes
    // phase; callers must provide a nonzero sample rate.
    fn sample(&mut self, sample_rate: u32) -> f64 {
        let volume = (self.control & 0x0F) as f64 / 15.0;
        if !self.enabled || self.timer < 2 || volume <= 0.0 {
            return 0.0;
        }
        let freq = CPU_HZ_NTSC / (16.0 * (self.timer as f64 + 1.0));
        self.phase = (self.phase + freq / sample_rate as f64).fract();
        if self.control & 0x80 != 0 {
            return volume;
        }
        let duty = ((self.control >> 4) & 0x07) as f64 + 1.0;
        let step = self.phase * 16.0;
        if step < duty {
            volume
        } else {
            -volume
        }
    }

    // Append control, timer and enable fields; the live fractional pulse
    // phase is not included in this private representation.
    fn snapshot_bytes(&self, out: &mut Vec<u8>) {
        out.push(self.control);
        out.extend_from_slice(&self.timer.to_le_bytes());
        out.push(self.enabled as u8);
    }
}

#[derive(Debug, Clone)]
struct Vrc6SawState {
    rate: u8,
    timer: u16,
    enabled: bool,
    phase: f64,
}

impl Default for Vrc6SawState {
    // Start the saw disabled with zero rate, timer and fractional phase.
    fn default() -> Self {
        Self {
            rate: 0,
            timer: 0,
            enabled: false,
            phase: 0.0,
        }
    }
}

impl Vrc6SawState {
    // Latch the six-bit rate and 12-bit timer, resetting phase on disable.
    fn write(&mut self, reg: u16, value: u8) {
        match reg & 0x03 {
            0 => self.rate = value & 0x3F,
            1 => self.timer = (self.timer & 0x0F00) | value as u16,
            2 => {
                self.timer = (self.timer & 0x00FF) | (((value & 0x0F) as u16) << 8);
                self.enabled = value & 0x80 != 0;
                if !self.enabled {
                    self.phase = 0.0;
                }
            }
            _ => {}
        }
    }

    // Generate a continuous saw approximation at the modeled timer frequency,
    // scaled by rate. Gated output freezes phase, and sample rate must be nonzero.
    fn sample(&mut self, sample_rate: u32) -> f64 {
        if !self.enabled || self.timer < 2 || self.rate == 0 {
            return 0.0;
        }
        let freq = CPU_HZ_NTSC / (14.0 * (self.timer as f64 + 1.0));
        self.phase = (self.phase + freq / sample_rate as f64).fract();
        let amp = self.rate as f64 / 63.0;
        ((self.phase * 2.0) - 1.0) * amp
    }

    // Append rate, timer and enable state without the fractional saw phase.
    fn snapshot_bytes(&self, out: &mut Vec<u8>) {
        out.push(self.rate);
        out.extend_from_slice(&self.timer.to_le_bytes());
        out.push(self.enabled as u8);
    }
}

#[derive(Debug, Clone, Default)]
struct Vrc6AudioState {
    pulse: [Vrc6PulseState; 2],
    saw: Vrc6SawState,
}

impl Vrc6AudioState {
    // Decode mirrored pulse-one, pulse-two and saw register triplets; return
    // whether the address was consumed so mapper bank logic can stop dispatch.
    fn write(&mut self, addr: u16, value: u8) -> bool {
        match addr & 0xF003 {
            0x9000..=0x9002 => {
                self.pulse[0].write(addr, value);
                true
            }
            0xA000..=0xA002 => {
                self.pulse[1].write(addr, value);
                true
            }
            0xB000..=0xB002 => {
                self.saw.write(addr, value);
                true
            }
            _ => false,
        }
    }

    // Advance two pulses and the saw once, apply fixed mixer gains and clamp
    // the summed contribution to i16.
    fn sample(&mut self, sample_rate: u32) -> i16 {
        let mixed = self.pulse[0].sample(sample_rate) * 1900.0
            + self.pulse[1].sample(sample_rate) * 1900.0
            + self.saw.sample(sample_rate) * 1700.0;
        mixed.clamp(i16::MIN as f64, i16::MAX as f64) as i16
    }

    // Append both pulse records followed by the saw record in a fixed order.
    fn snapshot_bytes(&self, out: &mut Vec<u8>) {
        self.pulse[0].snapshot_bytes(out);
        self.pulse[1].snapshot_bytes(out);
        self.saw.snapshot_bytes(out);
    }
}

const VRC7_CHANNEL_COUNT: usize = 6;
const VRC7_REG_COUNT: usize = 0x40;
// Clamp a floating-point gain to the unit interval.
fn vrc7_clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

// Decode the low-nibble operator frequency multiplier, including the
// half-frequency zero entry and repeated high-nibble values.
fn vrc7_mul_from_nibble(n: u8) -> f64 {
    // OPLL uses a small multiplier table. This clean-room approximation keeps
    // multiplier 0 musically useful instead of making the operator silent.
    match n & 0x0F {
        0 => 0.5,
        1 => 1.0,
        2 => 2.0,
        3 => 3.0,
        4 => 4.0,
        5 => 5.0,
        6 => 6.0,
        7 => 7.0,
        8 => 8.0,
        9 => 9.0,
        10 => 10.0,
        11 => 10.0,
        12 => 12.0,
        13 => 12.0,
        14 => 15.0,
        _ => 15.0,
    }
}

// Map the attack nibble to a positive quadratic per-sample increment;
// these rates are approximation constants rather than chip clock counts.
fn vrc7_attack_rate(n: u8) -> f64 {
    let x = (n & 0x0F) as f64 / 15.0;
    0.000015 + x * x * 0.0065
}

// Map the decay nibble to a positive quadratic per-sample decrement.
fn vrc7_decay_rate(n: u8) -> f64 {
    let x = (n & 0x0F) as f64 / 15.0;
    0.000004 + x * x * 0.0016
}

// Map the release nibble to a positive quadratic per-sample decrement.
fn vrc7_release_rate(n: u8) -> f64 {
    let x = (n & 0x0F) as f64 / 15.0;
    0.000003 + x * x * 0.0012
}

// Map the sustain nibble from full level down to a small positive floor.
fn vrc7_sustain_level(n: u8) -> f64 {
    // 0 is a high sustain level, 15 is close to silence.
    1.0 - ((n & 0x0F) as f64 / 15.0) * 0.92
}

// Evaluate a sine at a cycle-based phase, optionally silencing its negative
// half for the alternate operator waveform.
fn vrc7_wave(phase: f64, half_sine: bool) -> f64 {
    let s = (phase * std::f64::consts::TAU).sin();
    if half_sine && s < 0.0 {
        0.0
    } else {
        s
    }
}

#[derive(Debug, Clone, Copy)]
struct Vrc7SlotPatch {
    mul: f64,
    total_level: f64,
    attack_rate: f64,
    decay_rate: f64,
    sustain_level: f64,
    release_rate: f64,
    half_sine: bool,
    am: bool,
    vib: bool,
    eg_sustain: bool,
    key_scale: bool,
}

impl Vrc7SlotPatch {
    // Decode operator multiplier, gains, envelope rates and modulation flags
    // from patch bytes into the floating-point approximation parameters.
    fn from_regs(flags_mul: u8, total_level: f64, ar_dr: u8, sl_rr: u8, half_sine: bool) -> Self {
        Self {
            mul: vrc7_mul_from_nibble(flags_mul),
            total_level: vrc7_clamp01(total_level),
            attack_rate: vrc7_attack_rate(ar_dr >> 4),
            decay_rate: vrc7_decay_rate(ar_dr & 0x0F),
            sustain_level: vrc7_sustain_level(sl_rr >> 4),
            release_rate: vrc7_release_rate(sl_rr & 0x0F),
            half_sine,
            am: flags_mul & 0x80 != 0,
            vib: flags_mul & 0x40 != 0,
            eg_sustain: flags_mul & 0x20 != 0,
            key_scale: flags_mul & 0x10 != 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Vrc7Patch {
    mod_slot: Vrc7SlotPatch,
    car_slot: Vrc7SlotPatch,
    feedback: f64,
    mod_index: f64,
    brightness: f64,
}

impl Vrc7Patch {
    // Build modulator/carrier slots from an eight-byte patch and derive
    // feedback, modulation depth and brightness using fixed scaling rules.
    fn from_regs(regs: &[u8; 8]) -> Self {
        let mod_total = 1.0 - (((regs[2] & 0x3F) as f64 / 63.0) * 0.92);
        let mod_slot =
            Vrc7SlotPatch::from_regs(regs[0], mod_total, regs[4], regs[6], regs[3] & 0x08 != 0);
        let car_slot =
            Vrc7SlotPatch::from_regs(regs[1], 1.0, regs[5], regs[7], regs[3] & 0x10 != 0);
        let feedback = ((regs[3] & 0x07) as f64 / 7.0) * 4.2;
        let mod_index = 0.35 + mod_slot.total_level * 5.4;
        let brightness = 0.72 + car_slot.mul.min(8.0) * 0.035;
        Self {
            mod_slot,
            car_slot,
            feedback,
            mod_index,
            brightness,
        }
    }
}

// Clean-room synthetic patch set. These are deliberately not copied from YM2413
// or VRC7 instrument ROM tables; they only provide stable, varied timbres for
// debugging and WAV export until a verified chip-compatible table is added.
const VRC7_PRESET_BYTES: [[u8; 8]; 16] = [
    [0x01, 0x01, 0x30, 0x00, 0xD4, 0xF4, 0x64, 0x44],
    [0x01, 0x01, 0x24, 0x01, 0xE4, 0xF3, 0x65, 0x43],
    [0x13, 0x01, 0x18, 0x03, 0xF3, 0xE4, 0x54, 0x44],
    [0x21, 0x11, 0x20, 0x05, 0xD5, 0xF5, 0x43, 0x55],
    [0x41, 0x02, 0x16, 0x02, 0xE2, 0xF4, 0x65, 0x42],
    [0x03, 0x12, 0x28, 0x04, 0xD3, 0xE5, 0x54, 0x53],
    [0x15, 0x01, 0x10, 0x07, 0xF4, 0xD5, 0x33, 0x64],
    [0x01, 0x23, 0x12, 0x02, 0xE5, 0xF6, 0x44, 0x45],
    [0x17, 0x01, 0x08, 0x06, 0xF6, 0xE6, 0x22, 0x65],
    [0x01, 0x01, 0x38, 0x00, 0xC2, 0xF2, 0x76, 0x32],
    [0x23, 0x31, 0x12, 0x05, 0xF4, 0xE4, 0x34, 0x53],
    [0x32, 0x12, 0x14, 0x06, 0xE5, 0xF5, 0x43, 0x64],
    [0x46, 0x01, 0x0C, 0x07, 0xF7, 0xD6, 0x22, 0x75],
    [0x01, 0x42, 0x22, 0x01, 0xD4, 0xF4, 0x65, 0x43],
    [0x25, 0x22, 0x08, 0x07, 0xF6, 0xD7, 0x21, 0x76],
    [0x02, 0x01, 0x34, 0x01, 0xD3, 0xF3, 0x76, 0x32],
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vrc7EgPhase {
    Off,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Debug, Clone, Copy)]
struct Vrc7SlotState {
    phase: f64,
    env: f64,
    eg_phase: Vrc7EgPhase,
    last_output: f64,
}

impl Default for Vrc7SlotState {
    // Start an operator off with zero envelope, phase and previous output.
    fn default() -> Self {
        Self {
            phase: 0.0,
            env: 0.0,
            eg_phase: Vrc7EgPhase::Off,
            last_output: 0.0,
        }
    }
}

impl Vrc7SlotState {
    // Restart phase and envelope at zero in Attack and clear the old output.
    fn note_on(&mut self) {
        self.phase = 0.0;
        self.env = 0.0;
        self.eg_phase = Vrc7EgPhase::Attack;
        self.last_output = 0.0;
    }

    // Enter Release for an active operator while leaving an already-off
    // operator unchanged.
    fn note_off(&mut self) {
        if self.eg_phase != Vrc7EgPhase::Off {
            self.eg_phase = Vrc7EgPhase::Release;
        }
    }

    // Advance Attack/Decay/Sustain/Release by one sample using optional block
    // scaling, then apply total level and amplitude modulation. Rates are
    // per call, so wall-clock envelope duration depends on the sample rate.
    fn clock_envelope(
        &mut self,
        patch: Vrc7SlotPatch,
        block: u8,
        sustain_mode: bool,
        lfo_am: f64,
    ) -> f64 {
        let key_scale = if patch.key_scale {
            1.0 + block as f64 * 0.055
        } else {
            1.0
        };
        match self.eg_phase {
            Vrc7EgPhase::Off => {
                self.env = 0.0;
            }
            Vrc7EgPhase::Attack => {
                self.env = (self.env + patch.attack_rate * key_scale).min(1.0);
                if self.env >= 0.999 {
                    self.env = 1.0;
                    self.eg_phase = Vrc7EgPhase::Decay;
                }
            }
            Vrc7EgPhase::Decay => {
                self.env = (self.env - patch.decay_rate * key_scale).max(patch.sustain_level);
                if self.env <= patch.sustain_level + 0.000001 {
                    self.eg_phase = Vrc7EgPhase::Sustain;
                }
            }
            Vrc7EgPhase::Sustain => {
                if !patch.eg_sustain && !sustain_mode {
                    self.env = (self.env - patch.release_rate * 0.18 * key_scale).max(0.0);
                    if self.env <= 0.0001 {
                        self.eg_phase = Vrc7EgPhase::Off;
                    }
                }
            }
            Vrc7EgPhase::Release => {
                let sustain_factor = if sustain_mode { 0.38 } else { 1.0 };
                self.env = (self.env - patch.release_rate * sustain_factor * key_scale).max(0.0);
                if self.env <= 0.0001 {
                    self.env = 0.0;
                    self.eg_phase = Vrc7EgPhase::Off;
                }
            }
        }
        let am_gain = if patch.am {
            1.0 - vrc7_clamp01((lfo_am + 1.0) * 0.5) * 0.18
        } else {
            1.0
        };
        self.env * patch.total_level * am_gain
    }

    // Advance one operator cycle fraction using frequency multiplier and
    // optional vibrato; the caller supplies a nonzero sample rate.
    fn advance_phase(&mut self, hz: f64, patch: Vrc7SlotPatch, sample_rate: u32, lfo_vib: f64) {
        let vib = if patch.vib {
            1.0 + lfo_vib * 0.012
        } else {
            1.0
        };
        self.phase = (self.phase + (hz * patch.mul * vib) / sample_rate as f64).fract();
    }
}

#[derive(Debug, Clone, Copy)]
struct Vrc7ChannelState {
    freq_low: u8,
    freq_high: u8,
    instrument: u8,
    volume: u8,
    key_on: bool,
    sustain_mode: bool,
    mod_slot: Vrc7SlotState,
    car_slot: Vrc7SlotState,
    feedback_1: f64,
    feedback_2: f64,
}

impl Default for Vrc7ChannelState {
    // Initialize an unkeyed channel with silent operators, instrument zero,
    // maximum attenuation and no feedback history.
    fn default() -> Self {
        Self {
            freq_low: 0,
            freq_high: 0,
            instrument: 0,
            volume: 15,
            key_on: false,
            sustain_mode: false,
            mod_slot: Vrc7SlotState::default(),
            car_slot: Vrc7SlotState::default(),
            feedback_1: 0.0,
            feedback_2: 0.0,
        }
    }
}

impl Vrc7ChannelState {
    // Combine the low register and one high bit into the nine-bit frequency.
    fn fnum(&self) -> u16 {
        self.freq_low as u16 | (((self.freq_high & 0x01) as u16) << 8)
    }

    // Extract the three-bit octave/block field from the high register.
    fn block(&self) -> u8 {
        (self.freq_high >> 1) & 0x07
    }

    // Convert frequency and block fields into the model's base frequency;
    // zero frequency returns silence without operator advancement.
    fn note_hz(&self) -> f64 {
        let fnum = self.fnum() as f64;
        if fnum <= 0.0 {
            return 0.0;
        }
        // Clean-room OPLL-style approximation: the exact chip uses an internal
        // phase generator. This keeps VRC7 music in the right audible range while
        // staying deterministic and compact for debugger WAV output.
        let block_scale = 2.0_f64.powi(self.block() as i32);
        (fnum * 49_716.0 * block_scale) / 524_288.0
    }

    // Replace the low eight frequency bits without retriggering envelopes.
    fn write_low(&mut self, value: u8) {
        self.freq_low = value;
    }

    // Latch high frequency, key and sustain fields. Key-on edges restart
    // both operators and feedback; key-off edges enter release, while repeated
    // writes with the same key state do not retrigger.
    fn write_high(&mut self, value: u8) {
        let old_key = self.key_on;
        self.freq_high = value;
        self.key_on = value & 0x10 != 0;
        self.sustain_mode = value & 0x20 != 0;
        if self.key_on && !old_key {
            self.mod_slot.note_on();
            self.car_slot.note_on();
            self.feedback_1 = 0.0;
            self.feedback_2 = 0.0;
        } else if !self.key_on && old_key {
            self.mod_slot.note_off();
            self.car_slot.note_off();
        }
    }

    // Split the register into a four-bit instrument index and attenuation.
    fn write_instrument_volume(&mut self, value: u8) {
        self.instrument = (value >> 4) & 0x0F;
        self.volume = value & 0x0F;
    }

    // Keep keyed, releasing or residual-envelope channels active until
    // both operators are off with zero gain.
    fn is_active(&self) -> bool {
        self.key_on
            || self.mod_slot.eg_phase != Vrc7EgPhase::Off
            || self.car_slot.eg_phase != Vrc7EgPhase::Off
            || self.mod_slot.env > 0.0
            || self.car_slot.env > 0.0
    }

    // Advance envelopes and a feedback modulator/carrier pair, then apply
    // channel attenuation and patch brightness. Zero frequency freezes the
    // envelopes too; a silent carrier skips phase/feedback advancement.
    fn sample(&mut self, patch: Vrc7Patch, sample_rate: u32, lfo_am: f64, lfo_vib: f64) -> f64 {
        if !self.is_active() {
            return 0.0;
        }

        let hz = self.note_hz();
        if hz <= 0.0 {
            return 0.0;
        }

        let block = self.block();
        let mod_env =
            self.mod_slot
                .clock_envelope(patch.mod_slot, block, self.sustain_mode, lfo_am);
        let car_env =
            self.car_slot
                .clock_envelope(patch.car_slot, block, self.sustain_mode, lfo_am);
        if car_env <= 0.0001 {
            return 0.0;
        }

        self.mod_slot
            .advance_phase(hz, patch.mod_slot, sample_rate, lfo_vib);
        let feedback = (self.feedback_1 + self.feedback_2) * 0.5 * patch.feedback;
        let modulator = vrc7_wave(
            self.mod_slot.phase + feedback * 0.035,
            patch.mod_slot.half_sine,
        );
        self.feedback_2 = self.feedback_1;
        self.feedback_1 = modulator;
        self.mod_slot.last_output = modulator;

        let phase_mod = modulator * patch.mod_index * mod_env;
        self.car_slot
            .advance_phase(hz, patch.car_slot, sample_rate, lfo_vib);
        let carrier = vrc7_wave(
            self.car_slot.phase + phase_mod * 0.08,
            patch.car_slot.half_sine,
        );
        self.car_slot.last_output = carrier;

        // Treat each volume step as two decibels of attenuation; the maximum
        // register value attenuates rather than mathematically muting the channel.
        let volume_gain = 10.0_f64.powf(-(self.volume as f64 * 2.0) / 20.0);
        carrier * car_env * volume_gain * patch.brightness
    }

    // Append frequency/key/patch fields, envelopes, envelope phases and
    // feedback samples. Operator oscillator phases and last outputs are omitted.
    fn snapshot_bytes(&self, out: &mut Vec<u8>) {
        out.push(self.freq_low);
        out.push(self.freq_high);
        out.push(self.instrument);
        out.push(self.volume);
        out.push(self.key_on as u8);
        out.push(self.sustain_mode as u8);
        out.extend_from_slice(&self.mod_slot.env.to_le_bytes());
        out.extend_from_slice(&self.car_slot.env.to_le_bytes());
        out.push(self.mod_slot.eg_phase as u8);
        out.push(self.car_slot.eg_phase as u8);
        out.extend_from_slice(&self.feedback_1.to_le_bytes());
        out.extend_from_slice(&self.feedback_2.to_le_bytes());
    }
}

#[derive(Debug, Clone)]
struct Vrc7AudioState {
    selected_reg: u8,
    regs: [u8; VRC7_REG_COUNT],
    user_patch: [u8; 8],
    channels: [Vrc7ChannelState; VRC7_CHANNEL_COUNT],
    writes: u64,
    am_lfo_phase: f64,
    vib_lfo_phase: f64,
}

impl Default for Vrc7AudioState {
    // Clear register shadows, user patch, six channels, write count and LFO
    // phases; instrument zero will use the programmable user patch.
    fn default() -> Self {
        Self {
            selected_reg: 0,
            regs: [0; VRC7_REG_COUNT],
            user_patch: [0; 8],
            channels: [Vrc7ChannelState::default(); VRC7_CHANNEL_COUNT],
            writes: 0,
            am_lfo_phase: 0.0,
            vib_lfo_phase: 0.0,
        }
    }
}

impl Vrc7AudioState {
    // Decode mirrored address/data ports, mask the selected register to six
    // bits and count each recognized port write.
    fn write(&mut self, addr: u16, value: u8) -> bool {
        match addr & 0xF030 {
            0x9010 => {
                self.selected_reg = value & 0x3F;
                self.writes = self.writes.saturating_add(1);
                true
            }
            0x9030 => {
                self.write_selected(value);
                self.writes = self.writes.saturating_add(1);
                true
            }
            _ => false,
        }
    }

    // Always retain the register shadow, then update user-patch or channel
    // fields for modeled register ranges; other registers have no audio effect.
    fn write_selected(&mut self, value: u8) {
        let reg = self.selected_reg as usize & (VRC7_REG_COUNT - 1);
        self.regs[reg] = value;
        match reg {
            0x00..=0x07 => self.user_patch[reg] = value,
            0x10..=0x15 => self.channels[reg - 0x10].write_low(value),
            0x20..=0x25 => self.channels[reg - 0x20].write_high(value),
            0x30..=0x35 => self.channels[reg - 0x30].write_instrument_volume(value),
            _ => {}
        }
    }

    // Advance shared AM/vibrato LFOs and mix six channels using the user
    // patch for instrument zero or the synthetic preset table otherwise.
    // Apply fixed output scaling and i16 clipping.
    fn sample(&mut self, sample_rate: u32) -> i16 {
        self.am_lfo_phase = (self.am_lfo_phase + 6.1 / sample_rate as f64).fract();
        self.vib_lfo_phase = (self.vib_lfo_phase + 6.4 / sample_rate as f64).fract();
        let lfo_am = (self.am_lfo_phase * std::f64::consts::TAU).sin();
        let lfo_vib = (self.vib_lfo_phase * std::f64::consts::TAU).sin();

        let mut mixed = 0.0;
        let user_patch = self.user_patch;
        for ch in &mut self.channels {
            let patch_bytes = if ch.instrument == 0 {
                user_patch
            } else {
                VRC7_PRESET_BYTES[ch.instrument as usize & 0x0F]
            };
            let patch = Vrc7Patch::from_regs(&patch_bytes);
            mixed += ch.sample(patch, sample_rate, lfo_am, lfo_vib) * 1450.0;
        }
        mixed.clamp(i16::MIN as f64, i16::MAX as f64) as i16
    }

    // Append register/user-patch shadows, write count, both LFO phases and
    // channel records; channel oscillator phases are not serialized.
    fn snapshot_bytes(&self, out: &mut Vec<u8>) {
        out.push(self.selected_reg);
        out.extend_from_slice(&self.regs);
        out.extend_from_slice(&self.user_patch);
        out.extend_from_slice(&self.writes.to_le_bytes());
        out.extend_from_slice(&self.am_lfo_phase.to_le_bytes());
        out.extend_from_slice(&self.vib_lfo_phase.to_le_bytes());
        for ch in &self.channels {
            ch.snapshot_bytes(out);
        }
    }

    // Count keyed or releasing/residual-envelope channels for diagnostics.
    fn active_channel_count(&self) -> usize {
        self.channels.iter().filter(|ch| ch.is_active()).count()
    }

    #[cfg(test)]
    // Decode the current programmable patch for field-level test assertions.
    fn user_patch_summary(&self) -> Vrc7Patch {
        Vrc7Patch::from_regs(&self.user_patch)
    }
}

#[derive(Debug, Clone)]
pub struct VrcFamilyMapper {
    mapper: u16,
    submapper: u8,
    prg_mode: bool,
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    mirroring: Mirroring,
    prg_regs: [u8; 4],
    chr_regs: [u8; 8],
    irq_latch: u16,
    irq_counter: u16,
    irq_control: u8,
    irq_prescaler: u16,
    irq_enabled: bool,
    irq_pending_flag: bool,
    audio_regs: [u8; 0x40],
    audio_writes: u64,
    vrc6_audio: Vrc6AudioState,
    vrc7_audio: Vrc7AudioState,
}

impl VrcFamilyMapper {
    // Construct the requested VRC mapper using compatibility submapper zero.
    pub fn new(mapper: u16, prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        Self::new_with_submapper(mapper, 0, prg_rom, chr_rom, mirroring)
    }
    // Retain mapper/submapper decoding context, allocate work RAM and CHR,
    // initialize first/final PRG banks and clear IRQ plus both audio models.
    pub fn new_with_submapper(
        mapper: u16,
        submapper: u8,
        prg_rom: Vec<u8>,
        chr_rom: Vec<u8>,
        mirroring: Mirroring,
    ) -> Self {
        let (chr, chr_ram) = ensure_chr(chr_rom);
        let count = bank_count(prg_rom.len(), 8 * 1024);
        Self {
            mapper,
            submapper,
            prg_mode: false,
            prg_rom,
            prg_ram: vec![0; 8 * 1024],
            chr,
            chr_ram,
            mirroring,
            prg_regs: [
                0,
                1,
                count.saturating_sub(2) as u8,
                count.saturating_sub(1) as u8,
            ],
            chr_regs: [0; 8],
            irq_latch: 0,
            irq_counter: 0,
            irq_control: 0,
            irq_prescaler: 0,
            irq_enabled: false,
            irq_pending_flag: false,
            audio_regs: [0; 0x40],
            audio_writes: 0,
            vrc6_audio: Vrc6AudioState::default(),
            vrc7_audio: Vrc7AudioState::default(),
        }
    }
    // Select the mapper IDs using the VRC4-style PRG mode and IRQ decoder.
    fn is_vrc4_like(&self) -> bool {
        matches!(self.mapper, 21 | 23 | 25 | 27)
    }
    // Suppress VRC IRQ behavior for mapper 22 or any configured submapper
    // 3/4; this predicate does not further restrict those submapper cases.
    fn is_vrc2(&self) -> bool {
        self.mapper == 22 || matches!(self.submapper, 3 | 4)
    }
    // Extract two board-dependent CPU address lines into a logical register
    // selector, using explicit submapper cases before compatibility defaults.
    fn vrc_register_select(&self, addr: u16) -> u8 {
        let (a0, a1) = match (self.mapper, self.submapper) {
            (22, _) | (25, 1 | 3) => (1, 0),
            (21, 1) => (1, 2),
            (21, 2) => (6, 7),
            (23, 2) => (2, 3),
            (25, 2) => (3, 2),
            // iNES mapper 21/23/25 defaults are compatibility mappings; the
            // explicit NES 2.0 submapper paths above are board-accurate.
            (21, _) => (1, 2),
            (23, _) => (0, 1),
            (25, _) => (1, 0),
            _ => (0, 1),
        };
        (((addr >> a0) & 1) | (((addr >> a1) & 1) << 1)) as u8
    }
    // Identify mapper IDs with the implemented VRC6/VRC7 audio paths.
    fn has_exp_audio(&self) -> bool {
        matches!(self.mapper, 24 | 26 | 85)
    }
    // Select the shared VRC6 path for mapper IDs 24 and 26.
    fn is_vrc6(&self) -> bool {
        matches!(self.mapper, 24 | 26)
    }
    // Select the VRC7 path for mapper 85.
    fn is_vrc7(&self) -> bool {
        self.mapper == 85
    }
    // Offer the original CPU address to the VRC6 decoder, then count and
    // shadow recognized writes. This wrapper applies no extra address-bit swap.
    fn write_vrc6_audio(&mut self, addr: u16, value: u8) -> bool {
        if self.is_vrc6() && self.vrc6_audio.write(addr, value) {
            self.audio_writes = self.audio_writes.saturating_add(1);
            self.audio_regs[(addr as usize) & 0x3F] = value;
            true
        } else {
            false
        }
    }
    // Offer the address to the VRC7 port decoder and retain a write count
    // and low-six-address-bit shadow for recognized accesses.
    fn write_vrc7_audio(&mut self, addr: u16, value: u8) -> bool {
        if self.is_vrc7() && self.vrc7_audio.write(addr, value) {
            self.audio_writes = self.audio_writes.saturating_add(1);
            self.audio_regs[(addr as usize) & 0x3F] = value;
            true
        } else {
            false
        }
    }
    // Return a family-specific scaffold label without changing dispatch.
    fn name_static(&self) -> &'static str {
        match self.mapper {
            21 => "VRC4a/VRC4c scaffold",
            22 => "VRC2a scaffold",
            23 => "VRC2b/VRC4e scaffold",
            24 => "VRC6a scaffold",
            25 => "VRC4b/VRC4d scaffold",
            26 => "VRC6b scaffold",
            73 => "VRC3 scaffold",
            75 => "VRC1 scaffold",
            85 => "VRC7 scaffold",
            _ => "VRC family scaffold",
        }
    }
    // Build four wrapped 8 KiB PRG selections. VRC6/7 expose three writable
    // slots plus the final bank; VRC4 mode exchanges the lower selectable
    // slot with the second-last fixed bank.
    fn prg_windows(&self) -> [usize; 4] {
        let count = bank_count(self.prg_rom.len(), 8 * 1024);
        if self.is_vrc6() || self.is_vrc7() {
            [
                self.prg_regs[0] as usize % count,
                self.prg_regs[1] as usize % count,
                self.prg_regs[2] as usize % count,
                count.saturating_sub(1),
            ]
        } else if self.is_vrc4_like() && self.prg_mode {
            [
                count.saturating_sub(2),
                self.prg_regs[1] as usize % count,
                self.prg_regs[0] as usize % count,
                count.saturating_sub(1),
            ]
        } else {
            [
                self.prg_regs[0] as usize % count,
                self.prg_regs[1] as usize % count,
                count.saturating_sub(2),
                count.saturating_sub(1),
            ]
        }
    }
    // For non-VRC2 paths, latch low/high IRQ bytes by address region and
    // handle control/reload in the $D000 region when dispatch reaches it.
    fn write_vrc_irq(&mut self, addr: u16, value: u8) {
        if self.is_vrc2() {
            return;
        }
        match addr & 0xF000 {
            0xF000 => self.irq_latch = (self.irq_latch & 0xFF00) | value as u16,
            0xE000 => self.irq_latch = (self.irq_latch & 0x00FF) | ((value as u16) << 8),
            0xD000 => {
                self.irq_control = value;
                self.irq_enabled = value & 0x02 != 0;
                self.irq_prescaler = 341;
                if self.irq_enabled {
                    self.irq_counter = self.irq_latch;
                } else {
                    self.irq_pending_flag = false;
                }
            }
            _ => {}
        }
    }

    // Use the decoded selector for two latch nibbles, control/reload and
    // acknowledgement. Control bit zero supplies the post-acknowledge enable.
    fn write_vrc4_irq(&mut self, addr: u16, value: u8) {
        if self.is_vrc2() {
            return;
        }
        match self.vrc_register_select(addr) {
            0 => self.irq_latch = (self.irq_latch & 0x00F0) | (value as u16 & 0x0F),
            1 => self.irq_latch = (self.irq_latch & 0x000F) | ((value as u16 & 0x0F) << 4),
            2 => {
                self.irq_control = value & 0x07;
                self.irq_enabled = value & 0x02 != 0;
                self.irq_pending_flag = false;
                self.irq_prescaler = 0;
                if self.irq_enabled {
                    self.irq_counter = self.irq_latch & 0x00FF;
                }
            }
            3 => {
                self.irq_pending_flag = false;
                self.irq_enabled = self.irq_control & 0x01 != 0;
            }
            _ => unreachable!(),
        }
    }
}

impl Mapper for VrcFamilyMapper {
    // Return the configured mapper ID for cartridge diagnostics.
    fn mapper_id(&self) -> u16 {
        self.mapper
    }
    // Use the family-specific scaffold name.
    fn mapper_name(&self) -> &'static str {
        self.name_static()
    }
    // Resolve one of the four modeled CPU ROM slots into a physical 8 KiB
    // bank; work RAM and registers have no cartridge-ROM bank label.
    fn physical_prg_bank_8k(&self, addr: u16) -> Option<u16> {
        if !(0x8000..=0xFFFF).contains(&addr) {
            return None;
        }
        let slot = ((addr - 0x8000) / 0x2000) as usize;
        physical_bank_8k(
            self.prg_rom.len(),
            8 * 1024,
            self.prg_windows()[slot],
            addr as usize & 0x1FFF,
        )
    }
    // Read the flat 8 KiB work-RAM window or a selected PRG-ROM slot, with
    // zero returned for other addresses.
    fn read_prg(
        &mut self,
        addr: u16,
        _frame: u64,
        _cycle: u64,
        _cfg: TraceConfig,
        _sink: &mut TraceSink,
    ) -> u8 {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr as usize - 0x6000) % self.prg_ram.len()],
            0x8000..=0x9FFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[0],
                addr as usize - 0x8000,
            ),
            0xA000..=0xBFFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[1],
                addr as usize - 0xA000,
            ),
            0xC000..=0xDFFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[2],
                addr as usize - 0xC000,
            ),
            0xE000..=0xFFFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[3],
                addr as usize - 0xE000,
            ),
            _ => 0,
        }
    }
    // Give recognized audio ports priority, then dispatch work RAM, family
    // PRG/CHR selectors, mirroring fields and IRQ registers. Branch ordering
    // is significant where board register regions overlap.
    fn write_prg(
        &mut self,
        addr: u16,
        value: u8,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        let mut expansion_audio_write = false;
        // An accepted sound-register write must not also change a PRG or CHR
        // bank in the overlapping CPU address range.
        if self.write_vrc6_audio(addr, value) || self.write_vrc7_audio(addr, value) {
            expansion_audio_write = true;
        } else {
            match addr {
                0x6000..=0x7FFF => {
                    let idx = (addr as usize - 0x6000) % self.prg_ram.len();
                    self.prg_ram[idx] = value;
                }
                0x8000..=0x8FFF if self.is_vrc7() && (addr & 0xF030) == 0x8010 => {
                    self.prg_regs[1] = value;
                }
                0x8000..=0x8FFF => self.prg_regs[0] = value,
                0x9000..=0x9FFF if self.is_vrc7() => {
                    self.prg_regs[2] = value;
                }
                0x9000..=0x9FFF => {
                    if self.has_exp_audio() {
                        self.audio_writes = self.audio_writes.saturating_add(1);
                        self.audio_regs[(addr as usize) & 0x3F] = value;
                    }
                    match self.vrc_register_select(addr) {
                        0 => match value & 0x03 {
                            0 => self.mirroring = Mirroring::Vertical,
                            1 => self.mirroring = Mirroring::Horizontal,
                            _ if !self.is_vrc2() => self.mirroring = Mirroring::MapperControlled,
                            _ => {}
                        },
                        2 if !self.is_vrc2() => self.prg_mode = value & 0x02 != 0,
                        _ => {}
                    }
                }
                0xA000..=0xAFFF if self.is_vrc7() => {
                    self.chr_regs[if addr & 0x0010 != 0 { 1 } else { 0 }] = value;
                }
                0xA000..=0xAFFF => self.prg_regs[1] = value,
                0xB000..=0xBFFF if self.is_vrc7() => {
                    self.chr_regs[if addr & 0x0010 != 0 { 3 } else { 2 }] = value;
                }
                0xC000..=0xCFFF if self.is_vrc6() => {
                    self.prg_regs[2] = value;
                }
                0xC000..=0xCFFF if self.is_vrc7() => {
                    self.chr_regs[if addr & 0x0010 != 0 { 5 } else { 4 }] = value;
                }
                // VRC7 uses this region for CHR selectors here, so it does not reach
                // the generic IRQ helper's $D000 control case.
                0xD000..=0xDFFF if self.is_vrc7() => {
                    self.chr_regs[if addr & 0x0010 != 0 { 7 } else { 6 }] = value;
                }
                0xE000..=0xFFFF if self.is_vrc7() => self.write_vrc_irq(addr, value),
                0xB000..=0xEFFF => {
                    if self.is_vrc4_like() || matches!(self.mapper, 22 | 23 | 24 | 26) {
                        if matches!(self.mapper, 21 | 22 | 23 | 25) {
                            let group = ((addr - 0xB000) >> 12) as usize;
                            let select = self.vrc_register_select(addr) as usize;
                            let slot = group * 2 + (select >> 1);
                            let old = self.chr_regs[slot];
                            self.chr_regs[slot] = if select & 1 == 0 {
                                (old & 0xF0) | (value & 0x0F)
                            } else {
                                (old & 0x0F) | ((value & 0x0F) << 4)
                            };
                        } else {
                            let slot = (((addr - 0xB000) / 0x0800) as usize).min(7);
                            self.chr_regs[slot] = value;
                        }
                    } else if self.mapper == 73 {
                        self.write_vrc_irq(addr, value);
                    }
                }
                0xF000..=0xFFFF if self.is_vrc4_like() => self.write_vrc4_irq(addr, value),
                0xF000..=0xFFFF => self.write_vrc_irq(addr, value),
                _ => {}
            }
        }
        // The trace category also labels any $9000-region write on an audio
        // variant as expansion audio, even when it changed a bank instead.
        let kind = if expansion_audio_write
            || (self.has_exp_audio() && (0x9000..=0x9FFF).contains(&addr))
        {
            "mapper.expansion_audio_write"
        } else {
            "mapper.vrc_write"
        };
        trace_mapper_write(
            kind,
            addr,
            value,
            frame,
            cycle,
            cfg,
            sink,
            format!(
                "{} write; PRG {:?}; IRQ latch={} enabled={} audio_writes={}",
                self.name_static(),
                self.prg_windows(),
                self.irq_latch,
                self.irq_enabled,
                self.audio_writes
            ),
        );
    }
    // Read a selected 1 KiB CHR bank using the eight register slots.
    fn read_chr(&mut self, addr: u16) -> u8 {
        read_bank(
            &self.chr,
            1024,
            self.chr_regs[((addr as usize) / 1024) & 7] as usize,
            addr as usize & 0x03FF,
        )
    }
    // Write through the same selected 1 KiB bank only for CHR RAM.
    fn write_chr(&mut self, addr: u16, value: u8) {
        if self.chr_ram {
            let slot = ((addr as usize) / 1024) & 7;
            let bank = self.chr_regs[slot] as usize;
            write_bank(&mut self.chr, 1024, bank, addr as usize & 0x03FF, value);
        }
    }
    // For enabled non-VRC2 IRQs, increment per CPU cycle or divide three
    // PPU-equivalent ticks per CPU cycle by 341. On an eight-bit overflow,
    // reload plus residual increments and latch the interrupt.
    fn clock_cpu(
        &mut self,
        cpu_cycles: u64,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        if !self.is_vrc2() && self.irq_enabled {
            let increments = if self.irq_control & 0x04 != 0 {
                cpu_cycles
            } else {
                let total = self.irq_prescaler as u64 + cpu_cycles * 3;
                self.irq_prescaler = (total % 341) as u16;
                total / 341
            };
            // This chunk update handles the overflow algebra once; it does not
            // iterate arbitrary numbers of repeated reload periods for huge inputs.
            let next = self.irq_counter as u64 + increments;
            if next >= 0x100 {
                self.irq_counter = (self.irq_latch as u64 + (next - 0x100)) as u16 & 0x00FF;
                self.irq_pending_flag = true;
                if cfg.mapper || cfg.nmi {
                    let mut event = TraceEvent::new("mapper.vrc_irq", frame, cycle);
                    event.message = Some(format!("{} IRQ pending", self.name_static()));
                    sink.push(event);
                }
            } else {
                self.irq_counter = next as u16;
            }
        }
    }
    // Expose the pending mapper IRQ latch.
    fn irq_pending(&self) -> bool {
        self.irq_pending_flag
    }
    // Clear only the pending latch, preserving timer and enable state.
    fn clear_irq(&mut self) {
        self.irq_pending_flag = false;
    }
    // Advance and mix the appropriate VRC6 or VRC7 audio model; other VRC
    // variants contribute silence.
    fn expansion_audio_sample(&mut self, sample_rate: u32) -> i16 {
        if self.is_vrc6() {
            self.vrc6_audio.sample(sample_rate)
        } else if self.is_vrc7() {
            self.vrc7_audio.sample(sample_rate)
        } else {
            0
        }
    }
    // Report effective PRG slots, CHR registers, IRQ and audio observations.
    // The mirroring field is only descriptive here: this type inherits the
    // trait's runtime four-screen policy.
    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            self.mapper,
            format!(
                "{} audio_writes={} vrc7_active={}",
                self.name_static(),
                self.audio_writes,
                self.vrc7_audio.active_channel_count()
            ),
            false,
            self.prg_windows().iter().map(|b| *b as u16).collect(),
            self.chr_regs.iter().map(|v| *v as u16).collect(),
            mirroring_name(self.mirroring),
            self.irq_pending(),
        )
    }
    // Use a V4-tagged restorable layout for variants without expansion
    // audio, and a larger observation layout for VRC6/7. Neither layout
    // includes mutable mirroring; audio layouts also omit some oscillator state.
    fn snapshot_bytes(&self) -> Vec<u8> {
        if !self.has_exp_audio() {
            let mut v = vec![
                b'V',
                b'4',
                self.mapper as u8,
                self.submapper,
                self.prg_mode as u8,
                self.irq_enabled as u8,
                self.irq_pending_flag as u8,
                self.irq_control,
            ];
            v.extend_from_slice(&self.irq_latch.to_le_bytes());
            v.extend_from_slice(&self.irq_counter.to_le_bytes());
            v.extend_from_slice(&self.irq_prescaler.to_le_bytes());
            v.extend_from_slice(&self.prg_regs);
            v.extend_from_slice(&self.chr_regs);
            v.extend_from_slice(&self.prg_ram);
            if self.chr_ram {
                v.extend_from_slice(&self.chr);
            }
            return v;
        }
        let mut v = vec![
            self.mapper as u8,
            self.irq_enabled as u8,
            self.irq_pending_flag as u8,
            self.irq_control,
        ];
        v.extend_from_slice(&self.irq_latch.to_le_bytes());
        v.extend_from_slice(&self.irq_counter.to_le_bytes());
        v.extend_from_slice(&self.irq_prescaler.to_le_bytes());
        v.extend_from_slice(&self.audio_writes.to_le_bytes());
        self.vrc6_audio.snapshot_bytes(&mut v);
        self.vrc7_audio.snapshot_bytes(&mut v);
        v.extend_from_slice(&self.prg_regs);
        v.extend_from_slice(&self.chr_regs);
        v.extend_from_slice(&self.audio_regs);
        v.extend_from_slice(&self.prg_ram);
        if self.chr_ram {
            v.extend_from_slice(&self.chr);
        }
        v
    }
    // Reject VRC6/7 restore. For other variants, validate length, V4 tag and
    // mapper/submapper before restoring banks, IRQ and RAM; existing mirroring
    // is retained because it is absent from the payload.
    fn restore_snapshot_bytes(&mut self, bytes: &[u8]) -> crate::error::Result<()> {
        if self.has_exp_audio() {
            return Err(crate::error::KurosakiError::SnapshotFormat(
                "VRC6/VRC7 restore pending".to_string(),
            ));
        }
        let expected = 26 + self.prg_ram.len() + if self.chr_ram { self.chr.len() } else { 0 };
        if bytes.len() != expected
            || bytes.get(..2) != Some(b"V4")
            || bytes[2] != self.mapper as u8
            || bytes[3] != self.submapper
        {
            return Err(crate::error::KurosakiError::SnapshotFormat(
                "VRC snapshot mismatch".to_string(),
            ));
        }
        self.prg_mode = bytes[4] != 0;
        self.irq_enabled = bytes[5] != 0;
        self.irq_pending_flag = bytes[6] != 0;
        self.irq_control = bytes[7];
        self.irq_latch = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
        self.irq_counter = u16::from_le_bytes(bytes[10..12].try_into().unwrap());
        self.irq_prescaler = u16::from_le_bytes(bytes[12..14].try_into().unwrap());
        self.prg_regs.copy_from_slice(&bytes[14..18]);
        self.chr_regs.copy_from_slice(&bytes[18..26]);
        let end = 26 + self.prg_ram.len();
        self.prg_ram.copy_from_slice(&bytes[26..end]);
        if self.chr_ram {
            self.chr.copy_from_slice(&bytes[end..]);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Namco163Mapper {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    mirroring: Mirroring,
    battery: bool,
    prg_regs: [u8; 3],
    chr_regs: [u8; 8],
    internal_ram: [u8; 128],
    ram_addr: u8,
    ram_auto_inc: bool,
    irq_counter: u16,
    irq_enabled: bool,
    irq_pending_flag: bool,
    audio_writes: u64,
}

impl Namco163Mapper {
    // Allocate work RAM, CHR and 128-byte internal register/wave RAM, select
    // initial PRG banks 0/1/2 and clear the IRQ and indirect-port state.
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring, battery: bool) -> Self {
        let (chr, chr_ram) = ensure_chr(chr_rom);
        Self {
            prg_rom,
            prg_ram: vec![0; 8 * 1024],
            chr,
            chr_ram,
            mirroring,
            battery,
            prg_regs: [0, 1, 2],
            chr_regs: [0; 8],
            internal_ram: [0; 128],
            ram_addr: 0,
            ram_auto_inc: false,
            irq_counter: 0,
            irq_enabled: false,
            irq_pending_flag: false,
            audio_writes: 0,
        }
    }
    // Wrap the three writable 8 KiB PRG selectors and keep the final ROM
    // bank fixed in the fourth slot.
    fn prg_windows(&self) -> [usize; 4] {
        let count = bank_count(self.prg_rom.len(), 8 * 1024);
        [
            self.prg_regs[0] as usize % count,
            self.prg_regs[1] as usize % count,
            self.prg_regs[2] as usize % count,
            count.saturating_sub(1),
        ]
    }
}

impl Mapper for Namco163Mapper {
    // Identify the Namco 163 scaffold as mapper 19.
    fn mapper_id(&self) -> u16 {
        19
    }
    // Return the Namco-specific scaffold label.
    fn mapper_name(&self) -> &'static str {
        "Namco 163 scaffold"
    }
    // Resolve a CPU ROM address through the selected 8 KiB PRG slot, with
    // no physical cartridge-ROM label for RAM or register addresses.
    fn physical_prg_bank_8k(&self, addr: u16) -> Option<u16> {
        if !(0x8000..=0xFFFF).contains(&addr) {
            return None;
        }
        let slot = ((addr - 0x8000) / 0x2000) as usize;
        physical_bank_8k(
            self.prg_rom.len(),
            8 * 1024,
            self.prg_windows()[slot],
            addr as usize & 0x1FFF,
        )
    }
    // Read indirect internal RAM at $4800 with optional seven-bit pointer
    // increment, expose counter bytes at $5000/$5800, or read work RAM/PRG
    // windows. Register reads do not acknowledge pending IRQ here.
    fn read_prg(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u8 {
        let value = match addr {
            0x4800 => {
                let v = self.internal_ram[(self.ram_addr & 0x7F) as usize];
                if self.ram_auto_inc {
                    self.ram_addr = self.ram_addr.wrapping_add(1) & 0x7F;
                }
                v
            }
            0x5000 => (self.irq_counter & 0xFF) as u8,
            0x5800 => ((self.irq_counter >> 8) as u8) | if self.irq_enabled { 0x80 } else { 0x00 },
            0x6000..=0x7FFF => self.prg_ram[(addr as usize - 0x6000) % self.prg_ram.len()],
            0x8000..=0x9FFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[0],
                addr as usize - 0x8000,
            ),
            0xA000..=0xBFFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[1],
                addr as usize - 0xA000,
            ),
            0xC000..=0xDFFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[2],
                addr as usize - 0xC000,
            ),
            0xE000..=0xFFFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[3],
                addr as usize - 0xE000,
            ),
            _ => 0,
        };
        if cfg.mapper && (0x4800..=0x5FFF).contains(&addr) {
            let mut event = TraceEvent::new("mapper.n163_read", frame, cycle);
            event.addr = Some(addr);
            event.value = Some(value);
            sink.push(event);
        }
        value
    }
    // Store internal RAM and IRQ fields or select banks through the modeled
    // address ranges. $E000-$E7FF selects the RAM pointer; $F800+ only selects
    // auto-increment. Internal RAM writes are counted as audio observations.
    fn write_prg(
        &mut self,
        addr: u16,
        value: u8,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        match addr {
            0x4800 => {
                self.internal_ram[(self.ram_addr & 0x7F) as usize] = value;
                self.audio_writes = self.audio_writes.saturating_add(1);
                if self.ram_auto_inc {
                    self.ram_addr = self.ram_addr.wrapping_add(1) & 0x7F;
                }
            }
            0x5000 => self.irq_counter = (self.irq_counter & 0x7F00) | value as u16,
            0x5800 => {
                self.irq_counter = (self.irq_counter & 0x00FF) | (((value & 0x7F) as u16) << 8);
                self.irq_enabled = value & 0x80 != 0;
                if !self.irq_enabled {
                    self.irq_pending_flag = false;
                }
            }
            0x6000..=0x7FFF => {
                let idx = (addr as usize - 0x6000) % self.prg_ram.len();
                self.prg_ram[idx] = value;
            }
            0x8000..=0xBFFF => self.chr_regs[((addr - 0x8000) / 0x0800) as usize] = value,
            // Four address groups wrap across three PRG registers in this scaffold;
            // the fourth group therefore aliases register zero.
            0xC000..=0xDFFF => self.prg_regs[((addr - 0xC000) / 0x0800) as usize % 3] = value,
            0xE000..=0xE7FF => self.ram_addr = value & 0x7F,
            0xF800..=0xFFFF => self.ram_auto_inc = value & 0x80 != 0,
            _ => {}
        }
        trace_mapper_write(
            if addr == 0x4800 {
                "mapper.expansion_audio_write"
            } else {
                "mapper.n163_write"
            },
            addr,
            value,
            frame,
            cycle,
            cfg,
            sink,
            format!(
                "Namco 163 write; PRG {:?}; IRQ={} enabled={} audio_writes={}",
                self.prg_windows(),
                self.irq_counter,
                self.irq_enabled,
                self.audio_writes
            ),
        );
    }
    // Read a wrapped 1 KiB CHR bank selected by the address's register slot.
    fn read_chr(&mut self, addr: u16) -> u8 {
        read_bank(
            &self.chr,
            1024,
            self.chr_regs[((addr as usize) / 1024) & 7] as usize,
            addr as usize & 0x03FF,
        )
    }
    // Write through the matching 1 KiB bank selection only for CHR RAM.
    fn write_chr(&mut self, addr: u16, value: u8) {
        if self.chr_ram {
            let slot = ((addr as usize) / 1024) & 7;
            let bank = self.chr_regs[slot] as usize;
            write_bank(&mut self.chr, 1024, bank, addr as usize & 0x03FF, value);
        }
    }
    // Add the CPU-cycle chunk, latch IRQ when the sum reaches $8000 and retain
    // the low 15 bits. The chunk is narrowed to u16; this is not a general
    // arbitrary-u64 elapsed-time update.
    fn clock_cpu(
        &mut self,
        cpu_cycles: u64,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        if self.irq_enabled {
            let next = self.irq_counter.saturating_add(cpu_cycles as u16);
            self.irq_counter = next & 0x7FFF;
            if next >= 0x8000 {
                self.irq_pending_flag = true;
                if cfg.mapper || cfg.nmi {
                    let mut event = TraceEvent::new("mapper.n163_irq", frame, cycle);
                    event.message = Some("Namco 163 IRQ pending".to_string());
                    sink.push(event);
                }
            }
        }
    }
    // Expose the pending mapper interrupt latch.
    fn irq_pending(&self) -> bool {
        self.irq_pending_flag
    }
    // Clear the pending latch without changing enable or counter state.
    fn clear_irq(&mut self) {
        self.irq_pending_flag = false;
    }
    // Report bank registers, diagnostic metadata and pending IRQ. The
    // mirroring string does not override the trait's four-screen runtime policy.
    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            19,
            format!(
                "Namco 163 scaffold audio_writes={} battery={}",
                self.audio_writes, self.battery
            ),
            false,
            self.prg_windows().iter().map(|b| *b as u16).collect(),
            self.chr_regs.iter().map(|v| *v as u16).collect(),
            mirroring_name(self.mirroring),
            self.irq_pending(),
        )
    }
    // Append port/IRQ fields, audio-write count, bank registers, internal/work
    // RAM and writable CHR. Audio synthesis, battery-RAM exposure and private
    // restore are not overridden by this scaffold.
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut v = vec![
            self.ram_addr,
            self.ram_auto_inc as u8,
            self.irq_enabled as u8,
            self.irq_pending_flag as u8,
            self.battery as u8,
        ];
        v.extend_from_slice(&self.irq_counter.to_le_bytes());
        v.extend_from_slice(&self.audio_writes.to_le_bytes());
        v.extend_from_slice(&self.prg_regs);
        v.extend_from_slice(&self.chr_regs);
        v.extend_from_slice(&self.internal_ram);
        v.extend_from_slice(&self.prg_ram);
        if self.chr_ram {
            v.extend_from_slice(&self.chr);
        }
        v
    }
}

#[derive(Debug, Clone)]
pub struct Sunsoft5bMapper {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    mirroring: Mirroring,
    battery: bool,
    command: u8,
    prg_regs: [u8; 4],
    chr_regs: [u8; 8],
    irq_counter: u16,
    irq_enabled: bool,
    irq_pending_flag: bool,
    audio_select: u8,
    audio_regs: [u8; 16],
    audio_writes: u64,
}

impl Sunsoft5bMapper {
    // Allocate backing, initialize first/final PRG banks and clear command,
    // IRQ and audio-register observation state.
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring, battery: bool) -> Self {
        let (chr, chr_ram) = ensure_chr(chr_rom);
        let c = bank_count(prg_rom.len(), 8 * 1024);
        Self {
            prg_rom,
            prg_ram: vec![0; 8 * 1024],
            chr,
            chr_ram,
            mirroring,
            battery,
            command: 0,
            prg_regs: [0, 1, c.saturating_sub(2) as u8, c.saturating_sub(1) as u8],
            chr_regs: [0; 8],
            irq_counter: 0,
            irq_enabled: false,
            irq_pending_flag: false,
            audio_select: 0,
            audio_regs: [0; 16],
            audio_writes: 0,
        }
    }
    // Use PRG registers 0-2 plus a fixed final 8 KiB ROM bank. Register 3 is
    // retained but does not affect these windows or the flat RAM window.
    fn prg_windows(&self) -> [usize; 4] {
        let c = bank_count(self.prg_rom.len(), 8 * 1024);
        [
            self.prg_regs[0] as usize % c,
            self.prg_regs[1] as usize % c,
            self.prg_regs[2] as usize % c,
            c.saturating_sub(1),
        ]
    }
}

impl Mapper for Sunsoft5bMapper {
    // Identify the FME-7/5B scaffold as mapper 69.
    fn mapper_id(&self) -> u16 {
        69
    }
    // Return the Sunsoft family scaffold label.
    fn mapper_name(&self) -> &'static str {
        "Sunsoft FME-7/5B scaffold"
    }
    // Resolve a CPU ROM address through the selected 8 KiB PRG slot, with
    // no physical cartridge-ROM label for RAM or register addresses.
    fn physical_prg_bank_8k(&self, addr: u16) -> Option<u16> {
        if !(0x8000..=0xFFFF).contains(&addr) {
            return None;
        }
        let slot = ((addr - 0x8000) / 0x2000) as usize;
        physical_bank_8k(
            self.prg_rom.len(),
            8 * 1024,
            self.prg_windows()[slot],
            addr as usize & 0x1FFF,
        )
    }
    // Read the flat 8 KiB work-RAM window or one of four modeled PRG-ROM
    // slots; other addresses return zero.
    fn read_prg(
        &mut self,
        addr: u16,
        _frame: u64,
        _cycle: u64,
        _cfg: TraceConfig,
        _sink: &mut TraceSink,
    ) -> u8 {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr as usize - 0x6000) % self.prg_ram.len()],
            0x8000..=0x9FFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[0],
                addr as usize - 0x8000,
            ),
            0xA000..=0xBFFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[1],
                addr as usize - 0xA000,
            ),
            0xC000..=0xDFFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[2],
                addr as usize - 0xC000,
            ),
            0xE000..=0xFFFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[3],
                addr as usize - 0xE000,
            ),
            _ => 0,
        }
    }
    // Latch a command at $8000-$9FFF and apply bank/mirroring/IRQ data at
    // $A000-$BFFF. The separate audio address/data regions retain shadows
    // and trace writes but do not implement an expansion sample generator.
    fn write_prg(
        &mut self,
        addr: u16,
        value: u8,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        match addr {
            0x8000..=0x9FFF => self.command = value & 0x0F,
            0xA000..=0xBFFF => match self.command {
                0..=7 => self.chr_regs[self.command as usize] = value,
                8 => self.prg_regs[0] = value,
                9 => self.prg_regs[1] = value,
                10 => self.prg_regs[2] = value,
                // Retain command eleven's data in the fourth shadow register even though
                // the effective upper PRG window stays fixed to the final ROM bank.
                11 => self.prg_regs[3] = value,
                12 => {
                    self.mirroring = if value & 1 == 0 {
                        Mirroring::Vertical
                    } else {
                        Mirroring::Horizontal
                    }
                }
                13 => {
                    self.irq_enabled = value & 1 != 0;
                    if !self.irq_enabled {
                        self.irq_pending_flag = false;
                    }
                }
                14 => self.irq_counter = (self.irq_counter & 0xFF00) | value as u16,
                15 => self.irq_counter = (self.irq_counter & 0x00FF) | ((value as u16) << 8),
                _ => {}
            },
            0xC000..=0xDFFF => self.audio_select = value & 0x0F,
            0xE000..=0xFFFF => {
                self.audio_regs[self.audio_select as usize] = value;
                self.audio_writes = self.audio_writes.saturating_add(1);
                trace_mapper_write(
                    "mapper.expansion_audio_write",
                    addr,
                    value,
                    frame,
                    cycle,
                    cfg,
                    sink,
                    "Sunsoft 5B YM2149-compatible audio register write observed",
                );
                return;
            }
            0x6000..=0x7FFF => {
                let idx = (addr as usize - 0x6000) % self.prg_ram.len();
                self.prg_ram[idx] = value;
            }
            _ => {}
        }
        trace_mapper_write(
            "mapper.sunsoft5b_write",
            addr,
            value,
            frame,
            cycle,
            cfg,
            sink,
            format!(
                "Sunsoft 5B command={} PRG {:?} IRQ={} enabled={}",
                self.command,
                self.prg_windows(),
                self.irq_counter,
                self.irq_enabled
            ),
        );
    }
    // Read a wrapped 1 KiB CHR bank selected by the address's register slot.
    fn read_chr(&mut self, addr: u16) -> u8 {
        read_bank(
            &self.chr,
            1024,
            self.chr_regs[((addr as usize) / 1024) & 7] as usize,
            addr as usize & 0x03FF,
        )
    }
    // Write through the matching 1 KiB bank selection only for CHR RAM.
    fn write_chr(&mut self, addr: u16, value: u8) {
        if self.chr_ram {
            let slot = ((addr as usize) / 1024) & 7;
            let bank = self.chr_regs[slot] as usize;
            write_bank(&mut self.chr, 1024, bank, addr as usize & 0x03FF, value);
        }
    }
    // Decrement an enabled positive counter by at most its remaining value
    // and latch IRQ on reaching zero. There is no automatic reload or wrap;
    // a zero counter remains idle until software changes it.
    fn clock_cpu(
        &mut self,
        cpu_cycles: u64,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        if self.irq_enabled && self.irq_counter > 0 {
            let dec = cpu_cycles.min(self.irq_counter as u64) as u16;
            self.irq_counter -= dec;
            if self.irq_counter == 0 {
                self.irq_pending_flag = true;
                if cfg.mapper || cfg.nmi {
                    let mut event = TraceEvent::new("mapper.sunsoft5b_irq", frame, cycle);
                    event.message = Some("Sunsoft FME-7 IRQ pending".to_string());
                    sink.push(event);
                }
            }
        }
    }
    // Expose the pending mapper interrupt latch.
    fn irq_pending(&self) -> bool {
        self.irq_pending_flag
    }
    // Clear the pending latch without changing enable or counter state.
    fn clear_irq(&mut self) {
        self.irq_pending_flag = false;
    }
    // Report bank registers, diagnostic metadata and pending IRQ. The
    // mirroring string does not override the trait's four-screen runtime policy.
    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            69,
            format!(
                "Sunsoft 5B scaffold audio_writes={} battery={}",
                self.audio_writes, self.battery
            ),
            false,
            self.prg_windows().iter().map(|b| *b as u16).collect(),
            self.chr_regs.iter().map(|v| *v as u16).collect(),
            mirroring_name(self.mirroring),
            self.irq_pending(),
        )
    }
    // Serialize command/IRQ/audio fields, bank and audio shadows, work RAM
    // and optional CHR RAM. Mutable mirroring is omitted; battery export
    // and private restore remain unsupported through the trait defaults.
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut v = vec![
            self.command,
            self.irq_enabled as u8,
            self.irq_pending_flag as u8,
            self.audio_select,
            self.battery as u8,
        ];
        v.extend_from_slice(&self.irq_counter.to_le_bytes());
        v.extend_from_slice(&self.audio_writes.to_le_bytes());
        v.extend_from_slice(&self.prg_regs);
        v.extend_from_slice(&self.chr_regs);
        v.extend_from_slice(&self.audio_regs);
        v.extend_from_slice(&self.prg_ram);
        if self.chr_ram {
            v.extend_from_slice(&self.chr);
        }
        v
    }
}

#[derive(Debug, Clone)]
pub struct BandaiFcgMapper {
    mapper: u16,
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    mirroring: Mirroring,
    battery: bool,
    prg_bank: u8,
    chr_regs: [u8; 8],
    irq_counter: u16,
    irq_enabled: bool,
    irq_pending_flag: bool,
    eeprom_events: u64,
}

impl BandaiFcgMapper {
    // Retain the mapper ID and battery/header metadata, allocate backing
    // and clear PRG/CHR selectors, IRQ and EEPROM-observation count.
    pub fn new(
        mapper: u16,
        prg_rom: Vec<u8>,
        chr_rom: Vec<u8>,
        mirroring: Mirroring,
        battery: bool,
    ) -> Self {
        let (chr, chr_ram) = ensure_chr(chr_rom);
        Self {
            mapper,
            prg_rom,
            prg_ram: vec![0; 8 * 1024],
            chr,
            chr_ram,
            mirroring,
            battery,
            prg_bank: 0,
            chr_regs: [0; 8],
            irq_counter: 0,
            irq_enabled: false,
            irq_pending_flag: false,
            eeprom_events: 0,
        }
    }
}

impl Mapper for BandaiFcgMapper {
    // Return the configured FCG variant mapper ID.
    fn mapper_id(&self) -> u16 {
        self.mapper
    }
    // Return the Bandai FCG scaffold label.
    fn mapper_name(&self) -> &'static str {
        "Bandai FCG scaffold"
    }
    // Convert the selectable lower or final fixed upper 16 KiB PRG window
    // into a physical 8 KiB ROM label.
    fn physical_prg_bank_8k(&self, addr: u16) -> Option<u16> {
        let (bank, offset) = match addr {
            0x8000..=0xBFFF => (self.prg_bank as usize, addr as usize - 0x8000),
            0xC000..=0xFFFF => (
                bank_count(self.prg_rom.len(), 16 * 1024).saturating_sub(1),
                addr as usize - 0xC000,
            ),
            _ => return None,
        };
        physical_bank_8k(self.prg_rom.len(), 16 * 1024, bank, offset)
    }
    // Read flat work RAM, a selectable lower 16 KiB ROM bank or the fixed
    // final upper bank. No EEPROM read protocol is modeled.
    fn read_prg(
        &mut self,
        addr: u16,
        _frame: u64,
        _cycle: u64,
        _cfg: TraceConfig,
        _sink: &mut TraceSink,
    ) -> u8 {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr as usize - 0x6000) % self.prg_ram.len()],
            0x8000..=0xBFFF => read_bank(
                &self.prg_rom,
                16 * 1024,
                self.prg_bank as usize,
                addr as usize - 0x8000,
            ),
            0xC000..=0xFFFF => read_bank(
                &self.prg_rom,
                16 * 1024,
                bank_count(self.prg_rom.len(), 16 * 1024).saturating_sub(1),
                addr as usize - 0xC000,
            ),
            _ => 0,
        }
    }
    // Decode the scaffold's broad RAM/bank/mirroring/IRQ regions. High-counter
    // writes also increment the EEPROM-event count; this is observation
    // metadata rather than an EEPROM serial protocol or stored EEPROM image.
    fn write_prg(
        &mut self,
        addr: u16,
        value: u8,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        match addr {
            0x6000..=0x7FFF => {
                let idx = (addr as usize - 0x6000) % self.prg_ram.len();
                self.prg_ram[idx] = value;
            }
            0x8000..=0xBFFF => self.chr_regs[((addr - 0x8000) / 0x0800) as usize & 7] = value,
            0xC000..=0xCFFF => self.prg_bank = value,
            0xD000..=0xDFFF => {
                self.mirroring = if value & 1 == 0 {
                    Mirroring::Vertical
                } else {
                    Mirroring::Horizontal
                }
            }
            0xE000..=0xEFFF => {
                self.irq_enabled = value & 1 != 0;
                if !self.irq_enabled {
                    self.irq_pending_flag = false;
                }
            }
            0xF000..=0xF7FF => self.irq_counter = (self.irq_counter & 0xFF00) | value as u16,
            0xF800..=0xFFFF => {
                self.irq_counter = (self.irq_counter & 0x00FF) | ((value as u16) << 8);
                // Count the observed high-counter write; no EEPROM bit-stream state
                // machine is advanced by this assignment.
                self.eeprom_events = self.eeprom_events.saturating_add(1);
            }
            _ => {}
        }
        trace_mapper_write(
            "mapper.bandai_fcg_write",
            addr,
            value,
            frame,
            cycle,
            cfg,
            sink,
            format!(
                "Bandai FCG mapper={} PRG={} IRQ={} enabled={} eeprom_events={}",
                self.mapper, self.prg_bank, self.irq_counter, self.irq_enabled, self.eeprom_events
            ),
        );
    }
    // Read a wrapped 1 KiB CHR bank selected by the address's register slot.
    fn read_chr(&mut self, addr: u16) -> u8 {
        read_bank(
            &self.chr,
            1024,
            self.chr_regs[((addr as usize) / 1024) & 7] as usize,
            addr as usize & 0x03FF,
        )
    }
    // Write through the matching 1 KiB bank selection only for CHR RAM.
    fn write_chr(&mut self, addr: u16, value: u8) {
        if self.chr_ram {
            let slot = ((addr as usize) / 1024) & 7;
            let bank = self.chr_regs[slot] as usize;
            write_bank(&mut self.chr, 1024, bank, addr as usize & 0x03FF, value);
        }
    }
    // Decrement an enabled positive counter by at most its remaining value
    // and latch IRQ on reaching zero. There is no automatic reload or wrap;
    // a zero counter remains idle until software changes it.
    fn clock_cpu(
        &mut self,
        cpu_cycles: u64,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        if self.irq_enabled && self.irq_counter > 0 {
            let dec = cpu_cycles.min(self.irq_counter as u64) as u16;
            self.irq_counter -= dec;
            if self.irq_counter == 0 {
                self.irq_pending_flag = true;
                if cfg.mapper || cfg.nmi {
                    let mut event = TraceEvent::new("mapper.bandai_irq", frame, cycle);
                    event.message = Some("Bandai FCG IRQ pending".to_string());
                    sink.push(event);
                }
            }
        }
    }
    // Expose the pending mapper interrupt latch.
    fn irq_pending(&self) -> bool {
        self.irq_pending_flag
    }
    // Clear the pending latch without changing enable or counter state.
    fn clear_irq(&mut self) {
        self.irq_pending_flag = false;
    }
    // Describe logical 16 KiB PRG selections, CHR registers, IRQ and EEPROM
    // observations. Stored mirroring remains diagnostic rather than a runtime
    // override, and the battery flag alone does not expose sidecar RAM.
    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            self.mapper,
            format!(
                "Bandai FCG scaffold eeprom_events={} battery={}",
                self.eeprom_events, self.battery
            ),
            false,
            vec![
                self.prg_bank as u16,
                bank_count(self.prg_rom.len(), 16 * 1024).saturating_sub(1) as u16,
            ],
            self.chr_regs.iter().map(|v| *v as u16).collect(),
            mirroring_name(self.mirroring),
            self.irq_pending(),
        )
    }
    // Append mapper/PRG/IRQ fields, EEPROM-event count, CHR registers and RAM.
    // The type has no private restore decoder and omits mutable mirroring.
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut v = vec![
            self.mapper as u8,
            self.prg_bank,
            self.irq_enabled as u8,
            self.irq_pending_flag as u8,
            self.battery as u8,
        ];
        v.extend_from_slice(&self.irq_counter.to_le_bytes());
        v.extend_from_slice(&self.eeprom_events.to_le_bytes());
        v.extend_from_slice(&self.chr_regs);
        v.extend_from_slice(&self.prg_ram);
        if self.chr_ram {
            v.extend_from_slice(&self.chr);
        }
        v
    }
}

#[derive(Debug, Clone)]
pub struct JalecoSs88006Mapper {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    mirroring: Mirroring,
    battery: bool,
    prg_regs: [u8; 3],
    chr_regs: [u8; 8],
    irq_counter: u16,
    irq_enabled: bool,
    irq_pending_flag: bool,
}

impl JalecoSs88006Mapper {
    // Allocate work/CHR backing, initialize PRG selectors 0/1/2 and clear
    // the counter and IRQ flags.
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring, battery: bool) -> Self {
        let (chr, chr_ram) = ensure_chr(chr_rom);
        Self {
            prg_rom,
            prg_ram: vec![0; 8 * 1024],
            chr,
            chr_ram,
            mirroring,
            battery,
            prg_regs: [0, 1, 2],
            chr_regs: [0; 8],
            irq_counter: 0,
            irq_enabled: false,
            irq_pending_flag: false,
        }
    }
    // Wrap the three writable 8 KiB PRG selectors and fix the last ROM bank
    // in the uppermost slot.
    fn prg_windows(&self) -> [usize; 4] {
        let c = bank_count(self.prg_rom.len(), 8 * 1024);
        [
            self.prg_regs[0] as usize % c,
            self.prg_regs[1] as usize % c,
            self.prg_regs[2] as usize % c,
            c.saturating_sub(1),
        ]
    }
}

impl Mapper for JalecoSs88006Mapper {
    // Identify the SS88006 scaffold as mapper 18.
    fn mapper_id(&self) -> u16 {
        18
    }
    // Return the Jaleco-specific scaffold label.
    fn mapper_name(&self) -> &'static str {
        "Jaleco SS88006 scaffold"
    }
    // Resolve a CPU ROM address through the selected 8 KiB PRG slot, with
    // no physical cartridge-ROM label for RAM or register addresses.
    fn physical_prg_bank_8k(&self, addr: u16) -> Option<u16> {
        if !(0x8000..=0xFFFF).contains(&addr) {
            return None;
        }
        let slot = ((addr - 0x8000) / 0x2000) as usize;
        physical_bank_8k(
            self.prg_rom.len(),
            8 * 1024,
            self.prg_windows()[slot],
            addr as usize & 0x1FFF,
        )
    }
    // Read the flat 8 KiB work-RAM window or one of four modeled PRG-ROM
    // slots; other addresses return zero.
    fn read_prg(
        &mut self,
        addr: u16,
        _frame: u64,
        _cycle: u64,
        _cfg: TraceConfig,
        _sink: &mut TraceSink,
    ) -> u8 {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr as usize - 0x6000) % self.prg_ram.len()],
            0x8000..=0x9FFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[0],
                addr as usize - 0x8000,
            ),
            0xA000..=0xBFFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[1],
                addr as usize - 0xA000,
            ),
            0xC000..=0xDFFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[2],
                addr as usize - 0xC000,
            ),
            0xE000..=0xFFFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[3],
                addr as usize - 0xE000,
            ),
            _ => 0,
        }
    }
    // Route broad address regions to whole-byte PRG/CHR selectors and low/
    // high counter fields; the final region controls IRQ enable. This decoder
    // does not assemble the bank registers through nibble writes.
    fn write_prg(
        &mut self,
        addr: u16,
        value: u8,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        match addr {
            0x6000..=0x7FFF => {
                let idx = (addr as usize - 0x6000) % self.prg_ram.len();
                self.prg_ram[idx] = value;
            }
            0x8000..=0x9FFF => self.prg_regs[((addr - 0x8000) / 0x0800) as usize % 3] = value,
            0xA000..=0xDFFF => self.chr_regs[((addr - 0xA000) / 0x0800) as usize % 8] = value,
            0xE000..=0xEFFF => self.irq_counter = (self.irq_counter & 0xFF00) | value as u16,
            0xF000..=0xF7FF => {
                self.irq_counter = (self.irq_counter & 0x00FF) | ((value as u16) << 8)
            }
            0xF800..=0xFFFF => {
                self.irq_enabled = value & 1 != 0;
                if !self.irq_enabled {
                    self.irq_pending_flag = false;
                }
            }
            _ => {}
        }
        trace_mapper_write(
            "mapper.jaleco_ss88006_write",
            addr,
            value,
            frame,
            cycle,
            cfg,
            sink,
            format!(
                "Jaleco SS88006 PRG {:?} IRQ={} enabled={}",
                self.prg_windows(),
                self.irq_counter,
                self.irq_enabled
            ),
        );
    }
    // Read a wrapped 1 KiB CHR bank selected by the address's register slot.
    fn read_chr(&mut self, addr: u16) -> u8 {
        read_bank(
            &self.chr,
            1024,
            self.chr_regs[((addr as usize) / 1024) & 7] as usize,
            addr as usize & 0x03FF,
        )
    }
    // Write through the matching 1 KiB bank selection only for CHR RAM.
    fn write_chr(&mut self, addr: u16, value: u8) {
        if self.chr_ram {
            let slot = ((addr as usize) / 1024) & 7;
            let bank = self.chr_regs[slot] as usize;
            write_bank(&mut self.chr, 1024, bank, addr as usize & 0x03FF, value);
        }
    }
    // Decrement an enabled positive counter by at most its remaining value
    // and latch IRQ on reaching zero. There is no automatic reload or wrap;
    // a zero counter remains idle until software changes it.
    fn clock_cpu(
        &mut self,
        cpu_cycles: u64,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        if self.irq_enabled && self.irq_counter > 0 {
            let dec = cpu_cycles.min(self.irq_counter as u64) as u16;
            self.irq_counter -= dec;
            if self.irq_counter == 0 {
                self.irq_pending_flag = true;
                if cfg.mapper || cfg.nmi {
                    let mut event = TraceEvent::new("mapper.jaleco_irq", frame, cycle);
                    event.message = Some("Jaleco SS88006 IRQ pending".to_string());
                    sink.push(event);
                }
            }
        }
    }
    // Expose the pending mapper interrupt latch.
    fn irq_pending(&self) -> bool {
        self.irq_pending_flag
    }
    // Clear the pending latch without changing enable or counter state.
    fn clear_irq(&mut self) {
        self.irq_pending_flag = false;
    }
    // Report bank registers, diagnostic metadata and pending IRQ. The
    // mirroring string does not override the trait's four-screen runtime policy.
    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            18,
            format!("Jaleco SS88006 scaffold battery={}", self.battery),
            false,
            self.prg_windows().iter().map(|b| *b as u16).collect(),
            self.chr_regs.iter().map(|v| *v as u16).collect(),
            mirroring_name(self.mirroring),
            self.irq_pending(),
        )
    }
    // Append IRQ/battery fields, counter, bank registers and work/CHR RAM
    // for observation; battery-RAM exposure and restore remain trait defaults.
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut v = vec![
            self.irq_enabled as u8,
            self.irq_pending_flag as u8,
            self.battery as u8,
        ];
        v.extend_from_slice(&self.irq_counter.to_le_bytes());
        v.extend_from_slice(&self.prg_regs);
        v.extend_from_slice(&self.chr_regs);
        v.extend_from_slice(&self.prg_ram);
        if self.chr_ram {
            v.extend_from_slice(&self.chr);
        }
        v
    }
}

#[derive(Debug, Clone)]
pub struct BoardScaffoldMapper {
    mapper: u16,
    spec: MapperSpec,
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    mirroring: Mirroring,
    prg_regs: [u8; 4],
    chr_regs: [u8; 8],
    control: u8,
    irq_counter: u16,
    irq_enabled: bool,
    irq_pending_flag: bool,
}

impl BoardScaffoldMapper {
    // Load the board's registry label and allocate common RAM/CHR backing with
    // initial first/final PRG banks, zero control and disabled IRQ.
    pub fn new(mapper: u16, prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        let spec = mapper_spec(mapper);
        let (chr, chr_ram) = ensure_chr(chr_rom);
        let c = bank_count(prg_rom.len(), 8 * 1024);
        Self {
            mapper,
            spec,
            prg_rom,
            prg_ram: vec![0; 8 * 1024],
            chr,
            chr_ram,
            mirroring,
            prg_regs: [0, 1, c.saturating_sub(2) as u8, c.saturating_sub(1) as u8],
            chr_regs: [0; 8],
            control: 0,
            irq_counter: 0,
            irq_enabled: false,
            irq_pending_flag: false,
        }
    }
    // Wrap all four stored 8 KiB PRG selectors to the available ROM banks.
    fn prg_windows(&self) -> [usize; 4] {
        let c = bank_count(self.prg_rom.len(), 8 * 1024);
        [
            self.prg_regs[0] as usize % c,
            self.prg_regs[1] as usize % c,
            self.prg_regs[2] as usize % c,
            self.prg_regs[3] as usize % c,
        ]
    }
}

impl Mapper for BoardScaffoldMapper {
    // Retain the cartridge mapper ID associated with this common scaffold.
    fn mapper_id(&self) -> u16 {
        self.mapper
    }
    // Identify the implementation as a shared board-family scaffold.
    fn mapper_name(&self) -> &'static str {
        "Board-family scaffold"
    }
    // Resolve a CPU ROM address through the selected 8 KiB PRG slot, with
    // no physical cartridge-ROM label for RAM or register addresses.
    fn physical_prg_bank_8k(&self, addr: u16) -> Option<u16> {
        if !(0x8000..=0xFFFF).contains(&addr) {
            return None;
        }
        let slot = ((addr - 0x8000) / 0x2000) as usize;
        physical_bank_8k(
            self.prg_rom.len(),
            8 * 1024,
            self.prg_windows()[slot],
            addr as usize & 0x1FFF,
        )
    }
    // Read the flat 8 KiB work-RAM window or one of four modeled PRG-ROM
    // slots; other addresses return zero.
    fn read_prg(
        &mut self,
        addr: u16,
        _frame: u64,
        _cycle: u64,
        _cfg: TraceConfig,
        _sink: &mut TraceSink,
    ) -> u8 {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr as usize - 0x6000) % self.prg_ram.len()],
            0x8000..=0x9FFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[0],
                addr as usize - 0x8000,
            ),
            0xA000..=0xBFFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[1],
                addr as usize - 0xA000,
            ),
            0xC000..=0xDFFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[2],
                addr as usize - 0xC000,
            ),
            0xE000..=0xFFFF => read_bank(
                &self.prg_rom,
                8 * 1024,
                self.prg_windows()[3],
                addr as usize - 0xE000,
            ),
            _ => 0,
        }
    }
    // Apply the shared bank/control/counter address layout without branching
    // on mapper ID. Only PRG registers 0/1 are written here; control is stored
    // for observation and does not alter bank selection.
    fn write_prg(
        &mut self,
        addr: u16,
        value: u8,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        match addr {
            0x6000..=0x7FFF => {
                let idx = (addr as usize - 0x6000) % self.prg_ram.len();
                self.prg_ram[idx] = value;
            }
            0x8000..=0x8FFF => self.prg_regs[0] = value,
            0x9000..=0x9FFF => self.control = value,
            0xA000..=0xAFFF => self.prg_regs[1] = value,
            0xB000..=0xEFFF => self.chr_regs[((addr - 0xB000) / 0x0800) as usize % 8] = value,
            0xF000..=0xF7FF => self.irq_counter = (self.irq_counter & 0xFF00) | value as u16,
            // In this shared decoder, the same high-byte write contains both the
            // counter's bit 15 and its IRQ-enable flag.
            0xF800..=0xFFFF => {
                self.irq_counter = (self.irq_counter & 0x00FF) | ((value as u16) << 8);
                self.irq_enabled = value & 0x80 != 0;
                if !self.irq_enabled {
                    self.irq_pending_flag = false;
                }
            }
            _ => {}
        }
        trace_mapper_write(
            "mapper.board_scaffold_write",
            addr,
            value,
            frame,
            cycle,
            cfg,
            sink,
            format!(
                "{} scaffold write; PRG {:?} CHR {:?} control={:02X}",
                self.spec.name,
                self.prg_windows(),
                self.chr_regs,
                self.control
            ),
        );
    }
    // Read a wrapped 1 KiB CHR bank selected by the address's register slot.
    fn read_chr(&mut self, addr: u16) -> u8 {
        read_bank(
            &self.chr,
            1024,
            self.chr_regs[((addr as usize) / 1024) & 7] as usize,
            addr as usize & 0x03FF,
        )
    }
    // Write through the matching 1 KiB bank selection only for CHR RAM.
    fn write_chr(&mut self, addr: u16, value: u8) {
        if self.chr_ram {
            let slot = ((addr as usize) / 1024) & 7;
            let bank = self.chr_regs[slot] as usize;
            write_bank(&mut self.chr, 1024, bank, addr as usize & 0x03FF, value);
        }
    }
    // Decrement an enabled positive counter by at most its remaining value
    // and latch IRQ on reaching zero. There is no automatic reload or wrap;
    // a zero counter remains idle until software changes it.
    fn clock_cpu(
        &mut self,
        cpu_cycles: u64,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        if self.irq_enabled && self.irq_counter > 0 {
            let dec = cpu_cycles.min(self.irq_counter as u64) as u16;
            self.irq_counter -= dec;
            if self.irq_counter == 0 {
                self.irq_pending_flag = true;
                if cfg.mapper || cfg.nmi {
                    let mut event = TraceEvent::new("mapper.board_irq", frame, cycle);
                    event.message = Some(format!("{} scaffold IRQ pending", self.spec.name));
                    sink.push(event);
                }
            }
        }
    }
    // Expose the pending mapper interrupt latch.
    fn irq_pending(&self) -> bool {
        self.irq_pending_flag
    }
    // Clear the pending latch without changing enable or counter state.
    fn clear_irq(&mut self) {
        self.irq_pending_flag = false;
    }
    // Report bank registers, diagnostic metadata and pending IRQ. The
    // mirroring string does not override the trait's four-screen runtime policy.
    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            self.mapper,
            format!("{} scaffold", self.spec.name),
            false,
            self.prg_windows().iter().map(|b| *b as u16).collect(),
            self.chr_regs.iter().map(|v| *v as u16).collect(),
            mirroring_name(self.mirroring),
            self.irq_pending(),
        )
    }
    // Append mapper/control/IRQ fields, all bank registers and writable RAM.
    // The representation supports observation; private restore is not implemented.
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut v = vec![
            self.mapper as u8,
            self.control,
            self.irq_enabled as u8,
            self.irq_pending_flag as u8,
        ];
        v.extend_from_slice(&self.irq_counter.to_le_bytes());
        v.extend_from_slice(&self.prg_regs);
        v.extend_from_slice(&self.chr_regs);
        v.extend_from_slice(&self.prg_ram);
        if self.chr_ram {
            v.extend_from_slice(&self.chr);
        }
        v
    }
}

#[cfg(test)]
mod vrc7_audio_tests {
    use super::*;

    // Write a register through address/data ports and require both accesses
    // to be accepted by the decoder.
    fn write_vrc7_reg(vrc7: &mut Vrc7AudioState, reg: u8, value: u8) {
        assert!(vrc7.write(0x9010, reg));
        assert!(vrc7.write(0x9030, value));
    }

    #[test]
    // Check shadow register, combined frequency, key state, instrument and
    // active-channel count after programming one channel.
    fn vrc7_register_port_latches_and_updates_channel() {
        let mut vrc7 = Vrc7AudioState::default();
        write_vrc7_reg(&mut vrc7, 0x10, 0x80);
        write_vrc7_reg(&mut vrc7, 0x20, 0x19); // key on, block 4, high fnum bit 1
        write_vrc7_reg(&mut vrc7, 0x30, 0x11); // preset 1, loud volume
        assert_eq!(vrc7.regs[0x10], 0x80);
        assert_eq!(vrc7.channels[0].fnum(), 0x180);
        assert!(vrc7.channels[0].key_on);
        assert_eq!(vrc7.channels[0].instrument, 1);
        assert_eq!(vrc7.active_channel_count(), 1);
    }

    #[test]
    // Require at least one nonzero sample from a keyed preset channel within
    // a bounded sample loop; no waveform or pitch reference is compared.
    fn vrc7_key_on_outputs_nonzero_samples() {
        let mut vrc7 = Vrc7AudioState::default();
        write_vrc7_reg(&mut vrc7, 0x10, 0x80);
        write_vrc7_reg(&mut vrc7, 0x20, 0x19);
        write_vrc7_reg(&mut vrc7, 0x30, 0x10);
        let mut nonzero = false;
        for _ in 0..4096 {
            if vrc7.sample(44_100) != 0 {
                nonzero = true;
                break;
            }
        }
        assert!(nonzero);
    }

    #[test]
    // Program all user-patch bytes and check selected operator flags plus
    // modulation/feedback bounds; exact envelope-rate values are not asserted.
    fn vrc7_user_patch_decodes_slot_flags_and_rates() {
        let mut vrc7 = Vrc7AudioState::default();
        write_vrc7_reg(&mut vrc7, 0x00, 0xF1); // AM + VIB + sustain + KSR, mul=1
        write_vrc7_reg(&mut vrc7, 0x01, 0x02);
        write_vrc7_reg(&mut vrc7, 0x02, 0x08);
        write_vrc7_reg(&mut vrc7, 0x03, 0x18); // mod and carrier half-sine flags
        write_vrc7_reg(&mut vrc7, 0x04, 0xF1);
        write_vrc7_reg(&mut vrc7, 0x05, 0xE2);
        write_vrc7_reg(&mut vrc7, 0x06, 0x34);
        write_vrc7_reg(&mut vrc7, 0x07, 0x56);
        let patch = vrc7.user_patch_summary();
        assert!(patch.mod_slot.am);
        assert!(patch.mod_slot.vib);
        assert!(patch.mod_slot.eg_sustain);
        assert!(patch.mod_slot.key_scale);
        assert!(patch.mod_slot.half_sine);
        assert!(patch.car_slot.half_sine);
        assert!(patch.mod_index > 1.0);
        assert!(patch.feedback >= 0.0);
    }

    #[test]
    // Build a nonzero carrier envelope, clear key-on and check immediate
    // Release state plus low gain after one second of modeled samples.
    fn vrc7_key_off_enters_release_and_fades() {
        let mut vrc7 = Vrc7AudioState::default();
        write_vrc7_reg(&mut vrc7, 0x10, 0xA0);
        write_vrc7_reg(&mut vrc7, 0x20, 0x19);
        write_vrc7_reg(&mut vrc7, 0x30, 0x10);
        for _ in 0..2048 {
            let _ = vrc7.sample(44_100);
        }
        assert!(vrc7.channels[0].car_slot.env > 0.0);
        write_vrc7_reg(&mut vrc7, 0x20, 0x09); // clear key-on
        assert_eq!(vrc7.channels[0].car_slot.eg_phase, Vrc7EgPhase::Release);
        for _ in 0..44_100 {
            let _ = vrc7.sample(44_100);
        }
        assert!(vrc7.channels[0].car_slot.env < 0.05);
    }

    #[test]
    // Check the $9018/$9038 address/data mirrors by reading the updated shadow.
    fn vrc7_audio_ports_accept_common_low_address_mirrors() {
        let mut vrc7 = Vrc7AudioState::default();
        assert!(vrc7.write(0x9018, 0x10));
        assert!(vrc7.write(0x9038, 0x80));
        assert_eq!(vrc7.regs[0x10], 0x80);
    }
    #[test]
    // Program VRC7 through mapper CPU writes, require nonzero expansion
    // samples and check the diagnostic active-channel count.
    fn vrc_family_mapper_routes_vrc7_audio_ports() {
        let mut mapper = VrcFamilyMapper::new(
            85,
            vec![0; 32 * 1024],
            vec![0; 8 * 1024],
            Mirroring::Horizontal,
        );
        let mut sink = TraceSink::default();
        mapper.write_prg(0x9010, 0x10, 0, 0, TraceConfig::none(), &mut sink);
        mapper.write_prg(0x9030, 0x80, 0, 0, TraceConfig::none(), &mut sink);
        mapper.write_prg(0x9010, 0x20, 0, 0, TraceConfig::none(), &mut sink);
        mapper.write_prg(0x9030, 0x19, 0, 0, TraceConfig::none(), &mut sink);
        mapper.write_prg(0x9010, 0x30, 0, 0, TraceConfig::none(), &mut sink);
        mapper.write_prg(0x9030, 0x10, 0, 0, TraceConfig::none(), &mut sink);
        let mut nonzero = false;
        for _ in 0..4096 {
            if mapper.expansion_audio_sample(44_100) != 0 {
                nonzero = true;
                break;
            }
        }
        assert!(nonzero);
        assert!(mapper.debug_state().name.contains("vrc7_active=1"));
    }

    // Fill each original synthetic 8 KiB PRG bank with its own ID so reads
    // reveal selected bank windows without executing a game.
    fn marker_banks(count: usize) -> Vec<u8> {
        let mut prg = Vec::new();
        for bank in 0..count {
            prg.extend(std::iter::repeat_n(bank as u8, 8 * 1024));
        }
        prg
    }

    // Fill original synthetic 1 KiB CHR banks with distinct ID bytes for
    // address-routing assertions.
    fn marker_chr_1k(count: usize) -> Vec<u8> {
        let mut chr = Vec::new();
        for bank in 0..count {
            chr.extend(std::iter::repeat_n(bank as u8, 1024));
        }
        chr
    }

    #[test]
    // Write all eight VRC7 CHR selector addresses, then check register
    // summaries and the first byte read through each pattern-memory slot.
    fn vrc7_chr_registers_use_konami_1k_addresses() {
        let mut mapper = VrcFamilyMapper::new(
            85,
            vec![0; 32 * 1024],
            marker_chr_1k(8),
            Mirroring::Horizontal,
        );
        let mut sink = TraceSink::default();
        mapper.write_prg(0xA000, 0, 0, 0, TraceConfig::none(), &mut sink);
        mapper.write_prg(0xA010, 1, 0, 0, TraceConfig::none(), &mut sink);
        mapper.write_prg(0xB000, 2, 0, 0, TraceConfig::none(), &mut sink);
        mapper.write_prg(0xB010, 3, 0, 0, TraceConfig::none(), &mut sink);
        mapper.write_prg(0xC000, 4, 0, 0, TraceConfig::none(), &mut sink);
        mapper.write_prg(0xC010, 5, 0, 0, TraceConfig::none(), &mut sink);
        mapper.write_prg(0xD000, 6, 0, 0, TraceConfig::none(), &mut sink);
        mapper.write_prg(0xD010, 7, 0, 0, TraceConfig::none(), &mut sink);

        assert_eq!(
            mapper.debug_state().chr_bank_window,
            vec![0, 1, 2, 3, 4, 5, 6, 7]
        );
        assert_eq!(mapper.read_chr(0x0000), 0);
        assert_eq!(mapper.read_chr(0x0400), 1);
        assert_eq!(mapper.read_chr(0x0800), 2);
        assert_eq!(mapper.read_chr(0x0C00), 3);
        assert_eq!(mapper.read_chr(0x1000), 4);
        assert_eq!(mapper.read_chr(0x1400), 5);
        assert_eq!(mapper.read_chr(0x1800), 6);
        assert_eq!(mapper.read_chr(0x1C00), 7);
    }

    #[test]
    // Check three independently selected VRC7 PRG windows and require audio
    // port writes to preserve the $C000 window. No boot stub is executed here.
    fn vrc7_9000_selects_c000_prg_window_for_kitaqfc_boot_stub() {
        let mut mapper = VrcFamilyMapper::new(
            85,
            marker_banks(5),
            vec![0; 8 * 1024],
            Mirroring::Horizontal,
        );
        let mut sink = TraceSink::default();
        mapper.write_prg(0x9000, 2, 0, 0, TraceConfig::none(), &mut sink);
        assert_eq!(
            mapper.read_prg(0xC000, 0, 0, TraceConfig::none(), &mut sink),
            2
        );
        mapper.write_prg(0x8000, 3, 0, 0, TraceConfig::none(), &mut sink);
        mapper.write_prg(0x8010, 4, 0, 0, TraceConfig::none(), &mut sink);
        assert_eq!(
            mapper.read_prg(0x8000, 0, 0, TraceConfig::none(), &mut sink),
            3
        );
        assert_eq!(
            mapper.read_prg(0xA000, 0, 0, TraceConfig::none(), &mut sink),
            4
        );
        mapper.write_prg(0x9010, 0x10, 0, 0, TraceConfig::none(), &mut sink);
        mapper.write_prg(0x9030, 0x80, 0, 0, TraceConfig::none(), &mut sink);
        assert_eq!(
            mapper.read_prg(0xC000, 0, 0, TraceConfig::none(), &mut sink),
            2
        );
    }

    #[test]
    // Check that mapper 24's $C000 register selects the corresponding PRG
    // window using a synthetic bank-ID read.
    fn vrc6_c000_selects_c000_prg_window_for_kitaqfc_boot_stub() {
        let mut mapper = VrcFamilyMapper::new(
            24,
            marker_banks(5),
            vec![0; 8 * 1024],
            Mirroring::Horizontal,
        );
        let mut sink = TraceSink::default();
        mapper.write_prg(0xC000, 2, 0, 0, TraceConfig::none(), &mut sink);
        assert_eq!(
            mapper.read_prg(0xC000, 0, 0, TraceConfig::none(), &mut sink),
            2
        );
    }

    #[test]
    // Write IRQ-looking addresses on mapper 22 and clock a large chunk,
    // requiring the VRC2 IRQ gate to keep the pending flag clear.
    fn vrc2_never_asserts_mapper_irq() {
        let mut mapper = VrcFamilyMapper::new_with_submapper(
            22,
            0,
            marker_banks(8),
            marker_chr_1k(8),
            Mirroring::Horizontal,
        );
        let mut sink = TraceSink::default();
        for (addr, value) in [(0xF000, 0x0F), (0xF002, 0x0F), (0xF001, 0x06)] {
            mapper.write_prg(addr, value, 0, 0, TraceConfig::none(), &mut sink);
        }
        mapper.clock_cpu(100_000, 0, 0, TraceConfig::none(), &mut sink);
        assert!(!mapper.irq_pending());
    }

    #[test]
    // Check mapper 23/submapper 1 at the modeled two-cycle overflow and
    // 113-plus-one-cycle prescaler boundary using direct clock calls.
    fn vrc4_cycle_and_scanline_irq_modes_overflow_at_expected_boundaries() {
        let mut sink = TraceSink::default();
        let cfg = TraceConfig::none();
        let mut cycle = VrcFamilyMapper::new_with_submapper(
            23,
            1,
            marker_banks(8),
            marker_chr_1k(8),
            Mirroring::Horizontal,
        );
        cycle.write_prg(0xF000, 0x0E, 0, 0, cfg, &mut sink);
        cycle.write_prg(0xF001, 0x0F, 0, 0, cfg, &mut sink);
        cycle.write_prg(0xF002, 0x06, 0, 0, cfg, &mut sink);
        cycle.clock_cpu(1, 0, 0, cfg, &mut sink);
        assert!(!cycle.irq_pending());
        cycle.clock_cpu(1, 0, 0, cfg, &mut sink);
        assert!(cycle.irq_pending());

        let mut scanline = VrcFamilyMapper::new_with_submapper(
            23,
            1,
            marker_banks(8),
            marker_chr_1k(8),
            Mirroring::Horizontal,
        );
        scanline.write_prg(0xF000, 0x0F, 0, 0, cfg, &mut sink);
        scanline.write_prg(0xF001, 0x0F, 0, 0, cfg, &mut sink);
        scanline.write_prg(0xF002, 0x02, 0, 0, cfg, &mut sink);
        scanline.clock_cpu(113, 0, 0, cfg, &mut sink);
        assert!(!scanline.irq_pending());
        scanline.clock_cpu(1, 0, 0, cfg, &mut sink);
        assert!(scanline.irq_pending());
    }

    #[test]
    // Seed banks and pending IRQ, restore the V4 payload and compare its
    // bytes plus PRG/CHR summaries. This does not test post-restore execution
    // or state omitted from the serialized payload, such as mirroring.
    fn vrc4_versioned_snapshot_round_trips_mapper_state() {
        let mut mapper = VrcFamilyMapper::new_with_submapper(
            23,
            1,
            marker_banks(8),
            marker_chr_1k(8),
            Mirroring::Horizontal,
        );
        let mut sink = TraceSink::default();
        let cfg = TraceConfig::none();
        for (addr, value) in [(0x8000, 3), (0xA000, 4), (0xB000, 1), (0xB001, 2)] {
            mapper.write_prg(addr, value, 0, 0, cfg, &mut sink);
        }
        mapper.write_prg(0xF000, 0x0F, 0, 0, cfg, &mut sink);
        mapper.write_prg(0xF001, 0x0F, 0, 0, cfg, &mut sink);
        mapper.write_prg(0xF002, 0x06, 0, 0, cfg, &mut sink);
        mapper.clock_cpu(1, 0, 0, cfg, &mut sink);
        let snapshot = mapper.snapshot_bytes();
        assert_eq!(&snapshot[..2], b"V4");

        let mut restored = VrcFamilyMapper::new_with_submapper(
            23,
            1,
            marker_banks(8),
            marker_chr_1k(8),
            Mirroring::Horizontal,
        );
        restored.restore_snapshot_bytes(&snapshot).unwrap();
        assert_eq!(restored.snapshot_bytes(), snapshot);
        assert_eq!(
            restored.debug_state().prg_bank_window,
            mapper.debug_state().prg_bank_window
        );
        assert_eq!(
            restored.debug_state().chr_bank_window,
            mapper.debug_state().chr_bank_window
        );
    }
}
