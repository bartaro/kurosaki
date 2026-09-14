use crate::error::{KurosakiError, Result};
use crate::fds::FdsDiskImage;
use crate::hash::sha256_hex;
use crate::mapper_db::{mapper_spec, MapperSupportLevel};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const INES_MAGIC: &[u8; 4] = b"NES\x1A";
const PRG_ROM_BANK_SIZE: usize = 16 * 1024;
const CHR_ROM_BANK_SIZE: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HeaderKind {
    INes,
    Nes20,
    Fds,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mirroring {
    Horizontal,
    Vertical,
    FourScreen,
    MapperControlled,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// Header-derived metadata plus explicit inference labels and warnings. RAM
// totals may combine volatile and nonvolatile storage; battery import performs
// its own stricter layout checks.
pub struct RomInfo {
    pub path: Option<PathBuf>,
    pub file_size: usize,
    pub sha256: String,
    pub header_kind: HeaderKind,
    pub mapper: u16,
    pub submapper: u8,
    pub prg_rom_size: usize,
    pub chr_rom_size: usize,
    pub prg_ram_size: Option<usize>,
    pub chr_ram_size: Option<usize>,
    pub mirroring: Mirroring,
    pub battery: bool,
    pub trainer: bool,
    pub four_screen: bool,
    pub console_type: u8,
    pub region_hint: Option<String>,
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_profile_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_profile_confidence: Option<f32>,
}

#[derive(Debug, Clone)]
// Owned program/character payloads and optional trainer or disk state.
// The raw header is preserved so consumers can inspect original flag fields.
pub struct Cartridge {
    pub info: RomInfo,
    pub prg_rom: Vec<u8>,
    pub chr_rom: Vec<u8>,
    pub trainer: Option<Vec<u8>>,
    pub raw_header: [u8; 16],
    pub fds_disk: Option<FdsDiskImage>,
    pub fds_fast_boot_pc: Option<u16>,
}

impl Cartridge {
    // Read the complete input and prefer iNES magic over FDS detection. FDS files
    // require an external BIOS; other inputs use the iNES parser. Preserve the input
    // path only after parsing succeeds.
    pub fn load_file(path: impl AsRef<Path>) -> Result<Self> {
        let path_ref = path.as_ref();
        let bytes = fs::read(path_ref)?;
        let mut cart = if bytes.len() >= 4 && &bytes[0..4] == INES_MAGIC {
            Self::from_bytes(&bytes)?
        } else if looks_like_fds(path_ref, &bytes) {
            let bios = load_fds_bios(path_ref)?;
            Self::from_fds_bytes_with_bios(&bytes, bios)?
        } else {
            Self::from_bytes(&bytes)?
        };
        cart.info.path = Some(path_ref.to_path_buf());
        Ok(cart)
    }

    // Require an 8 KiB caller-supplied BIOS and parse disk blocks for direct boot.
    // The ROM fingerprint covers disk bytes only, not the BIOS; PRG-ROM holds the
    // BIOS while the disk object supplies the initial RAM contents.
    pub fn from_fds_bytes_with_bios(bytes: &[u8], bios_rom: Vec<u8>) -> Result<Self> {
        if bios_rom.len() != 8 * 1024 {
            return Err(KurosakiError::InvalidRom(format!(
                "disksys.rom must be 8192 bytes, got {}",
                bios_rom.len()
            )));
        }
        let disk = FdsDiskImage::from_bytes(bytes)?;
        let fast_boot_pc = disk.fast_boot_pc();
        let mut warnings = disk.warnings.clone();
        warnings.push(
            "FDS direct boot preloads boot files into PRG-RAM/CHR-RAM and maps disksys.rom at $E000-$FFFF; cycle-accurate BIOS disk media timing remains future work."
                .to_string(),
        );
        if let Some(pc) = fast_boot_pc {
            warnings.push(format!(
                "FDS direct boot reset PC resolved from $DFFC: ${pc:04X}"
            ));
        } else {
            warnings.push(
                "FDS direct boot could not resolve a reset vector from $DFFC; BIOS reset vector will be used."
                    .to_string(),
            );
        }

        let mut raw_header = [0u8; 16];
        let n = bytes.len().min(16);
        raw_header[..n].copy_from_slice(&bytes[..n]);

        let info = RomInfo {
            path: None,
            file_size: bytes.len(),
            sha256: sha256_hex(bytes),
            header_kind: HeaderKind::Fds,
            mapper: 20,
            submapper: 0,
            prg_rom_size: bios_rom.len(),
            chr_rom_size: 0,
            prg_ram_size: Some(32 * 1024),
            chr_ram_size: Some(8 * 1024),
            mirroring: Mirroring::MapperControlled,
            battery: true,
            trainer: false,
            four_screen: false,
            console_type: 3,
            region_hint: Some("ntsc_fds".to_string()),
            warnings,
            board_profile: None,
            board_profile_source: None,
            board_profile_confidence: None,
        };

        Ok(Self {
            info,
            prg_rom: bios_rom,
            chr_rom: Vec::new(),
            trainer: None,
            raw_header,
            fds_disk: Some(disk),
            fds_fast_boot_pc: fast_boot_pc,
        })
    }

    // Validate the iNES header and declared trainer/ROM payload lengths, then
    // copy the payloads and derive metadata. Extra trailing bytes are accepted and
    // participate in the file hash; no CPU code is executed by this parser.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 16 {
            return Err(KurosakiError::InvalidRom(
                "file is smaller than iNES header".to_string(),
            ));
        }
        if &bytes[0..4] != INES_MAGIC {
            return Err(KurosakiError::InvalidRom(
                "missing NES<EOF> magic".to_string(),
            ));
        }

        let mut raw_header = [0u8; 16];
        raw_header.copy_from_slice(&bytes[0..16]);

        let flags6 = raw_header[6];
        let flags7 = raw_header[7];
        let header_kind = if (flags7 & 0x0C) == 0x08 {
            HeaderKind::Nes20
        } else {
            HeaderKind::INes
        };

        let trainer_present = flags6 & 0x04 != 0;
        let four_screen = flags6 & 0x08 != 0;
        let mirroring = if four_screen {
            Mirroring::FourScreen
        } else if flags6 & 0x01 != 0 {
            Mirroring::Vertical
        } else {
            Mirroring::Horizontal
        };
        let battery = flags6 & 0x02 != 0;

        let mut warnings = Vec::new();
        let mapper = match header_kind {
            HeaderKind::INes => ((flags6 >> 4) as u16) | ((flags7 & 0xF0) as u16),
            HeaderKind::Nes20 => {
                ((flags6 >> 4) as u16)
                    | ((flags7 & 0xF0) as u16)
                    | (((raw_header[8] & 0x0F) as u16) << 8)
            }
            HeaderKind::Fds => unreachable!("iNES parser never assigns FDS header kind"),
        };
        let submapper = if header_kind == HeaderKind::Nes20 {
            raw_header[8] >> 4
        } else {
            0
        };

        let (prg_rom_size, chr_rom_size) = match header_kind {
            HeaderKind::INes => (
                raw_header[4] as usize * PRG_ROM_BANK_SIZE,
                raw_header[5] as usize * CHR_ROM_BANK_SIZE,
            ),
            HeaderKind::Nes20 => (
                nes20_rom_size(
                    raw_header[4],
                    raw_header[9] & 0x0F,
                    PRG_ROM_BANK_SIZE,
                    &mut warnings,
                    "PRG-ROM",
                ),
                nes20_rom_size(
                    raw_header[5],
                    raw_header[9] >> 4,
                    CHR_ROM_BANK_SIZE,
                    &mut warnings,
                    "CHR-ROM",
                ),
            ),
            HeaderKind::Fds => unreachable!("iNES parser never assigns FDS header kind"),
        };

        // A present trainer consumes 512 bytes before PRG. Preserve it separately;
        // this parser does not copy trainer data into emulated RAM.
        let mut offset = 16usize;
        let trainer = if trainer_present {
            if bytes.len() < offset + 512 {
                return Err(KurosakiError::InvalidRom(
                    "trainer flag is set but file is truncated".to_string(),
                ));
            }
            let t = bytes[offset..offset + 512].to_vec();
            offset += 512;
            Some(t)
        } else {
            None
        };

        if bytes.len() < offset + prg_rom_size + chr_rom_size {
            return Err(KurosakiError::InvalidRom(format!(
                "file is truncated: expected at least {} bytes, got {}",
                offset + prg_rom_size + chr_rom_size,
                bytes.len()
            )));
        }

        let prg_rom = bytes[offset..offset + prg_rom_size].to_vec();
        offset += prg_rom_size;
        let chr_rom = bytes[offset..offset + chr_rom_size].to_vec();

        // Legacy zero PRG-RAM banks imply one 8 KiB bank; NES 2.0 sums the two
        // RAM nibbles without retaining their volatility split in this total.
        let prg_ram_size = match header_kind {
            HeaderKind::INes => {
                let banks = if raw_header[8] == 0 {
                    1
                } else {
                    raw_header[8] as usize
                };
                Some(banks * 8 * 1024)
            }
            HeaderKind::Nes20 => Some(
                nes20_shift_size(raw_header[10] & 0x0F) + nes20_shift_size(raw_header[10] >> 4),
            ),
            HeaderKind::Fds => unreachable!("iNES parser never assigns FDS header kind"),
        };
        let chr_ram_size = match header_kind {
            HeaderKind::INes => {
                if chr_rom_size == 0 {
                    Some(8 * 1024)
                } else {
                    Some(0)
                }
            }
            HeaderKind::Nes20 => Some(
                nes20_shift_size(raw_header[11] & 0x0F) + nes20_shift_size(raw_header[11] >> 4),
            ),
            HeaderKind::Fds => unreachable!("iNES parser never assigns FDS header kind"),
        };

        // Expose the header region as a hint. Choosing the emulator clock policy
        // is a separate concern from reading this field.
        let region_hint = match header_kind {
            HeaderKind::INes => match raw_header[9] & 0x01 {
                0 => Some("ntsc_or_unspecified".to_string()),
                _ => Some("pal_hint".to_string()),
            },
            HeaderKind::Nes20 => match raw_header[12] & 0x03 {
                0 => Some("ntsc".to_string()),
                1 => Some("pal".to_string()),
                2 => Some("multi_region".to_string()),
                _ => Some("dendy".to_string()),
            },
            HeaderKind::Fds => unreachable!("iNES parser never assigns FDS header kind"),
        };

        let mapper_support = mapper_spec(mapper);
        if matches!(
            mapper_support.support,
            MapperSupportLevel::ProbeOnly | MapperSupportLevel::Unknown
        ) {
            warnings.push(format!(
                "Mapper {mapper} is registered as {support:?}; KUROSAKI will use the generic probe-only mapper for execution attempts, so bank/IRQ/audio behavior is not accurate yet",
                support = mapper_support.support
            ));
        }
        let (board_profile, board_profile_source, board_profile_confidence) = infer_board_profile(
            mapper,
            prg_rom_size,
            chr_rom_size,
            prg_ram_size,
            chr_ram_size,
        );
        if chr_rom_size == 0 && board_profile.as_deref() != Some("surom512") {
            warnings.push("CHR-RAM cartridge detected; PPU CHR-RAM write path is scaffolded but not accuracy-complete yet".to_string());
        }

        let info = RomInfo {
            path: None,
            file_size: bytes.len(),
            sha256: sha256_hex(bytes),
            header_kind,
            mapper,
            submapper,
            prg_rom_size,
            chr_rom_size,
            prg_ram_size,
            chr_ram_size,
            mirroring,
            battery,
            trainer: trainer_present,
            four_screen,
            console_type: raw_header[7] & 0x03,
            region_hint,
            warnings,
            board_profile,
            board_profile_source,
            board_profile_confidence,
        };

        Ok(Self {
            info,
            prg_rom,
            chr_rom,
            trainer,
            raw_header,
            fds_disk: None,
            fds_fast_boot_pc: None,
        })
    }

    // Serialize metadata alone as indented JSON; ROM, trainer and disk payloads
    // are not included in this report.
    pub fn to_info_json_pretty(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(&self.info)?)
    }
}

// Recognize the exact mapper-1, 512 KiB PRG-ROM and 8 KiB PRG/CHR-RAM size
// combination as surom512. Confidence is a fixed label for this size rule, not
// a measured probability or verification of physical board wiring.
fn infer_board_profile(
    mapper: u16,
    prg_rom_size: usize,
    chr_rom_size: usize,
    prg_ram_size: Option<usize>,
    chr_ram_size: Option<usize>,
) -> (Option<String>, Option<String>, Option<f32>) {
    if mapper == 1
        && prg_rom_size == 512 * 1024
        && chr_rom_size == 0
        && prg_ram_size == Some(8 * 1024)
        && chr_ram_size == Some(8 * 1024)
    {
        return (
            Some("surom512".to_string()),
            Some("size_inference".to_string()),
            Some(1.0),
        );
    }
    (None, None, None)
}

// Recognize the case-insensitive .fds extension or disk magic. This is only
// format dispatch; malformed candidates are handled by the disk parser.
fn looks_like_fds(path: &Path, bytes: &[u8]) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("fds"))
        .unwrap_or(false)
        || (bytes.len() >= 4 && &bytes[0..4] == b"FDS\x1A")
}

// Search the environment override, disk directory, working directory and
// executable directory in that order. The first existing file either supplies
// 8192 bytes or causes an error; an invalid candidate does not fall through.
fn load_fds_bios(disk_path: &Path) -> Result<Vec<u8>> {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("KUROSAKI_DISKSYS_ROM") {
        candidates.push(PathBuf::from(path));
    }
    if let Some(parent) = disk_path.parent() {
        candidates.push(parent.join("disksys.rom"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("disksys.rom"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("disksys.rom"));
        }
    }

    for candidate in candidates {
        if candidate.is_file() {
            let bios = fs::read(&candidate)?;
            if bios.len() == 8 * 1024 {
                return Ok(bios);
            }
            return Err(KurosakiError::InvalidRom(format!(
                "found disksys.rom at {}, but it is {} bytes instead of 8192",
                candidate.display(),
                bios.len()
            )));
        }
    }

    Err(KurosakiError::InvalidRom(
        "FDS image needs disksys.rom; place it next to the disk image, in the current KUROSAKI directory, or set KUROSAKI_DISKSYS_ROM".to_string(),
    ))
}

// Decode ordinary bank counts using the supplied unit. The high-nibble
// C..F branch is the existing approximate size fallback: it ignores lsb and
// derives exponent/multiplier from that nibble. Do not treat it as an exact
// NES 2.0 exponent decoder; the returned size controls subsequent slicing.
fn nes20_rom_size(
    lsb: u8,
    msb_or_exp: u8,
    unit: usize,
    warnings: &mut Vec<String>,
    label: &str,
) -> usize {
    if msb_or_exp & 0x0C == 0x0C {
        let exponent = (msb_or_exp >> 2) as usize;
        let multiplier = ((msb_or_exp & 0x03) as usize) * 2 + 1;
        warnings.push(format!(
            "NES 2.0 exponent/multiplier size encoding used for {label}; decoded using approximate formula"
        ));
        (1usize << exponent).saturating_mul(multiplier)
    } else {
        (((msb_or_exp as usize) << 8) | lsb as usize) * unit
    }
}

// Decode zero as absent RAM and a nonzero nibble as 64 shifted by that code.
// Callers combine volatile and nonvolatile sizes for the metadata total.
fn nes20_shift_size(code: u8) -> usize {
    if code == 0 {
        0
    } else {
        64usize << code
    }
}
