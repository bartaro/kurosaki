//! Mapper registry and support status database.
//!
//! KUROSAKI keeps this database separate from mapper execution. The goal is to
//! make every iNES/NES 2.0 mapper number visible to the CLI and diagnostics even
//! before a precise clean-room implementation exists.

use serde::{Deserialize, Serialize};

pub const NES20_MAPPER_ID_MAX: u16 = 4095;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MapperSupportLevel {
    /// Implemented well enough for KITAQFC smoke/golden tests in headless mode.
    Implemented,
    /// Has a mapper-specific scaffold, but needs timing/edge-case fixtures.
    Scaffold,
    /// Can be loaded through the generic probe mapper for inspection/tracing only.
    ProbeOnly,
    /// Mapper number is outside the registry range or not classified yet.
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MapperFamily {
    Nrom,
    DiscreteLogic,
    Mmc,
    Vrc,
    Namco,
    Sunsoft,
    Bandai,
    Jaleco,
    Fds,
    Unlicensed,
    Homebrew,
    Multicart,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapperSpec {
    pub mapper: u16,
    pub name: String,
    pub family: MapperFamily,
    pub support: MapperSupportLevel,
    pub prg_bank_granularity_kb: Option<u16>,
    pub chr_bank_granularity_kb: Option<u16>,
    pub has_irq: bool,
    pub has_expansion_audio: bool,
    pub has_battery: bool,
    pub notes: String,
}

impl MapperSpec {
    pub fn generic_probe(mapper: u16) -> Self {
        Self {
            mapper,
            name: format!("Mapper {mapper} probe-only"),
            family: MapperFamily::Unknown,
            support: MapperSupportLevel::ProbeOnly,
            prg_bank_granularity_kb: None,
            chr_bank_granularity_kb: None,
            has_irq: false,
            has_expansion_audio: false,
            has_battery: false,
            notes: "No clean-room board implementation is attached yet; KUROSAKI will use a fixed-window probe mapper for inspection, trace, and diagnostics only.".to_string(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spec(
    mapper: u16,
    name: &str,
    family: MapperFamily,
    support: MapperSupportLevel,
    prg_kb: Option<u16>,
    chr_kb: Option<u16>,
    has_irq: bool,
    has_expansion_audio: bool,
    has_battery: bool,
    notes: &str,
) -> MapperSpec {
    MapperSpec {
        mapper,
        name: name.to_string(),
        family,
        support,
        prg_bank_granularity_kb: prg_kb,
        chr_bank_granularity_kb: chr_kb,
        has_irq,
        has_expansion_audio,
        has_battery,
        notes: notes.to_string(),
    }
}

/// Returns the current KUROSAKI mapper registry entry.
///
/// This is deliberately conservative. It does not claim exactness unless a
/// KUROSAKI mapper implementation and fixture path exist.
pub fn mapper_spec(mapper: u16) -> MapperSpec {
    use MapperFamily::*;
    use MapperSupportLevel::*;
    match mapper {
        0 => spec(0, "NROM", Nrom, Implemented, Some(16), Some(8), false, false, false, "Mapper 0 fixed PRG/CHR mapping is implemented."),
        1 => spec(1, "MMC1 / SxROM", Mmc, Implemented, Some(16), Some(4), false, false, true, "MMC1 shift-register PRG/CHR banking scaffold is implemented for headless KITAQFC tests."),
        2 => spec(2, "UNROM / UOROM", DiscreteLogic, Implemented, Some(16), None, false, false, false, "16 KiB switchable PRG bank plus fixed upper bank is implemented."),
        3 => spec(3, "CNROM", DiscreteLogic, Implemented, Some(32), Some(8), false, false, false, "8 KiB CHR bank switching is implemented."),
        4 => spec(4, "MMC3 / MMC6", Mmc, Scaffold, Some(8), Some(1), true, false, true, "PRG/CHR banking and A12-driven IRQ scaffold exist; timing is fixture-driven and not yet hardware-validated."),
        5 => spec(5, "MMC5 / ExROM", Mmc, Scaffold, Some(8), Some(1), true, true, true, "MMC5 scaffold implements PRG/CHR register observation, ExRAM, scanline IRQ scaffold, multiplier, and expansion-audio register tracing; split-screen and exact ExRAM modes remain fixture work."),
        7 => spec(7, "AxROM", DiscreteLogic, Implemented, Some(32), None, false, false, false, "32 KiB PRG switching and single-screen mirroring state are implemented."),
        9 => spec(9, "MMC2 / PxROM", Mmc, Scaffold, Some(8), Some(4), false, false, false, "Latch-controlled 4 KiB CHR banking and fixed PRG windows are implemented; edge timing remains fixture work."),
        10 => spec(10, "MMC4 / FxROM", Mmc, Scaffold, Some(16), Some(4), false, false, false, "Latch-controlled 4 KiB CHR banking and fixed PRG windows are implemented; edge timing remains fixture work."),
        11 => spec(11, "Color Dreams", Unlicensed, Implemented, Some(32), Some(8), false, false, false, "Simple Color Dreams PRG/CHR register mapping is implemented; board-specific security variants remain outside this profile."),
        13 => spec(13, "CPROM", DiscreteLogic, Implemented, Some(32), Some(4), false, false, false, "32 KiB fixed PRG with switchable upper 4 KiB CHR-RAM bank is implemented."),
        16 => spec(16, "Bandai FCG", Bandai, Scaffold, Some(16), Some(1), true, false, true, "Bandai FCG scaffold implements PRG/CHR bank registers, IRQ counter scaffold, and EEPROM event observation; EEPROM protocol details remain fixture work."),
        18 => spec(18, "Jaleco SS88006", Jaleco, Scaffold, Some(8), Some(1), true, false, true, "Jaleco SS88006 scaffold implements PRG/CHR register observation and IRQ counter scaffold; nibble wiring still needs board fixtures."),
        19 => spec(19, "Namco 163", Namco, Scaffold, Some(8), Some(1), true, true, true, "Namco 163 scaffold implements PRG/CHR bank registers, internal RAM/audio register observation, and IRQ counter scaffold; exact expansion audio remains fixture work."),
        20 => spec(20, "Famicom Disk System", Fds, Scaffold, Some(32), None, true, true, true, "FDS register/timer/disk-event scaffold exists and FDS wavetable audio now includes volume envelope, modulation envelope, master envelope speed, and first-pass pitch modulation; BIOS/media timing remains fixture work."),
        21 => spec(21, "Konami VRC4a/VRC4c", Vrc, Scaffold, Some(8), Some(1), true, false, false, "VRC family scaffold provides PRG/CHR register observation and IRQ skeleton; exact address-line variant mapping remains fixture work."),
        22 => spec(22, "Konami VRC2a", Vrc, Scaffold, Some(8), Some(1), false, false, false, "VRC2 scaffold provides PRG/CHR register observation; exact variant wiring remains fixture work."),
        23 => spec(23, "Konami VRC2b/VRC4e", Vrc, Scaffold, Some(8), Some(1), true, false, false, "VRC2/VRC4 scaffold provides PRG/CHR register observation and IRQ skeleton where applicable; exact variant wiring remains fixture work."),
        24 => spec(24, "Konami VRC6a", Vrc, Scaffold, Some(8), Some(1), true, true, false, "VRC6 scaffold provides bank registers, IRQ skeleton, expansion-audio register tracing, and first-pass 2-pulse + saw waveform generation."),
        25 => spec(25, "Konami VRC4b/VRC4d", Vrc, Scaffold, Some(8), Some(1), true, false, false, "VRC4 scaffold provides PRG/CHR register observation and IRQ skeleton; exact address-line variant mapping remains fixture work."),
        26 => spec(26, "Konami VRC6b", Vrc, Scaffold, Some(8), Some(1), true, true, false, "VRC6 scaffold provides bank registers, IRQ skeleton, expansion-audio register tracing, and first-pass 2-pulse + saw waveform generation."),
        30 => spec(30, "UNROM 512", Homebrew, Scaffold, Some(16), None, false, false, true, "Can share much of UXROM behavior, but flash/mirroring/submapper semantics need fixtures."),
        32 => spec(32, "Irem G-101", DiscreteLogic, Scaffold, Some(8), Some(1), false, false, false, "Board-family scaffold provides PRG/CHR register observation; Irem-specific mode bits remain fixture work."),
        33 => spec(33, "Taito TC0190/TC0350", DiscreteLogic, Scaffold, Some(8), Some(1), false, false, false, "Board-family scaffold provides PRG/CHR register observation; exact Taito register behavior remains fixture work."),
        34 => spec(34, "BNROM / NINA-001", DiscreteLogic, Scaffold, Some(32), Some(4), false, false, false, "BNROM submapper 0 32 KiB PRG mapping is implemented; NINA variants remain scaffold-level, so the mapper family stays conservatively classified."),
        48 => spec(48, "Taito TC0690", DiscreteLogic, Scaffold, Some(8), Some(1), true, false, false, "Board-family scaffold provides PRG/CHR register observation and IRQ skeleton; exact Taito behavior remains fixture work."),
        64 => spec(64, "RAMBO-1", DiscreteLogic, Scaffold, Some(8), Some(1), true, false, false, "Board-family scaffold provides register observation and IRQ skeleton; RAMBO-1 details remain fixture work."),
        66 => spec(66, "GxROM / MHROM", DiscreteLogic, Implemented, Some(32), Some(8), false, false, false, "32 KiB PRG and 8 KiB CHR bank registers are implemented."),
        68 => spec(68, "Sunsoft 4", Sunsoft, Scaffold, Some(16), Some(2), false, false, false, "Board-family scaffold provides register observation; Sunsoft 4 nametable behavior remains fixture work."),
        69 => spec(69, "Sunsoft FME-7 / 5B", Sunsoft, Scaffold, Some(8), Some(1), true, true, true, "Sunsoft FME-7/5B scaffold implements command/parameter banking, IRQ skeleton, and 5B audio register tracing."),
        71 => spec(71, "Camerica/Codemasters", Unlicensed, Scaffold, Some(16), None, false, false, false, "UXROM-like scaffold can cover simple PRG switching cases; board variants need fixtures."),
        73 => spec(73, "Konami VRC3", Vrc, Scaffold, Some(16), None, true, false, false, "Dedicated VRC3 16 KiB PRG windows and latch/counter IRQ execution are implemented; timing modes need hardware fixtures."),
        75 => spec(75, "Konami VRC1", Vrc, Scaffold, Some(8), Some(4), false, false, false, "VRC1 scaffold provides PRG/CHR register observation; exact wiring remains fixture work."),
        76 => spec(76, "Namco 109 variant", Namco, Scaffold, Some(8), Some(2), false, false, false, "Board-family scaffold provides register observation; exact Namco 109 wiring remains fixture work."),
        79 => spec(79, "NINA-03/NINA-06", Unlicensed, Implemented, Some(32), Some(8), false, false, false, "NINA register writes in $4100-$5FFF select 32 KiB PRG and 8 KiB CHR banks."),
        85 => spec(85, "Konami VRC7", Vrc, Scaffold, Some(8), Some(1), true, true, false, "VRC7 scaffold provides bank registers, IRQ skeleton, FM audio register handling, and debugger-grade six-channel FM synthesis; YM2413/OPLL-exact behavior remains fixture work."),
        87 => spec(87, "Jaleco JF-13", Jaleco, Scaffold, Some(32), Some(8), false, false, false, "Board-family scaffold provides register observation; exact CHR bit wiring remains fixture work."),
        94 => spec(94, "UN1ROM", DiscreteLogic, Implemented, Some(16), None, false, false, false, "UN1ROM shifted 16 KiB PRG bank register is implemented; board-specific mirroring variants remain outside this profile."),
        118 => spec(118, "TLSROM / TKSROM", Mmc, Scaffold, Some(8), Some(1), true, false, false, "MMC3 execution with CHR A17 nametable mirroring; board-specific polarity still needs fixtures."),
        119 => spec(119, "TQROM", Mmc, Scaffold, Some(8), Some(1), true, false, false, "MMC3 execution with CHR-ROM/CHR-RAM mixing through CHR A16; exact board RAM size needs fixtures."),
        159 => spec(159, "Bandai FCG with EEPROM", Bandai, Scaffold, Some(16), Some(1), true, false, true, "Bandai FCG scaffold with EEPROM event observation; precise EEPROM protocol remains fixture work."),
        180 => spec(180, "Crazy Climber", DiscreteLogic, Implemented, Some(16), None, false, false, false, "Fixed lower 16 KiB with switchable upper 16 KiB PRG mapping is implemented."),
        185 => spec(185, "CNROM security variants", DiscreteLogic, ProbeOnly, Some(32), Some(8), false, false, false, "Needs open bus/security behavior fixtures."),
        206 => spec(206, "Namco 118 / DxROM", Namco, Scaffold, Some(8), Some(1), false, false, false, "Board-family scaffold provides register observation; exact Namco 118 behavior remains fixture work."),
        210 => spec(210, "Namco 175/340", Namco, Scaffold, Some(16), Some(8), false, false, true, "Board-family scaffold provides register observation; exact Namco 175/340 details remain fixture work."),
        218 => spec(218, "Magic Floor", Homebrew, ProbeOnly, Some(32), None, false, false, false, "Homebrew mapper; needs fixture."),
        _ => MapperSpec::generic_probe(mapper),
    }
}

pub fn all_mapper_specs() -> Vec<MapperSpec> {
    (0..=NES20_MAPPER_ID_MAX).map(mapper_spec).collect()
}

pub fn implemented_mapper_specs() -> Vec<MapperSpec> {
    all_mapper_specs()
        .into_iter()
        .filter(|s| {
            matches!(
                s.support,
                MapperSupportLevel::Implemented | MapperSupportLevel::Scaffold
            )
        })
        .collect()
}
