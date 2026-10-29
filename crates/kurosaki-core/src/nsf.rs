//! NSF1/NSFe NTSC host. Format reference: https://www.nesdev.org/wiki/NSF
//! Executes INIT/PLAY on KUROSAKI's CPU, APU/DPCM and expansion sound cores.
//! No ROM conversion, external player, or GUI dependency is required.
use crate::{
    bus::Bus,
    cpu::CpuState,
    error::{KurosakiError, Result},
    mapper::{FdsMapper, Mapper, MapperDebugState, NromMapper},
    mapper_scaffolds::VrcFamilyMapper,
    namco163::Namco163Mapper,
    Mirroring, TraceConfig, TraceSink,
};
use serde::Serialize;
mod container;

const CPU_HZ: f64 = 1_789_773.0;
pub const MAX_NSF_BYTES: usize = 128 + 1024 * 1024 + 65536;
fn invalid(message: impl Into<String>) -> KurosakiError {
    KurosakiError::Nsf(message.into())
}

#[derive(Debug, Clone, Default, serde::Deserialize, Serialize)]
pub struct NsfTrack {
    pub name: String,
    pub time_ms: Option<u32>,
    pub fade_ms: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NsfHeader {
    pub format: &'static str,
    pub region: u8,
    pub tracks: Vec<NsfTrack>,
    pub title: String,
    pub artist: String,
    pub copyright: String,
    pub songs: u8,
    /// One-based, as in the file header and CLI.
    pub starting_song: u8,
    pub load_address: u16,
    pub init_address: u16,
    pub play_address: u16,
    pub ntsc_period_us: u16,
    pub banks: [u8; 8],
    pub expansion_bits: u8,
    pub expansion_chips: Vec<String>,
    pub banked: bool,
}
impl NsfHeader {
    /// One-based track number, including any declared fade.
    pub fn duration_ms(&self, song: u8) -> Option<u32> {
        let track = self.tracks.get(song.checked_sub(1)? as usize)?;
        track
            .time_ms
            .map(|ms| ms.saturating_add(track.fade_ms.unwrap_or(0)))
    }
}

#[derive(Debug, Clone)]
pub struct NsfFile {
    pub header: NsfHeader,
    data: Vec<u8>,
}

impl NsfFile {
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() > MAX_NSF_BYTES {
            return Err(invalid(
                "music file exceeds 1 MiB of program data plus metadata",
            ));
        }
        if data.get(..4) == Some(b"NSFE") {
            return container::from_nsfe(data);
        }
        if !(129..=MAX_NSF_BYTES).contains(&data.len()) || data.get(..5) != Some(b"NESM\x1a") {
            return Err(invalid(
                "expected an NSF1 header and 1 byte..1 MiB of music data",
            ));
        }
        if data[5] != 1 {
            return Err(invalid(
                "only NSF1/NSFe are supported; NSF2 execution features are not supported",
            ));
        }
        if data[122] & 3 == 1 {
            return Err(invalid("PAL-only NSF is not supported by the NTSC core"));
        }
        if data[123] & !0x17 != 0 {
            return Err(invalid(
                "unsupported expansion chip; supported: 2A03/DPCM, N163, VRC6, VRC7, FDS",
            ));
        }
        if data[124] != 0 {
            return Err(invalid("NSF2 execution flags are not supported"));
        }
        let declared =
            usize::from(data[125]) | usize::from(data[126]) << 8 | usize::from(data[127]) << 16;
        let end = if declared == 0 {
            data.len()
        } else {
            128 + declared
        };
        if end > data.len() || end <= 128 || end - 128 > 1024 * 1024 {
            return Err(invalid("invalid NSF program-data length"));
        }
        let word = |n| u16::from_le_bytes([data[n], data[n + 1]]);
        let string = |n| {
            String::from_utf8_lossy(&data[n..n + 32])
                .split('\0')
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let banks: [u8; 8] = data[112..120].try_into().unwrap();
        let banked = banks.iter().any(|v| *v != 0);
        let load = word(8);
        if data[6] == 0
            || data[7] == 0
            || data[7] > data[6]
            || load < 0x6000
            || word(10) < 0x6000
            || word(12) < 0x6000
        {
            return Err(invalid("invalid song count, starting song, or load/init/play address (expected $6000..$FFFF)"));
        }
        if !banked && load as usize + end - 128 > 65536 {
            return Err(invalid("unbanked music data extends past $FFFF"));
        }
        if banked && (load as usize & 4095) + end - 128 > 1024 * 1024 {
            return Err(invalid("padded music data exceeds 256 banks"));
        }
        if banked && load < 0x8000 && data[123] & 4 == 0 {
            return Err(invalid("banked non-FDS NSF must load at $8000 or above"));
        }
        let expansion_chips = [(1, "VRC6"), (2, "VRC7"), (4, "FDS"), (16, "N163")]
            .iter()
            .filter(|(bit, _)| data[123] & bit != 0)
            .map(|(_, name)| name.to_string())
            .collect();
        let mut file = Self {
            header: NsfHeader {
                format: "nsf-1",
                region: data[122],
                tracks: vec![NsfTrack::default(); data[6] as usize],
                title: string(14),
                artist: string(46),
                copyright: string(78),
                songs: data[6],
                starting_song: data[7],
                load_address: load,
                init_address: word(10),
                play_address: word(12),
                ntsc_period_us: if word(110) == 0 { 16639 } else { word(110) },
                banks,
                banked,
                expansion_bits: data[123],
                expansion_chips,
            },
            data: data[128..end].to_vec(),
        };
        if end < data.len() {
            container::read_metadata(&data[end..], &mut file.header)?;
        }
        Ok(file)
    }

    pub fn to_nsfe_bytes(&self) -> Result<Vec<u8>> {
        container::write_nsfe(self)
    }
    pub fn to_nsf_bytes(&self) -> Result<Vec<u8>> {
        container::write_nsf(self)
    }
}

struct NsfMapper {
    memory: Vec<u8>,
    image: Vec<u8>,
    banks: [u8; 10],
    banked: bool,
    fds: bool,
    chips: Vec<(u8, Box<dyn Mapper>)>,
}
impl NsfMapper {
    fn new(file: &NsfFile) -> Self {
        let h = &file.header;
        let mut memory = vec![0; 65536];
        let mut image = vec![0; h.load_address as usize & 4095];
        image.extend_from_slice(&file.data);
        image.resize(image.len().div_ceil(4096) * 4096, 0);
        if !h.banked {
            memory[h.load_address as usize..h.load_address as usize + file.data.len()]
                .copy_from_slice(&file.data);
        }
        let mut banks = [0; 10];
        banks[2..].copy_from_slice(&h.banks);
        banks[..2].copy_from_slice(&h.banks[6..]);
        let mut chips: Vec<(u8, Box<dyn Mapper>)> = Vec::new();
        for (bit, mapper) in [(1, 24), (2, 85)] {
            if h.expansion_bits & bit != 0 {
                chips.push((
                    bit,
                    Box::new(VrcFamilyMapper::new(
                        mapper,
                        vec![0; 32768],
                        vec![],
                        Mirroring::Horizontal,
                    )),
                ));
            }
        }
        if h.expansion_bits & 4 != 0 {
            chips.push((4, Box::new(FdsMapper::new(vec![0; 8192], vec![]))));
        }
        if h.expansion_bits & 16 != 0 {
            let mut chip =
                Namco163Mapper::new(vec![0; 32768], vec![], Mirroring::Horizontal, false);
            chip.use_averaged_audio();
            chips.push((16, Box::new(chip)));
        }
        Self {
            memory,
            image,
            banks,
            banked: h.banked,
            fds: h.expansion_bits & 4 != 0,
            chips,
        }
    }
    fn bank_offset(&self, addr: u16) -> Option<usize> {
        (self.banked && (addr >= 0x8000 || self.fds && addr >= 0x6000)).then(|| {
            self.banks[(addr as usize - 0x6000) / 4096] as usize * 4096 + (addr as usize & 4095)
        })
    }
    fn audio_address(bit: u8, addr: u16) -> bool {
        match bit {
            1 => matches!(addr, 0x9000..=0x9003 | 0xa000..=0xa002 | 0xb000..=0xb002),
            2 => matches!(addr, 0x9010 | 0x9030 | 0xe000),
            4 => matches!(addr, 0x4023 | 0x4040..=0x4092),
            16 => matches!(addr, 0x4800..=0x4fff | 0xe000..=0xe7ff | 0xf800..=0xffff),
            _ => false,
        }
    }
}
impl Mapper for NsfMapper {
    fn mapper_id(&self) -> u16 {
        0
    }
    fn mapper_name(&self) -> &'static str {
        "NSF host"
    }
    fn read_prg(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u8 {
        for (bit, chip) in &mut self.chips {
            if matches!((*bit, addr), (16, 0x4800..=0x4fff) | (4, 0x4040..=0x4092)) {
                return chip.read_prg(addr, frame, cycle, cfg, sink);
            }
        }
        match self.bank_offset(addr) {
            Some(offset) => self.image.get(offset).copied().unwrap_or(0),
            None => self.memory[addr as usize],
        }
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
        if self.banked
            && ((0x5ff8..=0x5fff).contains(&addr) || self.fds && (0x5ff6..=0x5ff7).contains(&addr))
        {
            self.banks[(addr - 0x5ff6) as usize] = value;
            return;
        }
        let mut audio_write = false;
        for (bit, chip) in &mut self.chips {
            if Self::audio_address(*bit, addr) {
                chip.write_prg(addr, value, frame, cycle, cfg, sink);
                audio_write = true;
            }
        }
        // Multi-chip register writes must not corrupt FDS code/data RAM.
        if !audio_write && ((0x6000..=0x7fff).contains(&addr) || self.fds && addr >= 0x8000) {
            if let Some(offset) = self.bank_offset(addr) {
                if let Some(v) = self.image.get_mut(offset) {
                    *v = value;
                }
            } else {
                self.memory[addr as usize] = value;
            }
        }
    }
    fn read_chr(&mut self, _: u16) -> u8 {
        0
    }
    fn write_chr(&mut self, _: u16, _: u8) {}
    fn clock_expansion_audio(&mut self, cycles: u64) {
        for (_, chip) in &mut self.chips {
            chip.clock_expansion_audio(cycles);
        }
    }
    fn expansion_audio_sample(&mut self, rate: u32) -> i16 {
        self.chips
            .iter_mut()
            .map(|(_, chip)| chip.expansion_audio_sample(rate) as i32)
            .sum::<i32>()
            .clamp(-32768, 32767) as i16
    }
    fn debug_state(&self) -> MapperDebugState {
        let mut state =
            NromMapper::new(vec![0; 32768], vec![], Mirroring::Horizontal).debug_state();
        state.name = "NSF host (no cartridge snapshots)".into();
        state
    }
    fn snapshot_bytes(&self) -> Vec<u8> {
        Vec::new()
    }
}

/// Stateful playback session. Recreate it to select a song and reset banks/RAM.
pub struct NsfPlayer {
    pub header: NsfHeader,
    pub song: u8,
    pub sample_rate: u32,
    pub play_calls: u64,
    pub trace: TraceSink,
    bus: Bus,
    cpu: CpuState,
    trace_config: TraceConfig,
    origin: u64,
    next_tick: u64,
}
impl NsfPlayer {
    pub fn new(
        file: &NsfFile,
        song: Option<u8>,
        sample_rate: u32,
        trace_config: TraceConfig,
    ) -> Result<Self> {
        let song = song.unwrap_or(file.header.starting_song);
        if song == 0 || song > file.header.songs {
            return Err(invalid("song must be in 1..=header.songs"));
        }
        if !(8000..=192000).contains(&sample_rate) {
            return Err(invalid("sample rate must be in 8000..=192000 Hz"));
        }
        let mut player = Self {
            header: file.header.clone(),
            song,
            sample_rate,
            play_calls: 0,
            trace: TraceSink::default(),
            bus: Bus::new(Box::new(NsfMapper::new(file))),
            cpu: CpuState::default(),
            trace_config,
            origin: 0,
            next_tick: 0,
        };
        player.bus.apu.sample_rate = sample_rate;
        player.bus.apu.capture_full_audio = true;
        for addr in 0x4000..=0x4013 {
            player
                .bus
                .write(addr, 0, 0, 0, trace_config, &mut player.trace);
        }
        for (addr, value) in [(0x4015, 0), (0x4015, 15), (0x4017, 0x40)] {
            player
                .bus
                .write(addr, value, 0, 0, trace_config, &mut player.trace);
        }
        player.cpu.a = song - 1; // NSF INIT uses zero-based song indices.
        player.cpu.x = 0; // NTSC
        player.call(file.header.init_address, CPU_HZ as u64 * 2)?;
        player.bus.apu.sample_buffer.clear();
        player.origin = player.cpu.cycles;
        Ok(player)
    }
    fn clock(&mut self, cycles: u64, start: u64) {
        let mut elapsed = 0;
        while elapsed < cycles {
            let step = (cycles - elapsed).min(16);
            self.bus.clock_audio_devices(
                step,
                self.play_calls,
                start + elapsed,
                self.trace_config,
                &mut self.trace,
            );
            elapsed += step;
        }
    }
    fn call(&mut self, addr: u16, budget: u64) -> Result<()> {
        self.cpu.pc = addr;
        self.cpu.sp = 0xfd;
        // Synthetic JSR return address; no host instructions occupy NSF memory.
        self.bus.ram[0x1fe] = 0xfe;
        self.bus.ram[0x1ff] = 0x4f;
        let start = self.cpu.cycles;
        while self.cpu.pc != 0x4fff {
            if self.cpu.cycles - start >= budget {
                return Err(invalid(format!("routine ${addr:04X} did not return within {budget} CPU cycles; non-returning INIT/PLAY is unsupported")));
            }
            let before = self.cpu.cycles;
            self.cpu.step(
                &mut self.bus,
                self.play_calls,
                self.trace_config,
                &mut self.trace,
            )?;
            self.clock(self.cpu.cycles - before, before);
            while self.bus.dma_stall_cycles != 0 {
                let stall = std::mem::take(&mut self.bus.dma_stall_cycles);
                self.clock(stall, self.cpu.cycles);
                self.cpu.cycles += stall;
            }
        }
        Ok(())
    }
    /// Render an exact number of mono PCM16 samples, preserving CPU/chip state
    /// between chunks. Caller controls real-time pacing and audio device output.
    pub fn render_samples(&mut self, count: usize) -> Result<Vec<i16>> {
        if count > self.sample_rate as usize * 600 {
            return Err(invalid("one render request is limited to 600 seconds"));
        }
        let interval = CPU_HZ * self.header.ntsc_period_us as f64 / 1_000_000.0;
        while self.bus.apu.sample_buffer.len() < count {
            let due = self.origin + (self.next_tick as f64 * interval).round() as u64;
            if self.cpu.cycles >= due {
                self.call(self.header.play_address, (interval as u64 * 4).max(100_000))?;
                self.play_calls += 1;
                self.next_tick += 1;
                // A long PLAY skips missed periods rather than accelerating
                // music by issuing a burst of overdue PLAY calls.
                let next = ((self.cpu.cycles - self.origin) as f64 / interval).ceil() as u64;
                self.next_tick = self.next_tick.max(next);
            } else {
                let missing = count - self.bus.apu.sample_buffer.len();
                let needed = (missing as f64 * CPU_HZ / self.sample_rate as f64).ceil() as u64;
                let cycles = (due - self.cpu.cycles).min(needed).clamp(1, 4096);
                self.clock(cycles, self.cpu.cycles);
                self.cpu.cycles += cycles;
                // DMA overlaps idle time; only execution needs CPU stalls.
                self.bus.dma_stall_cycles = 0;
            }
        }
        Ok(self.bus.apu.sample_buffer.drain(..count).collect())
    }
    pub fn cpu_cycles(&self) -> u64 {
        self.cpu.cycles
    }
    pub fn dmc_sample_fetches(&self) -> u64 {
        self.bus.apu.dmc_sample_fetches
    }
}
