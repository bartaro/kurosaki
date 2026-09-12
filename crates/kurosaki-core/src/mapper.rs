use crate::cart::{Cartridge, Mirroring};
use crate::error::{KurosakiError, Result};
use crate::fds::FdsDiskImage;
use crate::mapper_db::{mapper_spec, MapperFamily, MapperSpec, MapperSupportLevel};
use crate::mapper_scaffolds::{
    BandaiFcgMapper, BoardScaffoldMapper, JalecoSs88006Mapper, Mmc5Mapper, Namco163Mapper,
    Sunsoft5bMapper, VrcFamilyMapper,
};
use crate::trace::{TraceConfig, TraceEvent, TraceSink};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mmc1BoardVariant {
    GenericSxrom,
    Surom512,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Mmc1Config {
    pub board_variant: Mmc1BoardVariant,
    pub prg_ram_size: usize,
    pub chr_ram_size: usize,
    pub submapper: u8,
    pub battery: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NametableMirroring {
    Horizontal,
    Vertical,
    FourScreen,
    SingleScreenLow,
    SingleScreenHigh,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapperDebugState {
    pub mapper: u16,
    pub name: String,
    pub support: MapperSupportLevel,
    pub family: MapperFamily,
    pub probe_only: bool,
    pub prg_bank_window: Vec<u16>,
    pub chr_bank_window: Vec<u16>,
    pub mirroring: String,
    pub irq_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_profile_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outer_prg_bank: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner_prg_bank: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prg_ram_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmc1_shift: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmc1_control: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmc1_chr_bank0: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmc1_chr_bank1: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmc1_prg_bank: Option<u8>,
}

pub trait Mapper: Send {
    fn mapper_id(&self) -> u16;
    fn mapper_name(&self) -> &'static str;
    /// Returns the physical 8 KiB PRG-ROM bank currently visible at a CPU
    /// address. RAM, registers, firmware outside cartridge PRG-ROM, and
    /// unresolved probe mappings return `None`.
    fn physical_prg_bank_8k(&self, _addr: u16) -> Option<u16> {
        None
    }
    fn read_prg(
        &mut self,
        addr: u16,
        _frame: u64,
        _cycle: u64,
        _cfg: TraceConfig,
        _sink: &mut TraceSink,
    ) -> u8;
    fn write_prg(
        &mut self,
        addr: u16,
        value: u8,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    );
    fn read_chr(&mut self, addr: u16) -> u8;
    fn write_chr(&mut self, addr: u16, value: u8);
    fn notify_ppu_addr(
        &mut self,
        _addr: u16,
        _frame: u64,
        _cycle: u64,
        _cfg: TraceConfig,
        _sink: &mut TraceSink,
    ) {
    }
    fn clock_cpu(
        &mut self,
        _cpu_cycles: u64,
        _frame: u64,
        _cycle: u64,
        _cfg: TraceConfig,
        _sink: &mut TraceSink,
    ) {
    }
    fn irq_pending(&self) -> bool {
        false
    }
    fn clear_irq(&mut self) {}
    fn expansion_audio_sample(&mut self, _sample_rate: u32) -> i16 {
        0
    }
    fn nametable_mirroring(&self) -> NametableMirroring {
        NametableMirroring::FourScreen
    }
    fn debug_state(&self) -> MapperDebugState;
    fn snapshot_bytes(&self) -> Vec<u8>;
    /// Raw physical PRG RAM, not a CPU-window read or snapshot register blob.
    /// The cartridge layer separately validates whether this RAM is battery backed.
    fn battery_prg_ram(&self) -> Option<&[u8]> {
        None
    }
    fn battery_prg_ram_mut(&mut self) -> Option<&mut [u8]> {
        None
    }
    fn restore_snapshot_bytes(&mut self, _bytes: &[u8]) -> Result<()> {
        Err(KurosakiError::SnapshotFormat(format!(
            "mapper {} does not support restorable snapshots",
            self.mapper_id()
        )))
    }
    /// Restores mutable mapper state from a snapshot created for a compatible
    /// cartridge while retaining configuration owned by the loaded target
    /// cartridge. The default remains the strict same-configuration restore.
    fn restore_rebased_snapshot_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.restore_snapshot_bytes(bytes)
    }
    /// Applies an explicit patch to battery/work RAM represented by the
    /// mapper snapshot. Implementations must reject addresses outside their
    /// CPU-visible RAM window and must not alter mapper register state.
    fn patch_prg_ram_bytes(&mut self, _cpu_address: u16, _bytes: &[u8]) -> Result<()> {
        Err(KurosakiError::SnapshotFormat(format!(
            "mapper {} does not support snapshot PRG-RAM patches",
            self.mapper_id()
        )))
    }
}

pub fn create_mapper(cart: &Cartridge) -> Result<Box<dyn Mapper>> {
    let mapper = cart.info.mapper;
    match mapper {
        0 => Ok(Box::new(NromMapper::new(
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
        ))),
        1 => Ok(Box::new(Mmc1Mapper::new_with_config(
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
            Mmc1Config {
                board_variant: if cart.info.board_profile.as_deref() == Some("surom512") {
                    Mmc1BoardVariant::Surom512
                } else {
                    Mmc1BoardVariant::GenericSxrom
                },
                prg_ram_size: cart.info.prg_ram_size.unwrap_or(8 * 1024),
                chr_ram_size: cart.info.chr_ram_size.unwrap_or(8 * 1024),
                submapper: cart.info.submapper,
                battery: cart.info.battery,
            },
        ))),
        2 => Ok(Box::new(UxromMapper::new(
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
        ))),
        3 => Ok(Box::new(CnromMapper::new(
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
        ))),
        4 => Ok(Box::new(Mmc3Mapper::new(
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
            cart.info.battery,
        ))),
        118 | 119 => Ok(Box::new(Mmc3Mapper::new_variant(
            mapper,
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
            cart.info.battery,
        ))),
        5 => Ok(Box::new(Mmc5Mapper::new(
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
            cart.info.battery,
        ))),
        7 => Ok(Box::new(AxromMapper::new(
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
        ))),
        9 | 10 => Ok(Box::new(Mmc2Mmc4Mapper::new(
            mapper,
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
        ))),
        11 => Ok(Box::new(SimpleBankMapper::new(
            SimpleBankKind::ColorDreams,
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
        ))),
        13 => Ok(Box::new(SimpleBankMapper::new(
            SimpleBankKind::Cprom,
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
        ))),
        34 if cart.info.submapper == 0 => Ok(Box::new(SimpleBankMapper::new(
            SimpleBankKind::Bnrom,
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
        ))),
        66 => Ok(Box::new(SimpleBankMapper::new(
            SimpleBankKind::Gxrom,
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
        ))),
        79 => Ok(Box::new(SimpleBankMapper::new(
            SimpleBankKind::Nina03,
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
        ))),
        71 => Ok(Box::new(UxromMapper::new_variant(
            71,
            "Camerica/Codemasters",
            0,
            0x0F,
            false,
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
        ))),
        94 => Ok(Box::new(UxromMapper::new_variant(
            94,
            "UN1ROM",
            2,
            0x07,
            false,
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
        ))),
        180 => Ok(Box::new(UxromMapper::new_variant(
            180,
            "Crazy Climber",
            0,
            0x0F,
            true,
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
        ))),
        16 | 159 => Ok(Box::new(BandaiFcgMapper::new(
            mapper,
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
            cart.info.battery,
        ))),
        18 => Ok(Box::new(JalecoSs88006Mapper::new(
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
            cart.info.battery,
        ))),
        19 => Ok(Box::new(Namco163Mapper::new(
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
            cart.info.battery,
        ))),
        20 => {
            if let Some(disk) = &cart.fds_disk {
                Ok(Box::new(FdsMapper::new_with_disk(
                    cart.prg_rom.clone(),
                    disk.clone(),
                )))
            } else {
                Ok(Box::new(FdsMapper::new(
                    cart.prg_rom.clone(),
                    cart.chr_rom.clone(),
                )))
            }
        }
        73 => Ok(Box::new(Vrc3Mapper::new(
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
        ))),
        21 | 22 | 23 | 24 | 25 | 26 | 75 | 85 => Ok(Box::new(VrcFamilyMapper::new_with_submapper(
            mapper,
            cart.info.submapper,
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
        ))),
        32 | 33 | 48 | 64 | 68 | 76 | 87 | 206 | 210 => Ok(Box::new(BoardScaffoldMapper::new(
            mapper,
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
        ))),
        69 => Ok(Box::new(Sunsoft5bMapper::new(
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
            cart.info.battery,
        ))),
        _ => Ok(Box::new(GenericProbeMapper::new(
            mapper,
            cart.prg_rom.clone(),
            cart.chr_rom.clone(),
            cart.info.mirroring,
        ))),
    }
}

pub(crate) fn mapper_debug_state(
    mapper: u16,
    name: impl Into<String>,
    probe_only: bool,
    prg_bank_window: Vec<u16>,
    chr_bank_window: Vec<u16>,
    mirroring: String,
    irq_pending: bool,
) -> MapperDebugState {
    let spec = mapper_spec(mapper);
    MapperDebugState {
        mapper,
        name: name.into(),
        support: spec.support,
        family: spec.family,
        probe_only,
        prg_bank_window,
        chr_bank_window,
        mirroring,
        irq_pending,
        board_profile: None,
        board_profile_source: None,
        outer_prg_bank: None,
        inner_prg_bank: None,
        prg_ram_enabled: None,
        mmc1_shift: None,
        mmc1_control: None,
        mmc1_chr_bank0: None,
        mmc1_chr_bank1: None,
        mmc1_prg_bank: None,
    }
}

pub(crate) fn ensure_chr(chr_rom: Vec<u8>) -> (Vec<u8>, bool) {
    if chr_rom.is_empty() {
        (vec![0; 8 * 1024], true)
    } else {
        (chr_rom, false)
    }
}

pub(crate) fn bank_count(len: usize, bank_size: usize) -> usize {
    (len / bank_size).max(1)
}
pub(crate) fn wrap_bank(bank: usize, count: usize) -> usize {
    if count == 0 {
        0
    } else {
        bank % count
    }
}
pub(crate) fn read_bank(data: &[u8], bank_size: usize, bank: usize, offset: usize) -> u8 {
    if data.is_empty() {
        return 0;
    }
    let count = bank_count(data.len(), bank_size);
    let index = wrap_bank(bank, count) * bank_size + (offset % bank_size);
    data[index % data.len()]
}
pub(crate) fn physical_bank_8k(
    prg_rom_len: usize,
    bank_size: usize,
    bank: usize,
    offset: usize,
) -> Option<u16> {
    if prg_rom_len == 0 || bank_size < 8 * 1024 || !bank_size.is_multiple_of(8 * 1024) {
        return None;
    }
    let bank = wrap_bank(bank, bank_count(prg_rom_len, bank_size));
    let byte_offset = (bank * bank_size + offset % bank_size) % prg_rom_len;
    u16::try_from(byte_offset / (8 * 1024)).ok()
}
pub(crate) fn write_bank(data: &mut [u8], bank_size: usize, bank: usize, offset: usize, value: u8) {
    if data.is_empty() {
        return;
    }
    let count = bank_count(data.len(), bank_size);
    let index = wrap_bank(bank, count) * bank_size + (offset % bank_size);
    let len = data.len();
    data[index % len] = value;
}
pub(crate) fn mirroring_code(m: Mirroring) -> u8 {
    match m {
        Mirroring::Horizontal => 0,
        Mirroring::Vertical => 1,
        Mirroring::FourScreen => 2,
        Mirroring::MapperControlled => 3,
        Mirroring::Unknown => 4,
    }
}
pub(crate) fn mirroring_name(m: Mirroring) -> String {
    match m {
        Mirroring::Horizontal => "horizontal".to_string(),
        Mirroring::Vertical => "vertical".to_string(),
        Mirroring::FourScreen => "four_screen".to_string(),
        Mirroring::MapperControlled => "mapper_controlled".to_string(),
        Mirroring::Unknown => "unknown".to_string(),
    }
}

fn fixed_nametable_mirroring(m: Mirroring) -> NametableMirroring {
    match m {
        Mirroring::Horizontal => NametableMirroring::Horizontal,
        Mirroring::Vertical => NametableMirroring::Vertical,
        Mirroring::FourScreen => NametableMirroring::FourScreen,
        Mirroring::MapperControlled | Mirroring::Unknown => NametableMirroring::FourScreen,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn trace_mapper_write(
    kind: &str,
    addr: u16,
    value: u8,
    frame: u64,
    cycle: u64,
    cfg: TraceConfig,
    sink: &mut TraceSink,
    msg: impl Into<String>,
) {
    if cfg.mapper {
        let mut event = TraceEvent::new(kind, frame, cycle);
        event.addr = Some(addr);
        event.value = Some(value);
        event.message = Some(msg.into());
        sink.push(event);
    }
}

#[derive(Debug, Clone)]
pub struct NromMapper {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    mirroring: Mirroring,
}

impl NromMapper {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        let (chr, chr_ram) = ensure_chr(chr_rom);
        Self {
            prg_rom,
            prg_ram: vec![0; 8 * 1024],
            chr,
            chr_ram,
            mirroring,
        }
    }
}

impl Mapper for NromMapper {
    fn battery_prg_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }
    fn battery_prg_ram_mut(&mut self) -> Option<&mut [u8]> {
        Some(&mut self.prg_ram)
    }
    fn mapper_id(&self) -> u16 {
        0
    }
    fn mapper_name(&self) -> &'static str {
        "NROM"
    }
    fn physical_prg_bank_8k(&self, addr: u16) -> Option<u16> {
        if !(0x8000..=0xFFFF).contains(&addr) {
            return None;
        }
        let bank_size = if self.prg_rom.len() <= 16 * 1024 {
            16 * 1024
        } else {
            32 * 1024
        };
        physical_bank_8k(self.prg_rom.len(), bank_size, 0, addr as usize - 0x8000)
    }
    fn nametable_mirroring(&self) -> NametableMirroring {
        match self.mirroring {
            Mirroring::Horizontal => NametableMirroring::Horizontal,
            Mirroring::Vertical => NametableMirroring::Vertical,
            Mirroring::FourScreen => NametableMirroring::FourScreen,
            Mirroring::MapperControlled | Mirroring::Unknown => NametableMirroring::FourScreen,
        }
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
            0x6000..=0x7FFF => self.prg_ram[(addr as usize - 0x6000) % self.prg_ram.len()],
            0x8000..=0xFFFF => {
                let base = (addr as usize) - 0x8000;
                let mask = if self.prg_rom.len() <= 16 * 1024 {
                    0x3FFF
                } else {
                    0x7FFF
                };
                self.prg_rom[base & mask]
            }
            _ => 0,
        };
        if cfg.mapper {
            let mut event = TraceEvent::new("mapper.prg_read", frame, cycle);
            event.addr = Some(addr);
            event.value = Some(value);
            event.prg_bank = self.physical_prg_bank_8k(addr);
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
        match addr {
            0x6000..=0x7FFF => {
                let idx = (addr as usize - 0x6000) % self.prg_ram.len();
                self.prg_ram[idx] = value;
            }
            0x8000..=0xFFFF => trace_mapper_write(
                "mapper.prg_write_ignored",
                addr,
                value,
                frame,
                cycle,
                cfg,
                sink,
                "Write to NROM PRG-ROM area ignored",
            ),
            _ => {}
        }
    }

    fn read_chr(&mut self, addr: u16) -> u8 {
        self.chr[(addr as usize) % self.chr.len()]
    }
    fn write_chr(&mut self, addr: u16, value: u8) {
        if self.chr_ram {
            let len = self.chr.len();
            self.chr[(addr as usize) % len] = value;
        }
    }

    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            0,
            "NROM",
            false,
            if self.prg_rom.len() <= 16 * 1024 {
                vec![0, 0]
            } else {
                vec![0, 1]
            },
            vec![0],
            match self.nametable_mirroring() {
                NametableMirroring::Horizontal => "horizontal".to_string(),
                NametableMirroring::Vertical => "vertical".to_string(),
                NametableMirroring::FourScreen => "four_screen".to_string(),
                NametableMirroring::SingleScreenLow => "single_screen_low".to_string(),
                NametableMirroring::SingleScreenHigh => "single_screen_high".to_string(),
            },
            false,
        )
    }

    fn snapshot_bytes(&self) -> Vec<u8> {
        [self.prg_ram.as_slice(), self.chr.as_slice()].concat()
    }

    fn restore_snapshot_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let expected_len = self.prg_ram.len() + self.chr.len();
        if bytes.len() != expected_len {
            return Err(KurosakiError::SnapshotFormat(format!(
                "NROM private state length mismatch: expected {expected_len}, got {}",
                bytes.len()
            )));
        }
        let prg_end = self.prg_ram.len();
        self.prg_ram.copy_from_slice(&bytes[..prg_end]);
        self.chr.copy_from_slice(&bytes[prg_end..]);
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct UxromMapper {
    prg_rom: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    mirroring: Mirroring,
    mapper_id: u16,
    name: &'static str,
    bank_shift: u8,
    bank_mask: u8,
    fixed_lower: bool,
    bank_select: u8,
}

impl UxromMapper {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        Self::new_variant(
            2,
            "UNROM/UOROM",
            0,
            0x0F,
            false,
            prg_rom,
            chr_rom,
            mirroring,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_variant(
        mapper_id: u16,
        name: &'static str,
        bank_shift: u8,
        bank_mask: u8,
        fixed_lower: bool,
        prg_rom: Vec<u8>,
        chr_rom: Vec<u8>,
        mirroring: Mirroring,
    ) -> Self {
        let (chr, chr_ram) = ensure_chr(chr_rom);
        Self {
            prg_rom,
            chr,
            chr_ram,
            mirroring,
            mapper_id,
            name,
            bank_shift,
            bank_mask,
            fixed_lower,
            bank_select: 0,
        }
    }
}
impl Mapper for UxromMapper {
    fn mapper_id(&self) -> u16 {
        self.mapper_id
    }
    fn mapper_name(&self) -> &'static str {
        self.name
    }
    fn physical_prg_bank_8k(&self, addr: u16) -> Option<u16> {
        let (bank, offset) = match addr {
            0x8000..=0xBFFF => (
                if self.fixed_lower {
                    0
                } else {
                    self.bank_select as usize
                },
                addr as usize - 0x8000,
            ),
            0xC000..=0xFFFF => (
                if self.fixed_lower {
                    self.bank_select as usize
                } else {
                    bank_count(self.prg_rom.len(), 16 * 1024).saturating_sub(1)
                },
                addr as usize - 0xC000,
            ),
            _ => return None,
        };
        physical_bank_8k(self.prg_rom.len(), 16 * 1024, bank, offset)
    }
    fn nametable_mirroring(&self) -> NametableMirroring {
        fixed_nametable_mirroring(self.mirroring)
    }
    fn read_prg(
        &mut self,
        addr: u16,
        _frame: u64,
        _cycle: u64,
        _cfg: TraceConfig,
        _sink: &mut TraceSink,
    ) -> u8 {
        match addr {
            0x8000..=0xBFFF => read_bank(
                &self.prg_rom,
                16 * 1024,
                if self.fixed_lower {
                    0
                } else {
                    self.bank_select as usize
                },
                addr as usize - 0x8000,
            ),
            0xC000..=0xFFFF => read_bank(
                &self.prg_rom,
                16 * 1024,
                if self.fixed_lower {
                    self.bank_select as usize
                } else {
                    bank_count(self.prg_rom.len(), 16 * 1024) - 1
                },
                addr as usize - 0xC000,
            ),
            _ => 0,
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
        if addr >= 0x8000 {
            self.bank_select = (value >> self.bank_shift) & self.bank_mask;
            trace_mapper_write(
                "mapper.bank_switch",
                addr,
                value,
                frame,
                cycle,
                cfg,
                sink,
                format!("{} selected 16K PRG bank {}", self.name, self.bank_select),
            );
        }
    }
    fn read_chr(&mut self, addr: u16) -> u8 {
        self.chr[(addr as usize) % self.chr.len()]
    }
    fn write_chr(&mut self, addr: u16, value: u8) {
        if self.chr_ram {
            let len = self.chr.len();
            self.chr[(addr as usize) % len] = value;
        }
    }
    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            self.mapper_id,
            self.mapper_name(),
            false,
            vec![
                if self.fixed_lower {
                    0
                } else {
                    self.bank_select as u16
                },
                if self.fixed_lower {
                    self.bank_select as u16
                } else {
                    (bank_count(self.prg_rom.len(), 16 * 1024) - 1) as u16
                },
            ],
            vec![0],
            mirroring_name(self.mirroring),
            false,
        )
    }
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut v = vec![self.bank_select];
        v.extend_from_slice(&self.chr);
        v
    }

    fn restore_snapshot_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let expected_len = 1 + self.chr.len();
        if bytes.len() != expected_len {
            return Err(KurosakiError::SnapshotFormat(format!(
                "{} private state length mismatch: expected {expected_len}, got {}",
                self.name,
                bytes.len()
            )));
        }
        self.bank_select = bytes[0];
        self.chr.copy_from_slice(&bytes[1..]);
        Ok(())
    }
}

/// Konami VRC3 (mapper 73): a 16 KiB PRG board with a programmable IRQ counter.
/// The register layout is distinct from VRC2/4/6/7, so it intentionally does
/// not share their address-line decoder.
#[derive(Debug, Clone)]
pub struct Vrc3Mapper {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    mirroring: Mirroring,
    prg_bank: u8,
    irq_latch: u16,
    irq_counter: u16,
    irq_control: u8,
    irq_enabled: bool,
    irq_pending_flag: bool,
}

impl Vrc3Mapper {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        let (chr, chr_ram) = ensure_chr(chr_rom);
        Self {
            prg_rom,
            prg_ram: vec![0; 8 * 1024],
            chr,
            chr_ram,
            mirroring,
            prg_bank: 0,
            irq_latch: 0,
            irq_counter: 0,
            irq_control: 0,
            irq_enabled: false,
            irq_pending_flag: false,
        }
    }

    fn irq_limit(&self) -> u32 {
        if self.irq_control & 0x04 != 0 {
            0x100
        } else {
            0x1_0000
        }
    }

    fn write_irq_register(&mut self, addr: u16, value: u8) {
        match addr & 0xF000 {
            0x8000 => self.irq_latch = (self.irq_latch & 0xFFF0) | (value as u16 & 0x000F),
            0x9000 => self.irq_latch = (self.irq_latch & 0xFF0F) | ((value as u16 & 0x000F) << 4),
            0xA000 => self.irq_latch = (self.irq_latch & 0xF0FF) | ((value as u16 & 0x000F) << 8),
            0xB000 => self.irq_latch = (self.irq_latch & 0x0FFF) | ((value as u16 & 0x000F) << 12),
            0xC000 => {
                self.irq_control = value;
                self.irq_enabled = value & 0x02 != 0;
                self.irq_pending_flag = false;
                if self.irq_enabled {
                    self.irq_counter = self.irq_latch;
                }
            }
            0xD000 => {
                self.irq_pending_flag = false;
                self.irq_enabled = self.irq_control & 0x01 != 0;
            }
            _ => {}
        }
    }
}

impl Mapper for Vrc3Mapper {
    fn mapper_id(&self) -> u16 {
        73
    }
    fn mapper_name(&self) -> &'static str {
        "Konami VRC3"
    }
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
    fn nametable_mirroring(&self) -> NametableMirroring {
        fixed_nametable_mirroring(self.mirroring)
    }
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
                bank_count(self.prg_rom.len(), 16 * 1024) - 1,
                addr as usize - 0xC000,
            ),
            _ => 0,
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
        match addr {
            0x6000..=0x7FFF => {
                let index = (addr as usize - 0x6000) % self.prg_ram.len();
                self.prg_ram[index] = value;
            }
            0x8000..=0xDFFF => self.write_irq_register(addr, value),
            0xF000..=0xFFFF => self.prg_bank = value & 0x0F,
            _ => {}
        }
        if addr >= 0x8000 {
            trace_mapper_write(
                "mapper.vrc3_write",
                addr,
                value,
                frame,
                cycle,
                cfg,
                sink,
                format!(
                    "VRC3 write; PRG={} IRQ latch={:04X} counter={:04X} enabled={}",
                    self.prg_bank, self.irq_latch, self.irq_counter, self.irq_enabled
                ),
            );
        }
    }
    fn read_chr(&mut self, addr: u16) -> u8 {
        self.chr[addr as usize % self.chr.len()]
    }
    fn write_chr(&mut self, addr: u16, value: u8) {
        if self.chr_ram {
            let len = self.chr.len();
            self.chr[addr as usize % len] = value;
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
        if !self.irq_enabled {
            return;
        }
        let limit = self.irq_limit();
        let next = self.irq_counter as u32 + cpu_cycles as u32;
        if next >= limit {
            self.irq_counter = (next % limit) as u16;
            self.irq_pending_flag = true;
            if cfg.mapper || cfg.nmi {
                let mut event = TraceEvent::new("mapper.vrc3_irq", frame, cycle);
                event.message = Some("VRC3 IRQ pending after counter overflow".to_string());
                sink.push(event);
            }
        } else {
            self.irq_counter = next as u16;
        }
    }
    fn irq_pending(&self) -> bool {
        self.irq_pending_flag
    }
    fn clear_irq(&mut self) {
        self.irq_pending_flag = false;
    }
    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            73,
            format!(
                "VRC3 irq_latch={:04X} irq_counter={:04X}",
                self.irq_latch, self.irq_counter
            ),
            false,
            vec![
                self.prg_bank as u16,
                (bank_count(self.prg_rom.len(), 16 * 1024) - 1) as u16,
            ],
            vec![0],
            mirroring_name(self.mirroring),
            self.irq_pending(),
        )
    }
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![
            self.prg_bank,
            self.irq_control,
            self.irq_enabled as u8,
            self.irq_pending_flag as u8,
        ];
        bytes.extend_from_slice(&self.irq_latch.to_le_bytes());
        bytes.extend_from_slice(&self.irq_counter.to_le_bytes());
        bytes.extend_from_slice(&self.prg_ram);
        bytes.extend_from_slice(&self.chr);
        bytes
    }
    fn restore_snapshot_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let expected = 8 + self.prg_ram.len() + self.chr.len();
        if bytes.len() != expected {
            return Err(KurosakiError::SnapshotFormat(format!(
                "VRC3 private state length mismatch: expected {expected}, got {}",
                bytes.len()
            )));
        }
        self.prg_bank = bytes[0];
        self.irq_control = bytes[1];
        self.irq_enabled = bytes[2] != 0;
        self.irq_pending_flag = bytes[3] != 0;
        self.irq_latch = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        self.irq_counter = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
        let prg_end = 8 + self.prg_ram.len();
        self.prg_ram.copy_from_slice(&bytes[8..prg_end]);
        self.chr.copy_from_slice(&bytes[prg_end..]);
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct CnromMapper {
    prg_rom: Vec<u8>,
    chr: Vec<u8>,
    mirroring: Mirroring,
    chr_bank: u8,
}
impl CnromMapper {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        let (chr, _) = ensure_chr(chr_rom);
        Self {
            prg_rom,
            chr,
            mirroring,
            chr_bank: 0,
        }
    }
}
impl Mapper for CnromMapper {
    fn mapper_id(&self) -> u16 {
        3
    }
    fn mapper_name(&self) -> &'static str {
        "CNROM"
    }
    fn physical_prg_bank_8k(&self, addr: u16) -> Option<u16> {
        if !(0x8000..=0xFFFF).contains(&addr) {
            return None;
        }
        let bank_size = if self.prg_rom.len() <= 16 * 1024 {
            16 * 1024
        } else {
            32 * 1024
        };
        physical_bank_8k(self.prg_rom.len(), bank_size, 0, addr as usize - 0x8000)
    }
    fn nametable_mirroring(&self) -> NametableMirroring {
        fixed_nametable_mirroring(self.mirroring)
    }
    fn read_prg(
        &mut self,
        addr: u16,
        _frame: u64,
        _cycle: u64,
        _cfg: TraceConfig,
        _sink: &mut TraceSink,
    ) -> u8 {
        if addr >= 0x8000 {
            let mask = if self.prg_rom.len() <= 16 * 1024 {
                0x3FFF
            } else {
                0x7FFF
            };
            self.prg_rom[(addr as usize - 0x8000) & mask]
        } else {
            0
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
        if addr >= 0x8000 {
            self.chr_bank = value & 0x03;
            trace_mapper_write(
                "mapper.chr_bank_switch",
                addr,
                value,
                frame,
                cycle,
                cfg,
                sink,
                format!("CNROM selected 8K CHR bank {}", self.chr_bank),
            );
        }
    }
    fn read_chr(&mut self, addr: u16) -> u8 {
        read_bank(&self.chr, 8 * 1024, self.chr_bank as usize, addr as usize)
    }
    fn write_chr(&mut self, _addr: u16, _value: u8) {}
    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            3,
            "CNROM",
            false,
            if self.prg_rom.len() <= 16 * 1024 {
                vec![0, 0]
            } else {
                vec![0, 1]
            },
            vec![self.chr_bank as u16],
            mirroring_name(self.mirroring),
            false,
        )
    }
    fn snapshot_bytes(&self) -> Vec<u8> {
        vec![self.chr_bank]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SimpleBankKind {
    ColorDreams,
    Bnrom,
    Gxrom,
    Cprom,
    Nina03,
}

#[derive(Debug, Clone)]
pub struct SimpleBankMapper {
    kind: SimpleBankKind,
    prg_rom: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    mirroring: Mirroring,
    prg_bank: u8,
    chr_bank: u8,
}

impl SimpleBankMapper {
    fn new(kind: SimpleBankKind, prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        let (mut chr, chr_ram) = ensure_chr(chr_rom);
        if kind == SimpleBankKind::Cprom && chr_ram {
            chr.resize(16 * 1024, 0);
        }
        Self {
            kind,
            prg_rom,
            chr,
            chr_ram,
            mirroring,
            prg_bank: 0,
            chr_bank: 0,
        }
    }

    fn mapper_id(&self) -> u16 {
        match self.kind {
            SimpleBankKind::ColorDreams => 11,
            SimpleBankKind::Bnrom => 34,
            SimpleBankKind::Gxrom => 66,
            SimpleBankKind::Cprom => 13,
            SimpleBankKind::Nina03 => 79,
        }
    }

    fn name(&self) -> &'static str {
        match self.kind {
            SimpleBankKind::ColorDreams => "Color Dreams",
            SimpleBankKind::Bnrom => "BNROM",
            SimpleBankKind::Gxrom => "GxROM/MHROM",
            SimpleBankKind::Cprom => "CPROM",
            SimpleBankKind::Nina03 => "NINA-03/NINA-06",
        }
    }

    fn prg_bank_size(&self) -> usize {
        match self.kind {
            SimpleBankKind::ColorDreams
            | SimpleBankKind::Gxrom
            | SimpleBankKind::Bnrom
            | SimpleBankKind::Cprom
            | SimpleBankKind::Nina03 => 32 * 1024,
        }
    }
}

impl Mapper for SimpleBankMapper {
    fn mapper_id(&self) -> u16 {
        self.mapper_id()
    }

    fn mapper_name(&self) -> &'static str {
        self.name()
    }

    fn physical_prg_bank_8k(&self, addr: u16) -> Option<u16> {
        if !(0x8000..=0xFFFF).contains(&addr) {
            return None;
        }
        physical_bank_8k(
            self.prg_rom.len(),
            self.prg_bank_size(),
            self.prg_bank as usize,
            addr as usize - 0x8000,
        )
    }

    fn nametable_mirroring(&self) -> NametableMirroring {
        fixed_nametable_mirroring(self.mirroring)
    }

    fn read_prg(
        &mut self,
        addr: u16,
        _frame: u64,
        _cycle: u64,
        _cfg: TraceConfig,
        _sink: &mut TraceSink,
    ) -> u8 {
        if addr < 0x8000 {
            return 0;
        }
        read_bank(
            &self.prg_rom,
            self.prg_bank_size(),
            self.prg_bank as usize,
            addr as usize - 0x8000,
        )
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
        if addr < 0x8000
            && !(self.kind == SimpleBankKind::Nina03 && (0x4100..=0x5FFF).contains(&addr))
        {
            return;
        }
        match self.kind {
            SimpleBankKind::ColorDreams => {
                self.prg_bank = (value >> 4) & 0x03;
                self.chr_bank = value & 0x0F;
            }
            SimpleBankKind::Bnrom => {
                self.prg_bank = value & 0x0F;
            }
            SimpleBankKind::Gxrom => {
                self.prg_bank = (value >> 4) & 0x03;
                self.chr_bank = value & 0x03;
            }
            SimpleBankKind::Cprom => {
                self.prg_bank = 0;
                self.chr_bank = value & 0x01;
            }
            SimpleBankKind::Nina03 => {
                self.prg_bank = (value >> 3) & 0x03;
                self.chr_bank = value & 0x07;
            }
        }
        trace_mapper_write(
            "mapper.simple_bank_switch",
            addr,
            value,
            frame,
            cycle,
            cfg,
            sink,
            format!(
                "{} selected PRG bank {} and CHR bank {}",
                self.name(),
                self.prg_bank,
                self.chr_bank
            ),
        );
    }

    fn read_chr(&mut self, addr: u16) -> u8 {
        let chr_bank = match self.kind {
            SimpleBankKind::Bnrom | SimpleBankKind::Cprom => 0,
            SimpleBankKind::ColorDreams | SimpleBankKind::Gxrom | SimpleBankKind::Nina03 => {
                self.chr_bank as usize
            }
        };
        if self.kind == SimpleBankKind::Cprom {
            let bank = if addr < 0x1000 {
                0
            } else {
                self.chr_bank as usize + 1
            };
            read_bank(&self.chr, 4 * 1024, bank, addr as usize & 0x0FFF)
        } else {
            read_bank(&self.chr, 8 * 1024, chr_bank, addr as usize)
        }
    }

    fn write_chr(&mut self, addr: u16, value: u8) {
        if !self.chr_ram {
            return;
        }
        let chr_bank = match self.kind {
            SimpleBankKind::Bnrom | SimpleBankKind::Cprom => 0,
            SimpleBankKind::ColorDreams | SimpleBankKind::Gxrom | SimpleBankKind::Nina03 => {
                self.chr_bank as usize
            }
        };
        if self.kind == SimpleBankKind::Cprom {
            let bank = if addr < 0x1000 {
                0
            } else {
                self.chr_bank as usize + 1
            };
            write_bank(&mut self.chr, 4 * 1024, bank, addr as usize & 0x0FFF, value);
        } else {
            write_bank(&mut self.chr, 8 * 1024, chr_bank, addr as usize, value);
        }
    }

    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            self.mapper_id(),
            self.name(),
            false,
            vec![self.prg_bank as u16],
            vec![self.chr_bank as u16],
            mirroring_name(self.mirroring),
            false,
        )
    }

    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![
            self.prg_bank,
            self.chr_bank,
            self.kind as u8,
            mirroring_code(self.mirroring),
        ];
        bytes.extend_from_slice(&self.chr);
        bytes
    }

    fn restore_snapshot_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let header_len = 4;
        let expected_len = header_len + self.chr.len();
        if bytes.len() != expected_len {
            return Err(KurosakiError::SnapshotFormat(format!(
                "simple mapper private state length mismatch: expected {expected_len}, got {}",
                bytes.len()
            )));
        }
        if bytes[2] != self.kind as u8 || bytes[3] != mirroring_code(self.mirroring) {
            return Err(KurosakiError::SnapshotFormat(
                "simple mapper configuration does not match snapshot".to_string(),
            ));
        }
        self.prg_bank = bytes[0];
        self.chr_bank = bytes[1];
        self.chr.copy_from_slice(&bytes[header_len..]);
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct AxromMapper {
    prg_rom: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    bank_select: u8,
    single_screen_high: bool,
}
impl AxromMapper {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>) -> Self {
        let (chr, chr_ram) = ensure_chr(chr_rom);
        Self {
            prg_rom,
            chr,
            chr_ram,
            bank_select: 0,
            single_screen_high: false,
        }
    }
}
impl Mapper for AxromMapper {
    fn mapper_id(&self) -> u16 {
        7
    }
    fn mapper_name(&self) -> &'static str {
        "AxROM"
    }
    fn physical_prg_bank_8k(&self, addr: u16) -> Option<u16> {
        if !(0x8000..=0xFFFF).contains(&addr) {
            return None;
        }
        physical_bank_8k(
            self.prg_rom.len(),
            32 * 1024,
            self.bank_select as usize,
            addr as usize - 0x8000,
        )
    }
    fn read_prg(
        &mut self,
        addr: u16,
        _frame: u64,
        _cycle: u64,
        _cfg: TraceConfig,
        _sink: &mut TraceSink,
    ) -> u8 {
        if addr >= 0x8000 {
            read_bank(
                &self.prg_rom,
                32 * 1024,
                self.bank_select as usize,
                addr as usize - 0x8000,
            )
        } else {
            0
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
        if addr >= 0x8000 {
            self.bank_select = value & 0x07;
            self.single_screen_high = value & 0x10 != 0;
            trace_mapper_write(
                "mapper.bank_switch",
                addr,
                value,
                frame,
                cycle,
                cfg,
                sink,
                format!(
                    "AxROM selected 32K PRG bank {}, mirroring high={}",
                    self.bank_select, self.single_screen_high
                ),
            );
        }
    }
    fn read_chr(&mut self, addr: u16) -> u8 {
        self.chr[(addr as usize) % self.chr.len()]
    }
    fn write_chr(&mut self, addr: u16, value: u8) {
        if self.chr_ram {
            let len = self.chr.len();
            self.chr[(addr as usize) % len] = value;
        }
    }
    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            7,
            "AxROM",
            false,
            vec![self.bank_select as u16],
            vec![0],
            if self.single_screen_high {
                "single_screen_high"
            } else {
                "single_screen_low"
            }
            .to_string(),
            false,
        )
    }
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut v = vec![self.bank_select, self.single_screen_high as u8];
        v.extend_from_slice(&self.chr);
        v
    }
}

#[derive(Debug, Clone)]
pub struct Mmc1Mapper {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    shift: u8,
    control: u8,
    chr_bank0: u8,
    chr_bank1: u8,
    prg_bank: u8,
    mirroring: Mirroring,
    battery: bool,
    board_variant: Mmc1BoardVariant,
    submapper: u8,
    prg_ram_enabled: bool,
    last_mapper_write_cycle: Option<u64>,
    last_ppu_addr: u16,
    chr_mode_mismatch_reported: bool,
}

impl Mmc1Mapper {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring, battery: bool) -> Self {
        Self::new_with_config(
            prg_rom,
            chr_rom,
            mirroring,
            Mmc1Config {
                board_variant: Mmc1BoardVariant::GenericSxrom,
                prg_ram_size: 8 * 1024,
                chr_ram_size: 8 * 1024,
                submapper: 0,
                battery,
            },
        )
    }

    pub fn new_with_config(
        prg_rom: Vec<u8>,
        chr_rom: Vec<u8>,
        mirroring: Mirroring,
        config: Mmc1Config,
    ) -> Self {
        let (mut chr, chr_ram) = ensure_chr(chr_rom);
        if chr_ram && config.chr_ram_size > 0 {
            chr.resize(config.chr_ram_size, 0);
        }
        Self {
            prg_rom,
            prg_ram: vec![0; config.prg_ram_size.max(1)],
            chr,
            chr_ram,
            shift: 0x10,
            control: 0x0C,
            chr_bank0: 0,
            chr_bank1: 0,
            prg_bank: 0,
            mirroring,
            battery: config.battery,
            board_variant: config.board_variant,
            submapper: config.submapper,
            prg_ram_enabled: true,
            last_mapper_write_cycle: None,
            last_ppu_addr: 0,
            chr_mode_mismatch_reported: false,
        }
    }

    fn commit(&mut self, addr: u16, data: u8) -> &'static str {
        match addr {
            0x8000..=0x9FFF => {
                self.control = data & 0x1f;
                self.chr_mode_mismatch_reported = false;
                "control"
            }
            0xA000..=0xBFFF => {
                self.chr_bank0 = data & 0x1f;
                self.chr_mode_mismatch_reported = false;
                "chr_bank_0"
            }
            0xC000..=0xDFFF => {
                self.chr_bank1 = data & 0x1f;
                self.chr_mode_mismatch_reported = false;
                "chr_bank_1"
            }
            0xE000..=0xFFFF => {
                self.prg_bank = data & 0x1f;
                self.prg_ram_enabled = self.prg_bank & 0x10 == 0;
                "prg_bank"
            }
            _ => "unknown",
        }
    }

    fn board_profile(&self) -> &'static str {
        match self.board_variant {
            Mmc1BoardVariant::GenericSxrom => "generic_sxrom",
            Mmc1BoardVariant::Surom512 => "surom512",
        }
    }

    fn outer_prg_bank(&self) -> usize {
        if self.board_variant != Mmc1BoardVariant::Surom512 {
            return 0;
        }
        ((self.chr_bank0 >> 4) & 1) as usize
    }

    fn chr_outer_mismatch(&self) -> bool {
        self.board_variant == Mmc1BoardVariant::Surom512
            && self.control & 0x10 != 0
            && ((self.chr_bank0 ^ self.chr_bank1) & 0x10) != 0
    }

    fn prg_window(&self) -> (usize, usize) {
        let count16 = bank_count(self.prg_rom.len(), 16 * 1024);
        if self.board_variant == Mmc1BoardVariant::Surom512 {
            let outer_base = self.outer_prg_bank() * 16;
            let inner = (self.prg_bank & 0x0f) as usize;
            return match (self.control >> 2) & 0x03 {
                0 | 1 => {
                    let pair = inner & !1;
                    (outer_base + pair, outer_base + pair + 1)
                }
                2 => (outer_base, outer_base + inner),
                _ => (outer_base + inner, outer_base + 15),
            };
        }
        match (self.control >> 2) & 0x03 {
            0 | 1 => {
                let b = (self.prg_bank as usize & 0x0e) % count16;
                (b, (b + 1) % count16)
            }
            2 => (0, (self.prg_bank as usize & 0x0f) % count16),
            _ => ((self.prg_bank as usize & 0x0f) % count16, count16 - 1),
        }
    }
    fn chr_window(&self) -> Vec<u16> {
        if self.control & 0x10 == 0 {
            vec![(self.chr_bank0 & !1) as u16, (self.chr_bank0 | 1) as u16]
        } else {
            vec![self.chr_bank0 as u16, self.chr_bank1 as u16]
        }
    }
    fn effective_mirroring(&self) -> String {
        match self.control & 0x03 {
            0 => "single_screen_low".to_string(),
            1 => "single_screen_high".to_string(),
            2 => "vertical".to_string(),
            _ => "horizontal".to_string(),
        }
    }

    fn trace_details(&self, register: Option<&str>) -> BTreeMap<String, Value> {
        let (lo, hi) = self.prg_window();
        let mut details = BTreeMap::new();
        details.insert("board_profile".to_string(), json!(self.board_profile()));
        if let Some(register) = register {
            details.insert("register".to_string(), json!(register));
        }
        details.insert("outer_256k".to_string(), json!(self.outer_prg_bank()));
        details.insert("inner_16k".to_string(), json!(self.prg_bank & 0x0f));
        details.insert("prg_low_physical".to_string(), json!(lo));
        details.insert("prg_high_physical".to_string(), json!(hi));
        details.insert("prg_ram_enabled".to_string(), json!(self.prg_ram_enabled));
        details.insert("control".to_string(), json!(self.control));
        details.insert("prg_mode".to_string(), json!((self.control >> 2) & 3));
        details.insert("chr_mode".to_string(), json!((self.control >> 4) & 1));
        details
    }

    #[allow(clippy::too_many_arguments)]
    fn trace_event(
        &self,
        kind: &str,
        addr: u16,
        value: u8,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        register: Option<&str>,
        message: impl Into<String>,
    ) {
        if !cfg.mapper {
            return;
        }
        let mut event = TraceEvent::new(kind, frame, cycle);
        event.addr = Some(addr);
        event.value = Some(value);
        event.message = Some(message.into());
        event.severity = Some(
            match kind {
                "mapper.mmc1_write_ignored_consecutive"
                | "mapper.mmc1_chr_mode_unsafe"
                | "mapper.mmc1_prg_ram_disabled_access" => "warn",
                _ => "info",
            }
            .to_string(),
        );
        event.details = Some(self.trace_details(register));
        sink.push(event);
    }
}

impl Mapper for Mmc1Mapper {
    fn battery_prg_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }
    fn battery_prg_ram_mut(&mut self) -> Option<&mut [u8]> {
        Some(&mut self.prg_ram)
    }
    fn mapper_id(&self) -> u16 {
        1
    }
    fn mapper_name(&self) -> &'static str {
        "MMC1/SxROM"
    }
    fn physical_prg_bank_8k(&self, addr: u16) -> Option<u16> {
        let (low, high) = self.prg_window();
        let (bank, offset) = match addr {
            0x8000..=0xBFFF => (low, addr as usize - 0x8000),
            0xC000..=0xFFFF => (high, addr as usize - 0xC000),
            _ => return None,
        };
        physical_bank_8k(self.prg_rom.len(), 16 * 1024, bank, offset)
    }
    fn nametable_mirroring(&self) -> NametableMirroring {
        match self.control & 0x03 {
            0 => NametableMirroring::SingleScreenLow,
            1 => NametableMirroring::SingleScreenHigh,
            2 => NametableMirroring::Vertical,
            _ => NametableMirroring::Horizontal,
        }
    }
    fn read_prg(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u8 {
        match addr {
            0x6000..=0x7FFF => {
                if !self.prg_ram_enabled {
                    self.trace_event(
                        "mapper.mmc1_prg_ram_disabled_access",
                        addr,
                        0xff,
                        frame,
                        cycle,
                        cfg,
                        sink,
                        Some("prg_ram"),
                        "MMC1 PRG-RAM read while disabled; returning 0xFF",
                    );
                    0xff
                } else {
                    self.prg_ram[(addr as usize - 0x6000) % self.prg_ram.len()]
                }
            }
            0x8000..=0xBFFF => {
                let (lo, _) = self.prg_window();
                read_bank(&self.prg_rom, 16 * 1024, lo, addr as usize - 0x8000)
            }
            0xC000..=0xFFFF => {
                let (_, hi) = self.prg_window();
                read_bank(&self.prg_rom, 16 * 1024, hi, addr as usize - 0xC000)
            }
            _ => 0,
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
        match addr {
            0x6000..=0x7FFF => {
                if self.prg_ram_enabled {
                    let idx = (addr as usize - 0x6000) % self.prg_ram.len();
                    self.prg_ram[idx] = value;
                } else {
                    self.trace_event(
                        "mapper.mmc1_prg_ram_disabled_access",
                        addr,
                        value,
                        frame,
                        cycle,
                        cfg,
                        sink,
                        Some("prg_ram"),
                        "MMC1 PRG-RAM write ignored while disabled",
                    );
                }
            }
            0x8000..=0xFFFF => {
                if value & 0x80 != 0 {
                    self.shift = 0x10;
                    self.control |= 0x0C;
                    self.last_mapper_write_cycle = Some(cycle);
                    self.trace_event(
                        "mapper.mmc1_shift_reset",
                        addr,
                        value,
                        frame,
                        cycle,
                        cfg,
                        sink,
                        Some("shift"),
                        "MMC1 serial shift register reset",
                    );
                } else {
                    if self
                        .last_mapper_write_cycle
                        .and_then(|last| last.checked_add(1))
                        == Some(cycle)
                    {
                        self.trace_event(
                            "mapper.mmc1_write_ignored_consecutive",
                            addr,
                            value,
                            frame,
                            cycle,
                            cfg,
                            sink,
                            Some("shift"),
                            "MMC1 serial write ignored because it followed an accepted write on the next CPU cycle",
                        );
                        return;
                    }
                    self.last_mapper_write_cycle = Some(cycle);
                    let complete = self.shift & 1 != 0;
                    self.shift = (self.shift >> 1) | ((value & 1) << 4);
                    self.trace_event(
                        "mapper.mmc1_shift_write",
                        addr,
                        value,
                        frame,
                        cycle,
                        cfg,
                        sink,
                        Some("shift"),
                        "MMC1 accepted one LSB-first serial bit",
                    );
                    if complete {
                        let data = self.shift & 0x1F;
                        let old_outer = self.outer_prg_bank();
                        let old_prg_ram_enabled = self.prg_ram_enabled;
                        let register = self.commit(addr, data);
                        self.shift = 0x10;
                        let (lo, hi) = self.prg_window();
                        self.trace_event(
                            "mapper.mmc1_commit",
                            addr,
                            data,
                            frame,
                            cycle,
                            cfg,
                            sink,
                            Some(register),
                            format!(
                                "MMC1 commit data={data:02X}; PRG windows {lo}/{hi}; CHR {:?}",
                                self.chr_window()
                            ),
                        );
                        if old_outer != self.outer_prg_bank() {
                            self.trace_event(
                                "mapper.mmc1_outer_bank",
                                addr,
                                data,
                                frame,
                                cycle,
                                cfg,
                                sink,
                                Some(register),
                                "MMC1 SUROM outer 256 KiB bank changed",
                            );
                        }
                        self.trace_event(
                            "mapper.mmc1_prg_window",
                            addr,
                            data,
                            frame,
                            cycle,
                            cfg,
                            sink,
                            Some(register),
                            "MMC1 physical PRG windows updated",
                        );
                        if old_prg_ram_enabled != self.prg_ram_enabled {
                            self.trace_event(
                                "mapper.mmc1_prg_ram_enable",
                                addr,
                                data,
                                frame,
                                cycle,
                                cfg,
                                sink,
                                Some(register),
                                "MMC1 PRG-RAM enable state changed",
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }
    fn read_chr(&mut self, addr: u16) -> u8 {
        if self.control & 0x10 == 0 {
            read_bank(
                &self.chr,
                8 * 1024,
                (self.chr_bank0 as usize) >> 1,
                addr as usize,
            )
        } else if addr < 0x1000 {
            read_bank(&self.chr, 4 * 1024, self.chr_bank0 as usize, addr as usize)
        } else {
            read_bank(
                &self.chr,
                4 * 1024,
                self.chr_bank1 as usize,
                addr as usize - 0x1000,
            )
        }
    }
    fn write_chr(&mut self, addr: u16, value: u8) {
        if !self.chr_ram {
            return;
        }
        if self.control & 0x10 == 0 {
            write_bank(
                &mut self.chr,
                8 * 1024,
                (self.chr_bank0 as usize) >> 1,
                addr as usize,
                value,
            );
        } else if addr < 0x1000 {
            write_bank(
                &mut self.chr,
                4 * 1024,
                self.chr_bank0 as usize,
                addr as usize,
                value,
            );
        } else {
            write_bank(
                &mut self.chr,
                4 * 1024,
                self.chr_bank1 as usize,
                addr as usize - 0x1000,
                value,
            );
        }
    }
    fn notify_ppu_addr(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        self.last_ppu_addr = addr;
        if self.chr_outer_mismatch() && !self.chr_mode_mismatch_reported {
            self.chr_mode_mismatch_reported = true;
            self.trace_event(
                "mapper.mmc1_chr_mode_unsafe",
                addr,
                0,
                frame,
                cycle,
                cfg,
                sink,
                Some("chr_mode"),
                "SUROM 4 KiB CHR mode exposes mismatched outer-bank bits",
            );
        }
    }
    fn debug_state(&self) -> MapperDebugState {
        let (lo, hi) = self.prg_window();
        let mut state = mapper_debug_state(
            1,
            format!(
                "MMC1/{}{}",
                if self.board_variant == Mmc1BoardVariant::Surom512 {
                    "SUROM 512K"
                } else {
                    "SxROM"
                },
                if self.battery { " battery" } else { "" }
            ),
            false,
            vec![lo as u16, hi as u16],
            self.chr_window(),
            self.effective_mirroring(),
            false,
        );
        state.board_profile = Some(self.board_profile().to_string());
        state.board_profile_source = Some(if self.board_variant == Mmc1BoardVariant::Surom512 {
            "size_inference".to_string()
        } else {
            "generic_default".to_string()
        });
        state.outer_prg_bank = Some(self.outer_prg_bank() as u8);
        state.inner_prg_bank = Some(self.prg_bank & 0x0f);
        state.prg_ram_enabled = Some(self.prg_ram_enabled);
        state.mmc1_shift = Some(self.shift);
        state.mmc1_control = Some(self.control);
        state.mmc1_chr_bank0 = Some(self.chr_bank0);
        state.mmc1_chr_bank1 = Some(self.chr_bank1);
        state.mmc1_prg_bank = Some(self.prg_bank);
        state
    }
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut v = vec![
            self.shift,
            self.control,
            self.chr_bank0,
            self.chr_bank1,
            self.prg_bank,
            mirroring_code(self.mirroring),
            self.battery as u8,
            match self.board_variant {
                Mmc1BoardVariant::GenericSxrom => 0,
                Mmc1BoardVariant::Surom512 => 1,
            },
            self.submapper,
            self.prg_ram_enabled as u8,
            self.chr_mode_mismatch_reported as u8,
        ];
        v.extend_from_slice(
            &self
                .last_mapper_write_cycle
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        v.extend_from_slice(&self.last_ppu_addr.to_le_bytes());
        v.extend_from_slice(&self.prg_ram);
        v.extend_from_slice(&self.chr);
        v
    }
    fn restore_snapshot_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        const HEADER_LEN: usize = 21;
        let expected_len = HEADER_LEN + self.prg_ram.len() + self.chr.len();
        if bytes.len() != expected_len {
            return Err(KurosakiError::SnapshotFormat(format!(
                "MMC1 private state length mismatch: expected {expected_len}, got {}",
                bytes.len()
            )));
        }

        let expected_board = match self.board_variant {
            Mmc1BoardVariant::GenericSxrom => 0,
            Mmc1BoardVariant::Surom512 => 1,
        };
        if bytes[5] != mirroring_code(self.mirroring)
            || bytes[6] != self.battery as u8
            || bytes[7] != expected_board
            || bytes[8] != self.submapper
        {
            return Err(KurosakiError::SnapshotFormat(
                "MMC1 cartridge configuration does not match snapshot".to_string(),
            ));
        }

        self.shift = bytes[0];
        self.control = bytes[1];
        self.chr_bank0 = bytes[2];
        self.chr_bank1 = bytes[3];
        self.prg_bank = bytes[4];
        self.prg_ram_enabled = bytes[9] != 0;
        self.chr_mode_mismatch_reported = bytes[10] != 0;
        let last_cycle =
            u64::from_le_bytes(bytes[11..19].try_into().map_err(|_| {
                KurosakiError::SnapshotFormat("invalid MMC1 cycle field".to_string())
            })?);
        self.last_mapper_write_cycle = (last_cycle != u64::MAX).then_some(last_cycle);
        self.last_ppu_addr = u16::from_le_bytes(bytes[19..21].try_into().map_err(|_| {
            KurosakiError::SnapshotFormat("invalid MMC1 PPU address field".to_string())
        })?);

        let prg_end = HEADER_LEN + self.prg_ram.len();
        self.prg_ram.copy_from_slice(&bytes[HEADER_LEN..prg_end]);
        self.chr.copy_from_slice(&bytes[prg_end..]);
        Ok(())
    }

    fn restore_rebased_snapshot_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        const HEADER_LEN: usize = 21;
        let expected_len = HEADER_LEN + self.prg_ram.len() + self.chr.len();
        if bytes.len() != expected_len {
            return Err(KurosakiError::SnapshotFormat(format!(
                "MMC1 rebased private state length mismatch: expected {expected_len}, got {}",
                bytes.len()
            )));
        }
        if bytes[5] != mirroring_code(self.mirroring)
            || bytes[6] != self.battery as u8
            || bytes[8] != self.submapper
        {
            return Err(KurosakiError::SnapshotFormat(
                "MMC1 source and target cartridge configuration is not rebase-compatible"
                    .to_string(),
            ));
        }
        if bytes[7] > 1 {
            return Err(KurosakiError::SnapshotFormat(
                "MMC1 source snapshot has an unknown board variant".to_string(),
            ));
        }

        let mut adapted = bytes.to_vec();
        adapted[7] = match self.board_variant {
            Mmc1BoardVariant::GenericSxrom => 0,
            Mmc1BoardVariant::Surom512 => 1,
        };
        self.restore_snapshot_bytes(&adapted)
    }

    fn patch_prg_ram_bytes(&mut self, cpu_address: u16, bytes: &[u8]) -> Result<()> {
        let start = usize::from(cpu_address.checked_sub(0x6000).ok_or_else(|| {
            KurosakiError::SnapshotFormat(format!(
                "MMC1 PRG-RAM patch starts outside $6000-$7FFF: ${cpu_address:04X}"
            ))
        })?);
        let end = start.checked_add(bytes.len()).ok_or_else(|| {
            KurosakiError::SnapshotFormat("MMC1 PRG-RAM patch length overflow".to_string())
        })?;
        if end > self.prg_ram.len() || end > 0x2000 {
            return Err(KurosakiError::SnapshotFormat(format!(
                "MMC1 PRG-RAM patch exceeds $6000-$7FFF: start ${cpu_address:04X}, length {}",
                bytes.len()
            )));
        }
        self.prg_ram[start..end].copy_from_slice(bytes);
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Mmc2Mmc4Mapper {
    mapper_id: u16,
    name: &'static str,
    prg_rom: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    mirroring: NametableMirroring,
    prg_bank: u8,
    chr_banks: [u8; 4],
    latch0_fe: bool,
    latch1_fe: bool,
}

impl Mmc2Mmc4Mapper {
    fn new(mapper_id: u16, prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        let (chr, chr_ram) = ensure_chr(chr_rom);
        Self {
            mapper_id,
            name: if mapper_id == 9 {
                "MMC2/PxROM"
            } else {
                "MMC4/FxROM"
            },
            prg_rom,
            chr,
            chr_ram,
            mirroring: fixed_nametable_mirroring(mirroring),
            prg_bank: 0,
            chr_banks: [0; 4],
            latch0_fe: false,
            latch1_fe: false,
        }
    }

    fn selected_chr_bank(&self, addr: u16) -> usize {
        (if addr < 0x1000 {
            if self.latch0_fe {
                self.chr_banks[1]
            } else {
                self.chr_banks[0]
            }
        } else if self.latch1_fe {
            self.chr_banks[3]
        } else {
            self.chr_banks[2]
        }) as usize
    }
}

impl Mapper for Mmc2Mmc4Mapper {
    fn mapper_id(&self) -> u16 {
        self.mapper_id
    }

    fn mapper_name(&self) -> &'static str {
        self.name
    }

    fn physical_prg_bank_8k(&self, addr: u16) -> Option<u16> {
        if self.mapper_id == 9 {
            let bank = match addr {
                0x8000..=0x9FFF => self.prg_bank as usize,
                0xA000..=0xBFFF => bank_count(self.prg_rom.len(), 8 * 1024).saturating_sub(3),
                0xC000..=0xDFFF => bank_count(self.prg_rom.len(), 8 * 1024).saturating_sub(2),
                0xE000..=0xFFFF => bank_count(self.prg_rom.len(), 8 * 1024).saturating_sub(1),
                _ => return None,
            };
            return physical_bank_8k(self.prg_rom.len(), 8 * 1024, bank, addr as usize & 0x1FFF);
        }
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

    fn nametable_mirroring(&self) -> NametableMirroring {
        self.mirroring
    }

    fn read_prg(
        &mut self,
        addr: u16,
        _frame: u64,
        _cycle: u64,
        _cfg: TraceConfig,
        _sink: &mut TraceSink,
    ) -> u8 {
        if addr < 0x8000 {
            return 0;
        }
        if self.mapper_id == 9 {
            let bank = match addr {
                0x8000..=0x9FFF => self.prg_bank as usize,
                0xA000..=0xBFFF => bank_count(self.prg_rom.len(), 8 * 1024).saturating_sub(3),
                0xC000..=0xDFFF => bank_count(self.prg_rom.len(), 8 * 1024).saturating_sub(2),
                _ => bank_count(self.prg_rom.len(), 8 * 1024).saturating_sub(1),
            };
            read_bank(&self.prg_rom, 8 * 1024, bank, addr as usize & 0x1FFF)
        } else {
            let bank = if addr < 0xC000 {
                self.prg_bank as usize
            } else {
                bank_count(self.prg_rom.len(), 16 * 1024).saturating_sub(1)
            };
            read_bank(&self.prg_rom, 16 * 1024, bank, addr as usize & 0x3FFF)
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
        if !(0xA000..=0xFFFF).contains(&addr) {
            return;
        }
        match addr & 0xF000 {
            0xA000 => self.prg_bank = value,
            0xB000 => self.chr_banks[0] = value,
            0xC000 => self.chr_banks[1] = value,
            0xD000 => self.chr_banks[2] = value,
            0xE000 => self.chr_banks[3] = value,
            0xF000 => {
                self.mirroring = if value & 1 == 0 {
                    NametableMirroring::Vertical
                } else {
                    NametableMirroring::Horizontal
                };
            }
            _ => {}
        }
        trace_mapper_write(
            "mapper.mmc2_mmc4_write",
            addr,
            value,
            frame,
            cycle,
            cfg,
            sink,
            format!("{} register write ${addr:04X}={value:02X}", self.name),
        );
    }

    fn read_chr(&mut self, addr: u16) -> u8 {
        read_bank(
            &self.chr,
            4 * 1024,
            self.selected_chr_bank(addr),
            addr as usize & 0x0FFF,
        )
    }

    fn write_chr(&mut self, addr: u16, value: u8) {
        if self.chr_ram {
            let bank = self.selected_chr_bank(addr);
            write_bank(&mut self.chr, 4 * 1024, bank, addr as usize & 0x0FFF, value);
        }
    }

    fn notify_ppu_addr(
        &mut self,
        addr: u16,
        _frame: u64,
        _cycle: u64,
        _cfg: TraceConfig,
        _sink: &mut TraceSink,
    ) {
        match addr & 0x1FF8 {
            0x0FD8 => self.latch0_fe = false,
            0x0FE8 => self.latch0_fe = true,
            0x1FD8 => self.latch1_fe = false,
            0x1FE8 => self.latch1_fe = true,
            _ => {}
        }
    }

    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            self.mapper_id,
            self.name,
            false,
            vec![self.prg_bank as u16],
            self.chr_banks.iter().map(|bank| *bank as u16).collect(),
            match self.mirroring {
                NametableMirroring::Horizontal => "horizontal",
                NametableMirroring::Vertical => "vertical",
                NametableMirroring::FourScreen => "four_screen",
                NametableMirroring::SingleScreenLow => "single_screen_low",
                NametableMirroring::SingleScreenHigh => "single_screen_high",
            }
            .to_string(),
            false,
        )
    }

    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![
            self.prg_bank,
            self.chr_banks[0],
            self.chr_banks[1],
            self.chr_banks[2],
            self.chr_banks[3],
            self.latch0_fe as u8,
            self.latch1_fe as u8,
            self.mirroring as u8,
        ];
        bytes.extend_from_slice(&self.chr);
        bytes
    }

    fn restore_snapshot_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let header_len = 8;
        let expected_len = header_len + self.chr.len();
        if bytes.len() != expected_len {
            return Err(KurosakiError::SnapshotFormat(format!(
                "{} private state length mismatch: expected {expected_len}, got {}",
                self.name,
                bytes.len()
            )));
        }
        self.prg_bank = bytes[0];
        self.chr_banks.copy_from_slice(&bytes[1..5]);
        self.latch0_fe = bytes[5] != 0;
        self.latch1_fe = bytes[6] != 0;
        self.mirroring = match bytes[7] {
            0 => NametableMirroring::Horizontal,
            1 => NametableMirroring::Vertical,
            2 => NametableMirroring::FourScreen,
            3 => NametableMirroring::SingleScreenLow,
            _ => NametableMirroring::SingleScreenHigh,
        };
        self.chr.copy_from_slice(&bytes[header_len..]);
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Mmc3Mapper {
    mapper_id: u16,
    variant: u8,
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    variant_chr_ram: Vec<u8>,
    mirroring: Mirroring,
    battery: bool,
    bank_select: u8,
    bank_regs: [u8; 8],
    prg_ram_protect: u8,
    irq_latch: u8,
    irq_counter: u8,
    irq_reload: bool,
    irq_enabled: bool,
    irq_pending_flag: bool,
    last_ppu_a12: bool,
    a12_low_ticks: u16,
    irq_clock_count: u64,
}

impl Mmc3Mapper {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring, battery: bool) -> Self {
        Self::new_variant(4, prg_rom, chr_rom, mirroring, battery)
    }
    pub fn new_variant(
        mapper_id: u16,
        prg_rom: Vec<u8>,
        chr_rom: Vec<u8>,
        mirroring: Mirroring,
        battery: bool,
    ) -> Self {
        let (chr, chr_ram) = ensure_chr(chr_rom);
        Self {
            mapper_id,
            variant: match mapper_id {
                118 => 1,
                119 => 2,
                _ => 0,
            },
            prg_rom,
            prg_ram: vec![0; 8 * 1024],
            chr,
            chr_ram,
            variant_chr_ram: vec![0; 8 * 1024],
            mirroring,
            battery,
            bank_select: 0,
            bank_regs: [0; 8],
            prg_ram_protect: 0,
            irq_latch: 0,
            irq_counter: 0,
            irq_reload: false,
            irq_enabled: false,
            irq_pending_flag: false,
            last_ppu_a12: false,
            a12_low_ticks: 0,
            irq_clock_count: 0,
        }
    }
    fn prg_windows(&self) -> [usize; 4] {
        let count = bank_count(self.prg_rom.len(), 8 * 1024);
        let last = count - 1;
        let second_last = count.saturating_sub(2);
        let r6 = self.bank_regs[6] as usize % count;
        let r7 = self.bank_regs[7] as usize % count;
        if self.bank_select & 0x40 == 0 {
            [r6, r7, second_last, last]
        } else {
            [second_last, r7, r6, last]
        }
    }
    fn chr_windows(&self) -> Vec<u16> {
        self.bank_regs.iter().take(6).map(|v| *v as u16).collect()
    }
    fn chr_bank_for_addr(&self, addr: u16) -> (usize, bool) {
        let chr_mode = self.bank_select & 0x80 != 0;
        let a = addr as usize;
        let bank = if !chr_mode {
            match addr {
                0x0000..=0x07FF => (self.bank_regs[0] & !1) as usize + (a / 0x400),
                0x0800..=0x0FFF => (self.bank_regs[1] & !1) as usize + ((a - 0x0800) / 0x400),
                0x1000..=0x13FF => self.bank_regs[2] as usize,
                0x1400..=0x17FF => self.bank_regs[3] as usize,
                0x1800..=0x1BFF => self.bank_regs[4] as usize,
                _ => self.bank_regs[5] as usize,
            }
        } else {
            match addr {
                0x0000..=0x03FF => self.bank_regs[2] as usize,
                0x0400..=0x07FF => self.bank_regs[3] as usize,
                0x0800..=0x0BFF => self.bank_regs[4] as usize,
                0x0C00..=0x0FFF => self.bank_regs[5] as usize,
                0x1000..=0x17FF => (self.bank_regs[0] & !1) as usize + ((a - 0x1000) / 0x400),
                _ => (self.bank_regs[1] & !1) as usize + ((a - 0x1800) / 0x400),
            }
        };
        (bank, self.variant == 2 && (bank & 0x40) != 0)
    }
    fn write_register(&mut self, addr: u16, value: u8) {
        match (addr & 0xE001, addr & 1) {
            (0x8000, 0) => self.bank_select = value,
            (0x8001, 1) => self.bank_regs[(self.bank_select & 0x07) as usize] = value,
            (0xA000, 0) => {
                self.mirroring = if value & 1 == 0 {
                    Mirroring::Vertical
                } else {
                    Mirroring::Horizontal
                }
            }
            (0xA001, 1) => self.prg_ram_protect = value,
            (0xC000, 0) => self.irq_latch = value,
            (0xC001, 1) => self.irq_reload = true,
            (0xE000, 0) => {
                self.irq_enabled = false;
                self.irq_pending_flag = false;
            }
            (0xE001, 1) => self.irq_enabled = true,
            _ => {}
        }
    }

    fn clock_irq_counter(
        &mut self,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        self.irq_clock_count = self.irq_clock_count.saturating_add(1);
        if self.irq_counter == 0 || self.irq_reload {
            self.irq_counter = self.irq_latch;
            self.irq_reload = false;
        } else {
            self.irq_counter = self.irq_counter.saturating_sub(1);
        }
        if cfg.mapper {
            let mut event = TraceEvent::new("mapper.mmc3_irq_clock", frame, cycle);
            event.value = Some(self.irq_counter);
            event.message = Some(format!(
                "MMC3 A12 IRQ clock; counter={} latch={} enabled={}",
                self.irq_counter, self.irq_latch, self.irq_enabled
            ));
            sink.push(event);
        }
        if self.irq_counter == 0 && self.irq_enabled {
            self.irq_pending_flag = true;
            if cfg.mapper || cfg.nmi {
                let mut event = TraceEvent::new("mapper.irq", frame, cycle);
                event.severity = Some("info".to_string());
                event.message = Some("MMC3 IRQ pending after A12 scanline clock".to_string());
                sink.push(event);
            }
        }
    }
}

impl Mapper for Mmc3Mapper {
    fn battery_prg_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }
    fn battery_prg_ram_mut(&mut self) -> Option<&mut [u8]> {
        Some(&mut self.prg_ram)
    }
    fn mapper_id(&self) -> u16 {
        self.mapper_id
    }
    fn mapper_name(&self) -> &'static str {
        match self.mapper_id {
            118 => "TLSROM/TKSROM",
            119 => "TQROM",
            _ => "MMC3/MMC6",
        }
    }
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
            0x8000..=0xFFFF => {
                self.write_register(addr, value);
                trace_mapper_write(
                    "mapper.mmc3_write",
                    addr,
                    value,
                    frame,
                    cycle,
                    cfg,
                    sink,
                    format!(
                        "MMC3 write; PRG {:?}; IRQ latch={} enabled={}",
                        self.prg_windows(),
                        self.irq_latch,
                        self.irq_enabled
                    ),
                );
            }
            _ => {}
        }
    }
    fn read_chr(&mut self, addr: u16) -> u8 {
        let a = addr as usize;
        let (bank_1k, ram) = self.chr_bank_for_addr(addr);
        let physical_bank = if self.variant == 2 {
            bank_1k & 0x3F
        } else {
            bank_1k
        };
        if ram {
            read_bank(&self.variant_chr_ram, 1024, physical_bank, a % 1024)
        } else {
            read_bank(&self.chr, 1024, physical_bank, a % 1024)
        }
    }
    fn write_chr(&mut self, addr: u16, value: u8) {
        let (bank, variant_ram) = self.chr_bank_for_addr(addr);
        if variant_ram {
            let idx = (bank & 0x3F) * 1024 + (addr as usize % 1024);
            let len = self.variant_chr_ram.len();
            self.variant_chr_ram[idx % len] = value;
        } else if self.chr_ram {
            let a = addr as usize;
            let len = self.chr.len();
            self.chr[a % len] = value;
        }
    }
    fn notify_ppu_addr(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        // MMC3 clocks its IRQ counter on filtered rising edges of PPU A12. This is
        // still a fixture-driven approximation, but it is connected to real PPU
        // pattern fetches instead of being a write-only scaffold.
        let a12 = addr & 0x1000 != 0;
        if !a12 {
            self.a12_low_ticks = self.a12_low_ticks.saturating_add(1).min(32);
        } else if !self.last_ppu_a12 && self.a12_low_ticks >= 2 {
            self.clock_irq_counter(frame, cycle, cfg, sink);
            self.a12_low_ticks = 0;
        }
        self.last_ppu_a12 = a12;
    }
    fn nametable_mirroring(&self) -> NametableMirroring {
        if self.variant == 1 {
            // TLSROM/TKSROM route CHR A17 to the nametable select line.
            if self.bank_regs[0] & 0x40 == 0 {
                NametableMirroring::Vertical
            } else {
                NametableMirroring::Horizontal
            }
        } else {
            fixed_nametable_mirroring(self.mirroring)
        }
    }
    fn irq_pending(&self) -> bool {
        self.irq_pending_flag
    }
    fn clear_irq(&mut self) {
        self.irq_pending_flag = false;
    }
    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            self.mapper_id,
            format!(
                "{}{} irq_clocks={}",
                self.mapper_name(),
                if self.battery { " battery" } else { "" },
                self.irq_clock_count
            ),
            false,
            self.prg_windows().iter().map(|v| *v as u16).collect(),
            self.chr_windows(),
            match self.nametable_mirroring() {
                NametableMirroring::Horizontal => "horizontal".to_string(),
                NametableMirroring::Vertical => "vertical".to_string(),
                NametableMirroring::FourScreen => "four_screen".to_string(),
                NametableMirroring::SingleScreenLow => "single_screen_low".to_string(),
                NametableMirroring::SingleScreenHigh => "single_screen_high".to_string(),
            },
            self.irq_pending(),
        )
    }
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut v = vec![
            self.bank_select,
            self.prg_ram_protect,
            self.irq_latch,
            self.irq_counter,
            self.irq_reload as u8,
            self.irq_enabled as u8,
            self.irq_pending_flag as u8,
            self.last_ppu_a12 as u8,
            (self.a12_low_ticks & 0xFF) as u8,
            self.battery as u8,
            self.mapper_id as u8,
            self.variant,
        ];
        v.extend_from_slice(&self.irq_clock_count.to_le_bytes());
        v.extend_from_slice(&self.bank_regs);
        v.extend_from_slice(&self.prg_ram);
        v.extend_from_slice(&self.chr);
        v.extend_from_slice(&self.variant_chr_ram);
        v
    }
    fn restore_snapshot_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let header = 12 + 8 + 8;
        let expected = header + self.prg_ram.len() + self.chr.len() + self.variant_chr_ram.len();
        if bytes.len() != expected {
            return Err(KurosakiError::SnapshotFormat(format!(
                "MMC3 variant snapshot length mismatch: expected {expected}, got {}",
                bytes.len()
            )));
        }
        self.bank_select = bytes[0];
        self.prg_ram_protect = bytes[1];
        self.irq_latch = bytes[2];
        self.irq_counter = bytes[3];
        self.irq_reload = bytes[4] != 0;
        self.irq_enabled = bytes[5] != 0;
        self.irq_pending_flag = bytes[6] != 0;
        self.last_ppu_a12 = bytes[7] != 0;
        self.a12_low_ticks = bytes[8] as u16;
        self.irq_clock_count = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
        self.bank_regs.copy_from_slice(&bytes[20..28]);
        let mut at = 28;
        let prg_ram_len = self.prg_ram.len();
        self.prg_ram.copy_from_slice(&bytes[at..at + prg_ram_len]);
        at += prg_ram_len;
        let chr_len = self.chr.len();
        self.chr.copy_from_slice(&bytes[at..at + chr_len]);
        at += chr_len;
        let variant_chr_ram_len = self.variant_chr_ram.len();
        self.variant_chr_ram
            .copy_from_slice(&bytes[at..at + variant_chr_ram_len]);
        Ok(())
    }
}

const CPU_HZ_NTSC: f64 = 1_789_773.0;
const FDS_ENV_CLOCK_DIVIDER: f64 = 8.0;

#[derive(Debug, Clone)]
struct FdsEnvelopeUnit {
    control: u8,
    gain: u8,
    divider: f64,
    max_gain: u8,
}

impl FdsEnvelopeUnit {
    fn new(max_gain: u8) -> Self {
        Self {
            control: 0x80,
            gain: 0,
            divider: 0.0,
            max_gain,
        }
    }

    fn write_control(&mut self, value: u8) {
        self.control = value;
        self.divider = 0.0;
        let raw = value & 0x3F;
        if self.direct_mode() {
            self.gain = raw.min(self.max_gain);
        } else if self.increase_mode() {
            self.gain = 0;
        } else {
            self.gain = self.max_gain;
        }
    }

    fn direct_mode(&self) -> bool {
        self.control & 0x80 != 0
    }

    fn increase_mode(&self) -> bool {
        self.control & 0x40 != 0
    }

    fn output(&self) -> u8 {
        self.gain.min(self.max_gain)
    }

    fn clock(&mut self, sample_rate: u32, master_speed: u8, halted: bool) {
        if halted || self.direct_mode() {
            return;
        }
        let local = (self.control & 0x3F) as f64 + 1.0;
        let master = master_speed as f64 + 1.0;
        let clocks_per_second = CPU_HZ_NTSC / (FDS_ENV_CLOCK_DIVIDER * local * master);
        self.divider += clocks_per_second / sample_rate.max(1) as f64;
        while self.divider >= 1.0 {
            self.divider -= 1.0;
            if self.increase_mode() {
                if self.gain < self.max_gain {
                    self.gain += 1;
                }
            } else if self.gain > 0 {
                self.gain -= 1;
            }
        }
    }

    fn snapshot_bytes(&self, out: &mut Vec<u8>) {
        out.push(self.control);
        out.push(self.gain);
        out.extend_from_slice(&self.divider.to_le_bytes());
        out.push(self.max_gain);
    }
}

#[derive(Debug, Clone)]
struct FdsAudioState {
    wave_ram: [u8; 64],
    mod_table: [u8; 64],
    volume_reg: u8,
    volume_env_speed: u8,
    freq: u16,
    master_volume: u8,
    wave_write_enable: bool,
    wave_halt: bool,
    volume_env_halt: bool,
    phase: f64,
    mod_reg: u8,
    mod_freq: u16,
    mod_halt: bool,
    mod_env_halt: bool,
    mod_pos: usize,
    mod_phase: f64,
    mod_accum: i32,
    volume_env: FdsEnvelopeUnit,
    mod_env: FdsEnvelopeUnit,
    env_master_speed: u8,
    audio_writes: u64,
}

impl Default for FdsAudioState {
    fn default() -> Self {
        Self {
            // FDS BIOS/game code normally writes a 64-sample waveform before enabling audio.
            // A neutral midpoint avoids a DC pop before the first write.
            wave_ram: [32; 64],
            mod_table: [0; 64],
            volume_reg: 0x80,
            volume_env_speed: 0,
            freq: 0,
            master_volume: 0,
            wave_write_enable: false,
            wave_halt: true,
            volume_env_halt: true,
            phase: 0.0,
            mod_reg: 0x80,
            mod_freq: 0,
            mod_halt: true,
            mod_env_halt: true,
            mod_pos: 0,
            mod_phase: 0.0,
            mod_accum: 0,
            volume_env: FdsEnvelopeUnit::new(32),
            mod_env: FdsEnvelopeUnit::new(63),
            env_master_speed: 0,
            audio_writes: 0,
        }
    }
}

impl FdsAudioState {
    fn read(&self, addr: u16) -> u8 {
        match addr {
            0x4040..=0x407F => self.wave_ram[(addr - 0x4040) as usize] & 0x3F,
            0x4080 => self.volume_reg,
            0x4081 => self.volume_env_speed,
            0x4082 => self.freq as u8,
            0x4083 => {
                ((self.freq >> 8) as u8 & 0x0F)
                    | (if self.volume_env_halt { 0x40 } else { 0x00 })
                    | (if self.wave_halt { 0x80 } else { 0x00 })
            }
            0x4084 => self.mod_reg,
            0x4085 => (self.mod_accum as i8) as u8,
            0x4086 => self.mod_freq as u8,
            0x4087 => {
                ((self.mod_freq >> 8) as u8 & 0x0F)
                    | (if self.mod_env_halt { 0x40 } else { 0x00 })
                    | (if self.mod_halt { 0x80 } else { 0x00 })
            }
            0x4089 => {
                (self.master_volume & 0x03) | (if self.wave_write_enable { 0x80 } else { 0x00 })
            }
            0x408A => self.env_master_speed,
            0x4090 => self.volume_env.output(),
            0x4092 => self.mod_env.output(),
            _ => 0,
        }
    }

    fn write(&mut self, addr: u16, value: u8) {
        self.audio_writes = self.audio_writes.saturating_add(1);
        match addr {
            0x4040..=0x407F => {
                if self.wave_write_enable {
                    self.wave_ram[(addr - 0x4040) as usize] = value & 0x3F;
                }
            }
            0x4080 => {
                self.volume_reg = value;
                self.volume_env.write_control(value);
            }
            // Some test ROMs and homebrew tools mirror envelope speed writes here.
            // Keeping it as a compatibility shadow does not change official $4080 behavior.
            0x4081 => self.volume_env_speed = value,
            0x4082 => self.freq = (self.freq & 0x0F00) | value as u16,
            0x4083 => {
                self.freq = (self.freq & 0x00FF) | (((value & 0x0F) as u16) << 8);
                self.volume_env_halt = value & 0x40 != 0;
                self.wave_halt = value & 0x80 != 0;
                if self.wave_halt {
                    self.phase = 0.0;
                }
            }
            0x4084 => {
                self.mod_reg = value;
                self.mod_env.write_control(value);
            }
            0x4085 => {
                // Hardware exposes a signed modulation counter. Keeping it signed
                // makes the debug mixer deterministic while still allowing games
                // that poke this register to change pitch immediately.
                self.mod_accum = (value as i8) as i32;
                self.mod_pos = 0;
                self.mod_phase = 0.0;
            }
            0x4086 => self.mod_freq = (self.mod_freq & 0x0F00) | value as u16,
            0x4087 => {
                self.mod_freq = (self.mod_freq & 0x00FF) | (((value & 0x0F) as u16) << 8);
                self.mod_env_halt = value & 0x40 != 0;
                self.mod_halt = value & 0x80 != 0;
                if self.mod_halt {
                    self.mod_phase = 0.0;
                    self.mod_pos = 0;
                }
            }
            0x4088 => {
                if self.mod_halt || self.wave_write_enable {
                    self.mod_table[self.mod_pos & 63] = value & 0x07;
                    self.mod_pos = (self.mod_pos + 1) & 63;
                }
            }
            0x4089 => {
                self.master_volume = value & 0x03;
                self.wave_write_enable = value & 0x80 != 0;
            }
            0x408A => self.env_master_speed = value,
            _ => {}
        }
    }

    fn clock_envelopes(&mut self, sample_rate: u32) {
        let volume_halted = self.wave_halt || self.volume_env_halt;
        let mod_halted = self.mod_halt || self.mod_env_halt;
        self.volume_env
            .clock(sample_rate, self.env_master_speed, volume_halted);
        self.mod_env
            .clock(sample_rate, self.env_master_speed, mod_halted);
    }

    fn clock_modulation(&mut self, sample_rate: u32) {
        if self.mod_halt || self.mod_freq == 0 {
            return;
        }
        let hz = self.mod_freq as f64 * CPU_HZ_NTSC / 65_536.0;
        self.mod_phase += hz / sample_rate.max(1) as f64;
        while self.mod_phase >= 1.0 {
            self.mod_phase -= 1.0;
            let value = self.mod_table[self.mod_pos & 63] & 0x07;
            let delta = match value {
                0 => 0,
                1 => 1,
                2 => 2,
                3 => 4,
                4 => 0,
                5 => -4,
                6 => -2,
                7 => -1,
                _ => 0,
            };
            self.mod_accum = (self.mod_accum + delta).clamp(-128, 127);
            self.mod_pos = (self.mod_pos + 1) & 63;
        }
    }

    fn effective_frequency_hz(&mut self, sample_rate: u32) -> f64 {
        self.clock_modulation(sample_rate);
        let base = self.freq as f64 * CPU_HZ_NTSC / 65_536.0;
        let depth_units = self.mod_env.output() as f64;
        if self.mod_halt || self.mod_freq == 0 || depth_units <= 0.0 {
            return base;
        }
        // Clean-room approximation of FDS modulation. The real hardware bends the
        // wave frequency through a modulation unit; here the signed counter and
        // the modulation envelope produce a bounded deterministic pitch bend.
        let depth = depth_units / 63.0;
        let bend = (self.mod_accum as f64 / 128.0) * depth * 0.60;
        base * (1.0 + bend).clamp(0.45, 1.60)
    }

    fn sample(&mut self, sample_rate: u32) -> i16 {
        self.clock_envelopes(sample_rate);
        if self.wave_halt || self.freq == 0 {
            return 0;
        }
        let volume_units = self.volume_env.output().min(32);
        if volume_units == 0 {
            return 0;
        }
        let hz = self.effective_frequency_hz(sample_rate);
        self.phase = (self.phase + hz / sample_rate.max(1) as f64).fract();
        let index = ((self.phase * 64.0) as usize) & 63;
        let wave = self.wave_ram[index] as f64 - 32.0;
        let volume = volume_units as f64 / 32.0;
        let master = match self.master_volume & 0x03 {
            0 => 1.00,
            1 => 0.67,
            2 => 0.50,
            _ => 0.40,
        };
        (wave * volume * master * 90.0).clamp(i16::MIN as f64, i16::MAX as f64) as i16
    }

    fn snapshot_bytes(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.wave_ram);
        out.extend_from_slice(&self.mod_table);
        out.push(self.volume_reg);
        out.push(self.volume_env_speed);
        out.extend_from_slice(&self.freq.to_le_bytes());
        out.push(self.master_volume);
        out.push(self.wave_write_enable as u8);
        out.push(self.wave_halt as u8);
        out.push(self.volume_env_halt as u8);
        out.extend_from_slice(&self.phase.to_le_bytes());
        out.push(self.mod_reg);
        out.extend_from_slice(&self.mod_freq.to_le_bytes());
        out.push(self.mod_halt as u8);
        out.push(self.mod_env_halt as u8);
        out.push((self.mod_pos & 0xFF) as u8);
        out.extend_from_slice(&self.mod_phase.to_le_bytes());
        out.extend_from_slice(&self.mod_accum.to_le_bytes());
        self.volume_env.snapshot_bytes(out);
        self.mod_env.snapshot_bytes(out);
        out.push(self.env_master_speed);
        out.extend_from_slice(&self.audio_writes.to_le_bytes());
    }
}

#[derive(Debug, Clone)]
pub struct FdsMapper {
    prg_ram: Vec<u8>,
    chr_ram: Vec<u8>,
    bios_rom: Vec<u8>,
    disk: Option<FdsDiskImage>,
    direct_boot_vectors: bool,
    disk_regs: [u8; 0x20],
    irq_reload: u16,
    irq_counter: u16,
    irq_enabled: bool,
    irq_repeat: bool,
    irq_pending_flag: bool,
    master_io_enable: bool,
    disk_motor_on: bool,
    transfer_busy: bool,
    transfer_cycles_remaining: u32,
    disk_byte_index: u32,
    disk_io_events: u64,
    audio: FdsAudioState,
}
impl FdsMapper {
    pub fn new(prg_rom: Vec<u8>, _chr_rom: Vec<u8>) -> Self {
        Self {
            prg_ram: vec![0; 32 * 1024],
            chr_ram: vec![0; 8 * 1024],
            bios_rom: normalize_fds_bios(prg_rom),
            disk: None,
            direct_boot_vectors: false,
            disk_regs: [0; 0x20],
            irq_reload: 0,
            irq_counter: 0,
            irq_enabled: false,
            irq_repeat: false,
            irq_pending_flag: false,
            master_io_enable: false,
            disk_motor_on: false,
            transfer_busy: false,
            transfer_cycles_remaining: 0,
            disk_byte_index: 0,
            disk_io_events: 0,
            audio: FdsAudioState::default(),
        }
    }

    pub fn new_with_disk(bios_rom: Vec<u8>, disk: FdsDiskImage) -> Self {
        let mut mapper = Self::new(bios_rom, Vec::new());
        mapper.prg_ram = disk.boot_prg_ram();
        mapper.chr_ram = disk.boot_chr_ram();
        mapper.direct_boot_vectors = true;
        mapper.disk = Some(disk);
        mapper
    }
    fn read_disk_reg(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u8 {
        let value = match addr {
            0x4030 => {
                let v = if self.irq_pending_flag { 0x01 } else { 0x00 };
                self.irq_pending_flag = false;
                v
            }
            0x4031 => {
                self.disk_io_events = self.disk_io_events.saturating_add(1);
                self.disk_byte_index = self.disk_byte_index.wrapping_add(1);
                (self.disk_byte_index & 0xFF) as u8
            }
            0x4032 => {
                if self.disk_motor_on {
                    0x00
                } else {
                    0x02
                }
            }
            0x4033 => 0x80,
            _ => self.disk_regs[(addr as usize - 0x4020) % self.disk_regs.len()],
        };
        if cfg.mapper {
            let mut event = TraceEvent::new("fds.reg_read", frame, cycle);
            event.addr = Some(addr);
            event.value = Some(value);
            event.message = Some(format!(
                "FDS read; motor={} busy={} byte_index={}",
                self.disk_motor_on, self.transfer_busy, self.disk_byte_index
            ));
            sink.push(event);
        }
        value
    }
    fn write_disk_reg(
        &mut self,
        addr: u16,
        value: u8,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        let idx = (addr as usize - 0x4020) % self.disk_regs.len();
        self.disk_regs[idx] = value;
        match addr {
            0x4020 => self.irq_reload = (self.irq_reload & 0xFF00) | value as u16,
            0x4021 => self.irq_reload = (self.irq_reload & 0x00FF) | ((value as u16) << 8),
            0x4022 => {
                self.irq_enabled = value & 0x02 != 0;
                self.irq_repeat = value & 0x01 != 0;
                if self.irq_enabled {
                    self.irq_counter = self.irq_reload;
                } else {
                    self.irq_pending_flag = false;
                }
            }
            0x4023 => self.master_io_enable = value & 0x01 != 0,
            0x4025 => {
                self.disk_motor_on = value & 0x01 == 0;
                self.transfer_busy = self.disk_motor_on;
                self.transfer_cycles_remaining = if self.disk_motor_on { 150 } else { 0 };
            }
            _ => {}
        }
        if cfg.mapper {
            let mut event = TraceEvent::new("fds.reg_write", frame, cycle);
            event.addr = Some(addr);
            event.value = Some(value);
            event.message = Some(format!(
                "FDS write; irq_reload={} irq_enabled={} motor={} busy={}",
                self.irq_reload, self.irq_enabled, self.disk_motor_on, self.transfer_busy
            ));
            sink.push(event);
        }
    }

    fn read_audio_reg(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u8 {
        let value = self.audio.read(addr);
        if cfg.mapper {
            let mut event = TraceEvent::new("fds.audio_read", frame, cycle);
            event.addr = Some(addr);
            event.value = Some(value);
            event.message = Some(format!(
                "FDS audio read; freq={} vol_env={} mod_freq={} mod_env={} mod_accum={} writes={}",
                self.audio.freq,
                self.audio.volume_env.output(),
                self.audio.mod_freq,
                self.audio.mod_env.output(),
                self.audio.mod_accum,
                self.audio.audio_writes
            ));
            sink.push(event);
        }
        value
    }

    fn write_audio_reg(
        &mut self,
        addr: u16,
        value: u8,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        self.audio.write(addr, value);
        if cfg.mapper || cfg.apu {
            let mut event = TraceEvent::new("mapper.expansion_audio_write", frame, cycle);
            event.addr = Some(addr);
            event.value = Some(value);
            event.message = Some(format!(
                "FDS audio write; freq={} vol_env={} vol_raw=${:02X} master={} mod_freq={} mod_env={} mod_raw=${:02X} wave_halt={} vol_env_halt={} mod_halt={} mod_env_halt={} wave_write_enable={} env_master={} writes={}",
                self.audio.freq,
                self.audio.volume_env.output(),
                self.audio.volume_reg,
                self.audio.master_volume,
                self.audio.mod_freq,
                self.audio.mod_env.output(),
                self.audio.mod_reg,
                self.audio.wave_halt,
                self.audio.volume_env_halt,
                self.audio.mod_halt,
                self.audio.mod_env_halt,
                self.audio.wave_write_enable,
                self.audio.env_master_speed,
                self.audio.audio_writes
            ));
            sink.push(event);
        }
    }
}
impl Mapper for FdsMapper {
    fn mapper_id(&self) -> u16 {
        20
    }
    fn mapper_name(&self) -> &'static str {
        "FDS timing scaffold"
    }
    fn read_prg(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u8 {
        match addr {
            0x4020..=0x403F => self.read_disk_reg(addr, frame, cycle, cfg, sink),
            0x4040..=0x409F => self.read_audio_reg(addr, frame, cycle, cfg, sink),
            0x6000..=0xDFFF => self.prg_ram[(addr as usize - 0x6000) % self.prg_ram.len()],
            0xFFFA..=0xFFFF if self.direct_boot_vectors => {
                let game_vector_addr = 0xDFFAusize + (addr as usize - 0xFFFA);
                self.prg_ram[(game_vector_addr - 0x6000) % self.prg_ram.len()]
            }
            0xE000..=0xFFFF => self.bios_rom[(addr as usize - 0xE000) % self.bios_rom.len()],
            _ => 0,
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
        match addr {
            0x4020..=0x403F => self.write_disk_reg(addr, value, frame, cycle, cfg, sink),
            0x4040..=0x409F => self.write_audio_reg(addr, value, frame, cycle, cfg, sink),
            0x6000..=0xDFFF => {
                let idx = (addr as usize - 0x6000) % self.prg_ram.len();
                self.prg_ram[idx] = value;
            }
            _ => {}
        }
    }
    fn read_chr(&mut self, addr: u16) -> u8 {
        self.chr_ram[(addr as usize) % self.chr_ram.len()]
    }
    fn write_chr(&mut self, addr: u16, value: u8) {
        let len = self.chr_ram.len();
        self.chr_ram[(addr as usize) % len] = value;
    }
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
                if self.irq_repeat {
                    self.irq_counter = self.irq_reload;
                } else {
                    self.irq_enabled = false;
                }
                if cfg.mapper || cfg.nmi {
                    let mut event = TraceEvent::new("fds.irq", frame, cycle);
                    event.message = Some("FDS timer IRQ pending".to_string());
                    sink.push(event);
                }
            }
        }
        if self.transfer_busy {
            if cpu_cycles as u32 >= self.transfer_cycles_remaining {
                self.transfer_busy = false;
                self.disk_io_events = self.disk_io_events.saturating_add(1);
                if cfg.mapper {
                    let mut event = TraceEvent::new("fds.transfer_tick", frame, cycle);
                    event.message = Some(format!(
                        "FDS disk byte transfer completed; events={}",
                        self.disk_io_events
                    ));
                    sink.push(event);
                }
            } else {
                self.transfer_cycles_remaining -= cpu_cycles as u32;
            }
        }
    }
    fn irq_pending(&self) -> bool {
        self.irq_pending_flag
    }
    fn clear_irq(&mut self) {
        self.irq_pending_flag = false;
    }
    fn expansion_audio_sample(&mut self, sample_rate: u32) -> i16 {
        self.audio.sample(sample_rate)
    }
    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            20,
            format!(
                "FDS timing+audio scaffold motor={} busy={} io_events={} audio_writes={} freq={} vol_env={} mod_freq={} mod_env={} mod_accum={}",
                self.disk_motor_on,
                self.transfer_busy,
                self.disk_io_events,
                self.audio.audio_writes,
                self.audio.freq,
                self.audio.volume_env.output(),
                self.audio.mod_freq,
                self.audio.mod_env.output(),
                self.audio.mod_accum
            ),
            false,
            vec![0],
            vec![0],
            "fds".to_string(),
            self.irq_pending(),
        )
    }
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut v = vec![
            self.irq_enabled as u8,
            self.irq_repeat as u8,
            self.irq_pending_flag as u8,
            self.master_io_enable as u8,
            self.disk_motor_on as u8,
            self.transfer_busy as u8,
        ];
        v.extend_from_slice(&self.irq_reload.to_le_bytes());
        v.extend_from_slice(&self.irq_counter.to_le_bytes());
        v.extend_from_slice(&self.transfer_cycles_remaining.to_le_bytes());
        v.extend_from_slice(&self.disk_byte_index.to_le_bytes());
        v.extend_from_slice(&self.disk_io_events.to_le_bytes());
        v.extend_from_slice(&self.disk_regs);
        v.push(self.direct_boot_vectors as u8);
        self.audio.snapshot_bytes(&mut v);
        v.extend_from_slice(&self.bios_rom);
        v.extend_from_slice(&self.prg_ram);
        v.extend_from_slice(&self.chr_ram);
        if let Some(disk) = &self.disk {
            v.extend_from_slice(&disk.raw);
        }
        v
    }
}

fn normalize_fds_bios(prg_rom: Vec<u8>) -> Vec<u8> {
    let mut bios = vec![0; 8 * 1024];
    if prg_rom.is_empty() {
        return bios;
    }
    let src = if prg_rom.len() >= bios.len() {
        &prg_rom[prg_rom.len() - bios.len()..]
    } else {
        &prg_rom[..]
    };
    let start = bios.len() - src.len();
    bios[start..].copy_from_slice(src);
    bios
}

/// Probe-only mapper used when KUROSAKI knows the mapper number but does not yet
/// have a board-accurate clean-room implementation.
///
/// This mapper intentionally keeps behavior simple and noisy: PRG is exposed as
/// fixed 16K/32K windows, CHR is exposed as CHR-ROM/CHR-RAM, and all writes into
/// mapper-controlled space are traced as probe events. This lets CLI tools,
/// Python bindings, and CI reports inspect arbitrary mapper ROMs without
/// pretending that the mapper is accurately emulated.
#[derive(Debug, Clone)]
pub struct GenericProbeMapper {
    mapper: u16,
    spec: MapperSpec,
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    mirroring: Mirroring,
    mapper_writes: u64,
    last_write_addr: u16,
    last_write_value: u8,
}

impl GenericProbeMapper {
    pub fn new(mapper: u16, prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        let spec = mapper_spec(mapper);
        let (chr, chr_ram) = ensure_chr(chr_rom);
        Self {
            mapper,
            spec,
            prg_rom,
            prg_ram: vec![0; 32 * 1024],
            chr,
            chr_ram,
            mirroring,
            mapper_writes: 0,
            last_write_addr: 0,
            last_write_value: 0,
        }
    }

    fn fixed_prg_read(&self, addr: u16) -> u8 {
        if self.prg_rom.is_empty() {
            return 0;
        }
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr as usize - 0x6000) % self.prg_ram.len()],
            0x8000..=0xBFFF => read_bank(&self.prg_rom, 16 * 1024, 0, addr as usize - 0x8000),
            0xC000..=0xFFFF => read_bank(
                &self.prg_rom,
                16 * 1024,
                bank_count(self.prg_rom.len(), 16 * 1024) - 1,
                addr as usize - 0xC000,
            ),
            _ => 0,
        }
    }
}

impl Mapper for GenericProbeMapper {
    fn mapper_id(&self) -> u16 {
        self.mapper
    }
    fn mapper_name(&self) -> &'static str {
        "Generic probe-only mapper"
    }

    fn physical_prg_bank_8k(&self, addr: u16) -> Option<u16> {
        let (bank, offset) = match addr {
            0x8000..=0xBFFF => (0, addr as usize - 0x8000),
            0xC000..=0xFFFF => (
                bank_count(self.prg_rom.len(), 16 * 1024).saturating_sub(1),
                addr as usize - 0xC000,
            ),
            _ => return None,
        };
        physical_bank_8k(self.prg_rom.len(), 16 * 1024, bank, offset)
    }

    fn read_prg(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u8 {
        let value = self.fixed_prg_read(addr);
        if cfg.mapper && addr >= 0x8000 {
            let mut event = TraceEvent::new("mapper.probe_prg_read", frame, cycle);
            event.addr = Some(addr);
            event.value = Some(value);
            event.message = Some(format!("Probe-only mapper {} read using fixed windows; real board behavior is not emulated", self.mapper));
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
        match addr {
            0x6000..=0x7FFF => {
                let idx = (addr as usize - 0x6000) % self.prg_ram.len();
                self.prg_ram[idx] = value;
            }
            0x8000..=0xFFFF | 0x4018..=0x5FFF => {
                self.mapper_writes = self.mapper_writes.saturating_add(1);
                self.last_write_addr = addr;
                self.last_write_value = value;
                if cfg.mapper || cfg.mem_write {
                    let mut event = TraceEvent::new("mapper.probe_write", frame, cycle);
                    event.addr = Some(addr);
                    event.value = Some(value);
                    event.severity = Some("warn".to_string());
                    event.message = Some(format!("Mapper {} ({}) is probe-only; write observed but not interpreted as real bank/IRQ/audio behavior", self.mapper, self.spec.name));
                    sink.push(event);
                }
            }
            _ => {}
        }
    }

    fn read_chr(&mut self, addr: u16) -> u8 {
        self.chr[(addr as usize) % self.chr.len()]
    }
    fn write_chr(&mut self, addr: u16, value: u8) {
        if self.chr_ram {
            let len = self.chr.len();
            self.chr[(addr as usize) % len] = value;
        }
    }

    fn debug_state(&self) -> MapperDebugState {
        mapper_debug_state(
            self.mapper,
            format!(
                "{} probe-only writes={} last=${:04X}:{:02X}",
                self.spec.name, self.mapper_writes, self.last_write_addr, self.last_write_value
            ),
            true,
            if self.prg_rom.len() <= 16 * 1024 {
                vec![0, 0]
            } else {
                vec![0, (bank_count(self.prg_rom.len(), 16 * 1024) - 1) as u16]
            },
            vec![0],
            mirroring_name(self.mirroring),
            false,
        )
    }

    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&self.mapper.to_le_bytes());
        v.extend_from_slice(&self.mapper_writes.to_le_bytes());
        v.extend_from_slice(&self.last_write_addr.to_le_bytes());
        v.push(self.last_write_value);
        v.extend_from_slice(&self.prg_ram);
        if self.chr_ram {
            v.extend_from_slice(&self.chr);
        }
        v
    }
}

#[cfg(test)]
mod fds_audio_tests {
    use super::*;

    fn make_audible_fds() -> FdsAudioState {
        let mut fds = FdsAudioState::default();
        fds.write(0x4089, 0x80); // enable wavetable RAM writes while programming samples
        for i in 0..64u16 {
            let value = if i < 32 { 0 } else { 63 };
            fds.write(0x4040 + i, value);
        }
        fds.write(0x4089, 0x00); // disable writes and play at the loudest master volume
        fds.write(0x4082, 0x80);
        fds.write(0x4083, 0x01); // unhalt wave + volume envelope, high frequency bits = 1
        fds
    }

    #[test]
    fn fds_direct_volume_outputs_samples() {
        let mut fds = make_audible_fds();
        fds.write(0x4080, 0x80 | 0x20); // direct volume = 32
        let mut non_zero = false;
        for _ in 0..2048 {
            if fds.sample(44_100) != 0 {
                non_zero = true;
                break;
            }
        }
        assert!(non_zero);
        assert_eq!(fds.read(0x4090), 32);
    }

    #[test]
    fn fds_volume_envelope_clocks_up_and_down() {
        let mut fds = make_audible_fds();
        fds.write(0x408A, 0x00);
        fds.write(0x4080, 0x40); // envelope mode, increase, fastest local rate
        for _ in 0..256 {
            let _ = fds.sample(44_100);
        }
        assert!(fds.volume_env.output() > 0);

        fds.write(0x4080, 0x00); // envelope mode, decrease, fastest local rate
        for _ in 0..256 {
            let _ = fds.sample(44_100);
        }
        assert!(fds.volume_env.output() < 32);
    }

    #[test]
    fn fds_modulation_envelope_affects_pitch_unit() {
        let mut fds = make_audible_fds();
        fds.write(0x4080, 0x80 | 0x20);
        fds.write(0x4085, 0x00);
        for _ in 0..64 {
            fds.write(0x4088, 0x03); // positive modulation-table delta
        }
        fds.write(0x4084, 0x40); // modulation envelope, increase, fastest local rate
        fds.write(0x4086, 0xFF);
        fds.write(0x4087, 0x03); // unhalt modulation + modulation envelope
        for _ in 0..4096 {
            let _ = fds.sample(44_100);
        }
        assert!(fds.mod_env.output() > 0);
        assert!(fds.mod_accum > 0);
        assert_eq!(fds.read(0x4092), fds.mod_env.output());
    }
}
