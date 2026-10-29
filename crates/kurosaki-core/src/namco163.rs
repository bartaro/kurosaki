//! Namco 163 wavetable sound and mapper 19.
//!
//! Register/timing reference: https://www.nesdev.org/wiki/Namco_163_audio
//! One channel updates every 15 CPU cycles. The held multiplexed DAC output
//! is integrated over each host sample interval; phases live in shared RAM.
use crate::cart::Mirroring;
use crate::error::{KurosakiError, Result};
use crate::mapper::{
    bank_count, ensure_chr, mapper_debug_state, mirroring_name, physical_bank_8k, read_bank,
    trace_mapper_write, write_bank, Mapper, MapperDebugState, NametableMirroring,
};
use crate::trace::{TraceConfig, TraceEvent, TraceSink};

// Match the N163/APU balance used by Mesen's default digital mixer:
// N163 = DAC * 20, APU curves = response * 5_000 (both then scaled equally).
// Our APU curves use 45_000, so N163 must use 20 * 45_000 / 5_000 = 180.
// The former value of 40 attenuated N163 by 13.06 dB relative to the APU.
const N163_DAC_PCM_SCALE: i64 = (crate::apu::APU_PCM_SCALE * 20.0 / 5_000.0) as i64;

#[derive(Debug, Clone)]
pub struct Namco163Mapper {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    mirroring: Mirroring,
    battery: bool,
    submapper: u8,
    prg_regs: [u8; 3],
    chr_regs: [u8; 8],
    nt_regs: [u8; 4],
    chr_mode: u8,
    internal_ram: [u8; 128],
    ram_port: u8,
    ram_addr: u8,
    irq_counter: u16,
    irq_enabled: bool,
    irq_pending_flag: bool,
    audio_writes: u64,
    sound_disabled: bool,
    sound_divider: u8,
    sound_channel: u8,
    dac: i16,
    averaged_output: bool,
    channel_outputs: [i16; 8],
    sample_area: i64,
    sample_cycles: u64,
}

impl Namco163Mapper {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring, battery: bool) -> Self {
        Self::new_with_submapper(prg_rom, chr_rom, mirroring, battery, 0)
    }

    pub fn new_with_submapper(
        prg_rom: Vec<u8>,
        chr_rom: Vec<u8>,
        mirroring: Mirroring,
        battery: bool,
        submapper: u8,
    ) -> Self {
        let (chr, chr_ram) = ensure_chr(chr_rom);
        let nt_regs = match mirroring {
            Mirroring::Vertical => [0xe0, 0xe1, 0xe0, 0xe1],
            _ => [0xe0, 0xe0, 0xe1, 0xe1],
        };
        Self {
            prg_rom,
            prg_ram: vec![0; 8192],
            chr,
            chr_ram,
            mirroring,
            battery,
            submapper,
            prg_regs: [0, 1, 2],
            chr_regs: [0; 8],
            nt_regs,
            chr_mode: 0,
            internal_ram: [0; 128],
            ram_port: 0,
            ram_addr: 0,
            irq_counter: 0,
            irq_enabled: false,
            irq_pending_flag: false,
            audio_writes: 0,
            sound_disabled: false,
            sound_divider: 15,
            sound_channel: 7,
            dac: 0,
            averaged_output: false,
            channel_outputs: [0; 8],
            sample_area: 0,
            sample_cycles: 0,
        }
    }

    /// NSF editors/players such as Mesen mix the held outputs of all active
    /// voices. Use that presentation for NSF/preview to avoid DAC switching
    /// aliases; cartridge playback retains the physical multiplexed DAC.
    pub fn use_averaged_audio(&mut self) {
        self.averaged_output = true;
    }

    fn prg_windows(&self) -> [usize; 4] {
        let count = bank_count(self.prg_rom.len(), 8192);
        [
            self.prg_regs[0] as usize % count,
            self.prg_regs[1] as usize % count,
            self.prg_regs[2] as usize % count,
            count.saturating_sub(1),
        ]
    }

    fn increment_ram_address(&mut self) {
        // The 7-bit auto-increment address wraps into waveform RAM after $7f.
        if self.ram_port & 0x80 != 0 {
            self.ram_addr = self.ram_addr.wrapping_add(1) & 0x7f;
        }
    }

    fn audio_enabled(&self) -> bool {
        !self.sound_disabled && !matches!(self.submapper, 1 | 2)
    }

    fn update_channel(&mut self) {
        let first = 7 - ((self.internal_ram[0x7f] >> 4) & 7);
        if self.sound_channel < first {
            self.sound_channel = 7;
        }
        let base = 0x40 + self.sound_channel as usize * 8;
        let ram = &mut self.internal_ram;
        let phase =
            ram[base + 1] as u32 | (ram[base + 3] as u32) << 8 | (ram[base + 5] as u32) << 16;
        let frequency =
            ram[base] as u32 | (ram[base + 2] as u32) << 8 | ((ram[base + 4] & 3) as u32) << 16;
        let length = 256 - (ram[base + 4] & 0xfc) as u32;
        let phase = (phase + frequency) % (length << 16);
        ram[base + 1] = phase as u8;
        ram[base + 3] = (phase >> 8) as u8;
        ram[base + 5] = (phase >> 16) as u8;
        let position = ((phase >> 16) as u8).wrapping_add(ram[base + 6]);
        let sample = (ram[position as usize / 2] >> ((position & 1) * 4)) & 15;
        let output = (sample as i16 - 8) * (ram[base + 7] & 15) as i16;
        self.channel_outputs[self.sound_channel as usize] = output;
        self.dac = if self.averaged_output {
            self.channel_outputs[first as usize..].iter().sum::<i16>() / (8 - first) as i16
        } else {
            output
        };
        self.sound_channel = if self.sound_channel == first {
            7
        } else {
            self.sound_channel - 1
        };
    }
}

impl Mapper for Namco163Mapper {
    fn mapper_id(&self) -> u16 {
        19
    }
    fn mapper_name(&self) -> &'static str {
        "Namco 163"
    }
    fn physical_prg_bank_8k(&self, addr: u16) -> Option<u16> {
        if addr < 0x8000 {
            return None;
        }
        physical_bank_8k(
            self.prg_rom.len(),
            8192,
            self.prg_windows()[((addr - 0x8000) / 8192) as usize],
            addr as usize & 0x1fff,
        )
    }
    fn read_prg(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u8 {
        let value = match addr {
            0x4800..=0x4fff => {
                let v = self.internal_ram[self.ram_addr as usize];
                self.increment_ram_address();
                v
            }
            0x5000..=0x57ff => self.irq_counter as u8,
            0x5800..=0x5fff => {
                (self.irq_counter >> 8) as u8 | if self.irq_enabled { 0x80 } else { 0 }
            }
            0x6000..=0x7fff => self.prg_ram[addr as usize & 0x1fff],
            0x8000..=0xffff => read_bank(
                &self.prg_rom,
                8192,
                self.prg_windows()[((addr - 0x8000) / 8192) as usize],
                addr as usize & 0x1fff,
            ),
            _ => 0,
        };
        if cfg.mapper && (0x4800..=0x5fff).contains(&addr) {
            let mut event = TraceEvent::new("mapper.n163_read", frame, cycle);
            event.addr = Some(addr);
            event.value = Some(value);
            sink.push(event);
        }
        value
    }
    fn write_prg(
        &mut self,
        addr: u16,
        value: u8,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        let ram_address = self.ram_addr;
        match addr {
            0x4800..=0x4fff => {
                self.internal_ram[self.ram_addr as usize] = value;
                self.audio_writes = self.audio_writes.saturating_add(1);
                self.increment_ram_address();
            }
            0x5000..=0x57ff => {
                self.irq_counter = (self.irq_counter & 0x7f00) | value as u16;
                self.irq_pending_flag = false;
            }
            0x5800..=0x5fff => {
                self.irq_counter = (self.irq_counter & 0xff) | ((value as u16 & 0x7f) << 8);
                self.irq_enabled = value & 0x80 != 0;
                self.irq_pending_flag = false;
            }
            0x6000..=0x7fff => {
                let window = (addr - 0x6000) / 2048;
                if self.ram_port & 0xf0 == 0x40 && self.ram_port & (1 << window) == 0 {
                    self.prg_ram[addr as usize & 0x1fff] = value;
                }
            }
            0x8000..=0xbfff => self.chr_regs[((addr - 0x8000) / 2048) as usize] = value,
            0xc000..=0xdfff => self.nt_regs[((addr - 0xc000) / 2048) as usize] = value,
            0xe000..=0xe7ff => {
                self.prg_regs[0] = value & 0x3f;
                self.sound_disabled = value & 0x40 != 0;
            }
            0xe800..=0xefff => {
                self.prg_regs[1] = value & 0x3f;
                self.chr_mode = value & 0xc0;
            }
            0xf000..=0xf7ff => self.prg_regs[2] = value & 0x3f,
            0xf800..=0xffff => {
                self.ram_port = value;
                self.ram_addr = value & 0x7f;
            }
            _ => {}
        }
        if cfg.mapper {
            trace_mapper_write(
                if (0x4800..=0x4fff).contains(&addr) {
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
                    "Namco 163 RAM=${ram_address:02X}; PRG {:?}; IRQ={} enabled={} audio_writes={}",
                    self.prg_windows(),
                    self.irq_counter,
                    self.irq_enabled,
                    self.audio_writes
                ),
            );
        }
    }
    fn read_chr(&mut self, addr: u16) -> u8 {
        read_bank(
            &self.chr,
            1024,
            self.chr_regs[(addr as usize / 1024) & 7] as usize,
            addr as usize & 1023,
        )
    }
    fn write_chr(&mut self, addr: u16, value: u8) {
        if self.chr_ram {
            write_bank(
                &mut self.chr,
                1024,
                self.chr_regs[(addr as usize / 1024) & 7] as usize,
                addr as usize & 1023,
                value,
            );
        }
    }
    fn nametable_mirroring(&self) -> NametableMirroring {
        match self.nt_regs.map(|v| v & 1) {
            [0, 0, 0, 0] => NametableMirroring::SingleScreenLow,
            [1, 1, 1, 1] => NametableMirroring::SingleScreenHigh,
            [0, 1, 0, 1] => NametableMirroring::Vertical,
            [0, 0, 1, 1] => NametableMirroring::Horizontal,
            _ => NametableMirroring::FourScreen,
        }
    }
    fn clock_cpu(
        &mut self,
        cpu_cycles: u64,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        if self.irq_enabled {
            self.irq_counter = (self.irq_counter as u64)
                .saturating_add(cpu_cycles)
                .min(0x7fff) as u16;
            if self.irq_counter == 0x7fff {
                if !self.irq_pending_flag && (cfg.mapper || cfg.nmi) {
                    sink.push(TraceEvent::new("mapper.n163_irq", frame, cycle));
                }
                self.irq_pending_flag = true;
            }
        }
    }
    fn irq_pending(&self) -> bool {
        self.irq_pending_flag
    }
    fn clear_irq(&mut self) {
        self.irq_pending_flag = false;
    }
    fn clock_expansion_audio(&mut self, mut cycles: u64) {
        if !self.audio_enabled() {
            // Disabled sound freezes the sequencer and contributes silence.
            self.sample_cycles += cycles;
            return;
        }
        while cycles > 0 {
            let step = cycles.min(self.sound_divider as u64);
            self.sample_area += self.dac as i64 * step as i64;
            self.sample_cycles += step;
            self.sound_divider -= step as u8;
            cycles -= step;
            if self.sound_divider == 0 {
                self.update_channel();
                self.sound_divider = 15;
            }
        }
    }
    fn expansion_audio_sample(&mut self, _sample_rate: u32) -> i16 {
        // Digital balance relative to ApuState's 45_000 PCM curve scale.
        // Cartridge analog resistor/filter calibration and NES 2.0 volume
        // profiles remain approximate.
        let value = if self.sample_cycles > 0 {
            self.sample_area * N163_DAC_PCM_SCALE / self.sample_cycles as i64
        } else if self.audio_enabled() {
            self.dac as i64 * N163_DAC_PCM_SCALE
        } else {
            0
        };
        self.sample_area = 0;
        self.sample_cycles = 0;
        value as i16
    }
    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            19,
            format!(
                "Namco 163 channels={} audio_enabled={} audio_writes={} battery={}",
                1 + ((self.internal_ram[0x7f] >> 4) & 7),
                self.audio_enabled(),
                self.audio_writes,
                self.battery
            ),
            false,
            self.prg_windows().iter().map(|b| *b as u16).collect(),
            self.chr_regs.iter().map(|b| *b as u16).collect(),
            mirroring_name(self.mirroring),
            self.irq_pending(),
        )
    }
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut v = b"N163\x01".to_vec();
        v.extend_from_slice(&[
            self.ram_port,
            self.ram_addr,
            self.irq_enabled as u8,
            self.irq_pending_flag as u8,
        ]);
        v.extend_from_slice(&self.irq_counter.to_le_bytes());
        v.extend_from_slice(&self.audio_writes.to_le_bytes());
        v.extend_from_slice(&self.prg_regs);
        v.extend_from_slice(&self.chr_regs);
        v.extend_from_slice(&self.nt_regs);
        v.extend_from_slice(&[
            self.chr_mode,
            self.sound_disabled as u8,
            self.sound_divider,
            self.sound_channel,
        ]);
        v.extend_from_slice(&self.dac.to_le_bytes());
        v.extend_from_slice(&self.sample_area.to_le_bytes());
        v.extend_from_slice(&self.sample_cycles.to_le_bytes());
        v.extend_from_slice(&[self.submapper, self.battery as u8]);
        v.extend_from_slice(&self.internal_ram);
        v.extend_from_slice(&self.prg_ram);
        if self.chr_ram {
            v.extend_from_slice(&self.chr);
        }
        v
    }
    fn restore_snapshot_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        const HEADER: usize = 58;
        let expected =
            HEADER + 128 + self.prg_ram.len() + if self.chr_ram { self.chr.len() } else { 0 };
        let bad = || {
            KurosakiError::SnapshotFormat(
                "N163 snapshot version, configuration, or state mismatch".into(),
            )
        };
        if bytes.len() != expected || bytes.get(..5) != Some(b"N163\x01") {
            return Err(bad());
        }
        let area = i64::from_le_bytes(bytes[40..48].try_into().unwrap());
        let cycles = u64::from_le_bytes(bytes[48..56].try_into().unwrap());
        let dac = i16::from_le_bytes(bytes[38..40].try_into().unwrap());
        if bytes[6] > 127
            || bytes[7] > 1
            || bytes[8] > 1
            || bytes[10] > 127
            || bytes[19..22].iter().any(|v| *v > 63)
            || bytes[34] & 0x3f != 0
            || bytes[35] > 1
            || !(1..=15).contains(&bytes[36])
            || bytes[37] > 7
            || !(-120..=105).contains(&dac)
            || cycles > i64::MAX as u64 / 4800
            || area.unsigned_abs() > cycles.saturating_mul(120)
            || bytes[56] != self.submapper
            || bytes[57] != self.battery as u8
        {
            return Err(bad());
        }
        self.ram_port = bytes[5];
        self.ram_addr = bytes[6];
        self.irq_enabled = bytes[7] != 0;
        self.irq_pending_flag = bytes[8] != 0;
        self.irq_counter = u16::from_le_bytes(bytes[9..11].try_into().unwrap());
        self.audio_writes = u64::from_le_bytes(bytes[11..19].try_into().unwrap());
        self.prg_regs.copy_from_slice(&bytes[19..22]);
        self.chr_regs.copy_from_slice(&bytes[22..30]);
        self.nt_regs.copy_from_slice(&bytes[30..34]);
        self.chr_mode = bytes[34];
        self.sound_disabled = bytes[35] != 0;
        self.sound_divider = bytes[36];
        self.sound_channel = bytes[37];
        self.dac = dac;
        self.sample_area = area;
        self.sample_cycles = cycles;
        self.internal_ram
            .copy_from_slice(&bytes[HEADER..HEADER + 128]);
        let end = HEADER + 128 + self.prg_ram.len();
        self.prg_ram.copy_from_slice(&bytes[HEADER + 128..end]);
        if self.chr_ram {
            self.chr.copy_from_slice(&bytes[end..]);
        }
        Ok(())
    }
}
