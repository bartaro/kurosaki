use crate::error::{KurosakiError, Result};
use serde::{Deserialize, Serialize};

pub const FDS_HEADER_SIZE: usize = 16;
pub const FDS_SIDE_SIZE: usize = 65_500;
pub const FDS_PRG_RAM_BASE: u16 = 0x6000;
pub const FDS_PRG_RAM_SIZE: usize = 0x8000;
pub const FDS_CHR_RAM_SIZE: usize = 0x2000;

const FDS_MAGIC: &[u8; 4] = b"FDS\x1A";

#[derive(Debug, Clone, Serialize, Deserialize)]
// One complete parsed disk file with its side and boot-ID classification.
// Payload bytes are omitted from serialization; this record is not a saved
// copy of the disk media.
pub struct FdsDiskFile {
    pub side: u8,
    pub number: u8,
    pub id: u8,
    pub name: String,
    pub load_address: u16,
    pub size: u16,
    pub file_type: u8,
    pub boot: bool,
    #[serde(skip_serializing)]
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// Parsed file inventory and warnings with original input bytes retained
// in memory. Raw media is omitted from serialization.
pub struct FdsDiskImage {
    pub side_count: u8,
    pub has_header: bool,
    pub files: Vec<FdsDiskFile>,
    pub warnings: Vec<String>,
    #[serde(skip_serializing)]
    pub raw: Vec<u8>,
}

impl FdsDiskImage {
    // Parse optional-header FDS data into ordered file records while retaining
    // the original bytes. Honor a nonzero header side count; otherwise infer a
    // ceiling count from length. Incomplete sides produce warnings, not padding.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.is_empty() {
            return Err(KurosakiError::InvalidRom("empty FDS image".to_string()));
        }
        let has_header = bytes.len() >= FDS_HEADER_SIZE && &bytes[0..4] == FDS_MAGIC;
        let data_offset = if has_header { FDS_HEADER_SIZE } else { 0 };
        if bytes.len() <= data_offset {
            return Err(KurosakiError::InvalidRom(
                "FDS image has no side data".to_string(),
            ));
        }

        let available_side_bytes = bytes.len() - data_offset;
        let inferred_sides = available_side_bytes.div_ceil(FDS_SIDE_SIZE).max(1);
        let header_sides = if has_header && bytes[4] != 0 {
            bytes[4] as usize
        } else {
            inferred_sides
        };
        // Cap representable sides at 255. A header count can exceed the available
        // sides or exclude extra trailing sides; parsing stops when input runs out.
        let side_count = header_sides.min(255);
        let mut warnings = Vec::new();
        if available_side_bytes < side_count * FDS_SIDE_SIZE {
            warnings.push(format!(
                "FDS image is shorter than {} full side(s); missing tail bytes are treated as absent",
                side_count
            ));
        }

        let mut files = Vec::new();
        for side in 0..side_count {
            let start = data_offset + side * FDS_SIDE_SIZE;
            if start >= bytes.len() {
                break;
            }
            let end = (start + FDS_SIDE_SIZE).min(bytes.len());
            parse_side(side as u8, &bytes[start..end], &mut files, &mut warnings)?;
        }

        Ok(Self {
            side_count: side_count as u8,
            has_header,
            files,
            warnings,
            raw: bytes.to_vec(),
        })
    }

    // Build a 32 KiB FF-filled direct-boot image from every boot PRG file in
    // parse order, including files on later sides. Skip starts below $6000, clip
    // the upper boundary and let later files overwrite overlaps, then fix the
    // recognized KITAQFC NMI-vector stub without running it.
    pub fn boot_prg_ram(&self) -> Vec<u8> {
        let mut prg = vec![0xFF; FDS_PRG_RAM_SIZE];
        for file in self.files.iter().filter(|f| f.boot && f.file_type == 0) {
            if file.load_address < FDS_PRG_RAM_BASE {
                continue;
            }
            let offset = (file.load_address - FDS_PRG_RAM_BASE) as usize;
            if offset >= prg.len() {
                continue;
            }
            let len = file.data.len().min(prg.len() - offset);
            prg[offset..offset + len].copy_from_slice(&file.data[..len]);
        }
        apply_license_bypass_vector_fixup(&mut prg);
        prg
    }

    // Build zero-filled 8 KiB CHR RAM from boot files of type 1 or 2 on all
    // parsed sides. Use load addresses directly, clip at the upper boundary and
    // let later records replace overlapping bytes.
    pub fn boot_chr_ram(&self) -> Vec<u8> {
        let mut chr = vec![0; FDS_CHR_RAM_SIZE];
        for file in self
            .files
            .iter()
            .filter(|f| f.boot && matches!(f.file_type, 1 | 2))
        {
            let offset = file.load_address as usize;
            if offset >= chr.len() {
                continue;
            }
            let len = file.data.len().min(chr.len() - offset);
            chr[offset..offset + len].copy_from_slice(&file.data[..len]);
        }
        chr
    }

    // Rebuild the direct-boot PRG image and read its little-endian $DFFC vector.
    // Reject only $0000 and $FFFF; other targets are returned without code validation.
    pub fn fast_boot_pc(&self) -> Option<u16> {
        let prg = self.boot_prg_ram();
        let offset = 0xDFFCusize - FDS_PRG_RAM_BASE as usize;
        if offset + 1 >= prg.len() {
            return None;
        }
        let pc = u16::from_le_bytes([prg[offset], prg[offset + 1]]);
        if pc == 0x0000 || pc == 0xFFFF {
            None
        } else {
            Some(pc)
        }
    }
}

// Recognize selected opcode fields of the KITAQFC $DFC0 startup stub when
// the NMI vector points there and reset does not. Copy its immediate NMI target
// into $DFFA; leave all code and reset bytes intact. The caller supplies the
// full boot RAM image; this is pattern recognition, not instruction execution.
fn apply_license_bypass_vector_fixup(prg: &mut [u8]) {
    let Some(stub_offset) = fds_prg_offset(0xDFC0) else {
        return;
    };
    let Some(nmi_vector_offset) = fds_prg_offset(0xDFFA) else {
        return;
    };
    let Some(reset_vector_offset) = fds_prg_offset(0xDFFC) else {
        return;
    };

    if stub_offset + 0x1E > prg.len() || nmi_vector_offset + 1 >= prg.len() {
        return;
    }

    let nmi_vector = u16::from_le_bytes([prg[nmi_vector_offset], prg[nmi_vector_offset + 1]]);
    let reset_vector = u16::from_le_bytes([prg[reset_vector_offset], prg[reset_vector_offset + 1]]);
    if nmi_vector != 0xDFC0 || reset_vector == 0xDFC0 {
        return;
    }

    let stub = &prg[stub_offset..];
    let looks_like_kitaqfc_bypass = stub[0x00] == 0xA9
        && stub[0x01] == 0x00
        && stub[0x02..0x05] == [0x8D, 0x00, 0x20]
        && stub[0x05..0x07] == [0x85, 0xFF]
        && stub[0x07] == 0xA9
        && stub[0x09..0x0C] == [0x8D, 0xFA, 0xDF]
        && stub[0x0C] == 0xA9
        && stub[0x0E..0x11] == [0x8D, 0xFB, 0xDF]
        && stub[0x1B..0x1E] == [0x6C, 0xFC, 0xFF];
    if !looks_like_kitaqfc_bypass {
        return;
    }

    let real_nmi_lo = stub[0x08];
    let real_nmi_hi = stub[0x0D];
    if real_nmi_lo == 0xFF && real_nmi_hi == 0xFF {
        return;
    }

    prg[nmi_vector_offset] = real_nmi_lo;
    prg[nmi_vector_offset + 1] = real_nmi_hi;
}

// Translate an address in the allocated $6000-based 32 KiB buffer to an
// offset. This buffer range is independent of the runtime BIOS overlay.
fn fds_prg_offset(addr: u16) -> Option<usize> {
    if addr < FDS_PRG_RAM_BASE {
        return None;
    }
    let offset = (addr - FDS_PRG_RAM_BASE) as usize;
    if offset >= FDS_PRG_RAM_SIZE {
        None
    } else {
        Some(offset)
    }
}

// Walk disk-info, count, header and data blocks at the expected byte offsets.
// Append only complete files, mark boot eligibility by the side-specific ID
// limit, and stop the side at its first malformed file with a warning. This
// parser does not validate media CRCs or search for replacement block markers.
fn parse_side(
    side: u8,
    data: &[u8],
    files: &mut Vec<FdsDiskFile>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    if data.len() < 58 {
        warnings.push(format!(
            "FDS side {} is too short to contain block 1/2",
            side
        ));
        return Ok(());
    }
    if data[0] != 0x01 {
        warnings.push(format!(
            "FDS side {} does not start with disk-info block marker 0x01",
            side
        ));
        return Ok(());
    }
    let boot_max_id = data[0x19];
    let mut pos = 56usize;
    if data.get(pos) != Some(&0x02) {
        warnings.push(format!(
            "FDS side {} is missing file-count block at offset {}",
            side, pos
        ));
        return Ok(());
    }
    let file_count = data.get(pos + 1).copied().unwrap_or(0) as usize;
    pos += 2;

    for file_index in 0..file_count {
        if pos + 16 > data.len() {
            warnings.push(format!(
                "FDS side {} file {} header is truncated",
                side, file_index
            ));
            break;
        }
        if data[pos] != 0x03 {
            warnings.push(format!(
                "FDS side {} file {} expected header marker 0x03 at offset {}, got ${:02X}",
                side, file_index, pos, data[pos]
            ));
            break;
        }
        let number = data[pos + 1];
        let id = data[pos + 2];
        let name = ascii_trim(&data[pos + 3..pos + 11]);
        let load_address = u16::from_le_bytes([data[pos + 0x0B], data[pos + 0x0C]]);
        let size = u16::from_le_bytes([data[pos + 0x0D], data[pos + 0x0E]]);
        let file_type = data[pos + 0x0F] & 0x7F;
        pos += 16;

        if pos >= data.len() || data[pos] != 0x04 {
            warnings.push(format!(
                "FDS side {} file {} expected data marker 0x04 at offset {}",
                side, file_index, pos
            ));
            break;
        }
        pos += 1;
        let data_len = size as usize;
        if pos + data_len > data.len() {
            warnings.push(format!(
                "FDS side {} file {} data is truncated from {} bytes",
                side, file_index, data_len
            ));
            break;
        }
        let file_data = data[pos..pos + data_len].to_vec();
        pos += data_len;

        files.push(FdsDiskFile {
            side,
            number,
            id,
            name,
            load_address,
            size,
            file_type,
            boot: id <= boot_max_id,
            data: file_data,
        });
    }

    Ok(())
}

// Keep printable ASCII and spaces, replace other bytes with a dot, and
// remove trailing whitespace while preserving leading spaces.
fn ascii_trim(bytes: &[u8]) -> String {
    let s: String = bytes
        .iter()
        .map(|b| {
            if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                '.'
            }
        })
        .collect();
    s.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // Construct an original two-file disk fixture and check parsed file count,
    // reset-vector extraction and initial CHR bytes without executing a BIOS.
    fn parses_kitaqfc_style_fds_blocks_and_boot_vectors() {
        let mut side = vec![0; FDS_SIDE_SIZE];
        side[0] = 0x01;
        side[0x19] = 1;
        side[56] = 0x02;
        side[57] = 2;

        let mut pos = 58usize;
        side[pos] = 0x03;
        side[pos + 2] = 0;
        side[pos + 3..pos + 11].copy_from_slice(b"KQFPRG  ");
        side[pos + 0x0B..pos + 0x0D].copy_from_slice(&0x6000u16.to_le_bytes());
        side[pos + 0x0D..pos + 0x0F].copy_from_slice(&(FDS_PRG_RAM_SIZE as u16).to_le_bytes());
        side[pos + 0x0F] = 0;
        pos += 16;
        side[pos] = 0x04;
        pos += 1;
        side[pos..pos + FDS_PRG_RAM_SIZE].fill(0xFF);
        let vector = pos + (0xDFFCusize - FDS_PRG_RAM_BASE as usize);
        side[vector..vector + 2].copy_from_slice(&0x8123u16.to_le_bytes());
        pos += FDS_PRG_RAM_SIZE;

        side[pos] = 0x03;
        side[pos + 1] = 1;
        side[pos + 2] = 1;
        side[pos + 3..pos + 11].copy_from_slice(b"KQFCHR  ");
        side[pos + 0x0B..pos + 0x0D].copy_from_slice(&0x0000u16.to_le_bytes());
        side[pos + 0x0D..pos + 0x0F].copy_from_slice(&4u16.to_le_bytes());
        side[pos + 0x0F] = 1;
        pos += 16;
        side[pos] = 0x04;
        pos += 1;
        side[pos..pos + 4].copy_from_slice(&[1, 2, 3, 4]);

        let mut image = vec![0; FDS_HEADER_SIZE];
        image[0..4].copy_from_slice(FDS_MAGIC);
        image[4] = 1;
        image.extend_from_slice(&side);

        let disk = FdsDiskImage::from_bytes(&image).expect("synthetic FDS image parses");
        assert_eq!(disk.files.len(), 2);
        assert_eq!(disk.fast_boot_pc(), Some(0x8123));
        assert_eq!(&disk.boot_chr_ram()[0..4], &[1, 2, 3, 4]);
    }

    #[test]
    // Construct the KITAQFC startup pattern and verify NMI-vector rewriting
    // while retaining the direct-boot reset target. No disk timing is exercised.
    fn direct_boot_applies_kitaqfc_license_bypass_nmi_vector() {
        let mut side = vec![0; FDS_SIDE_SIZE];
        side[0] = 0x01;
        side[0x19] = 0;
        side[56] = 0x02;
        side[57] = 1;

        let mut pos = 58usize;
        side[pos] = 0x03;
        side[pos + 2] = 0;
        side[pos + 3..pos + 11].copy_from_slice(b"KQFPRG  ");
        side[pos + 0x0B..pos + 0x0D].copy_from_slice(&0x6000u16.to_le_bytes());
        side[pos + 0x0D..pos + 0x0F].copy_from_slice(&(FDS_PRG_RAM_SIZE as u16).to_le_bytes());
        side[pos + 0x0F] = 0;
        pos += 16;
        side[pos] = 0x04;
        pos += 1;
        side[pos..pos + FDS_PRG_RAM_SIZE].fill(0xFF);
        let prg_base = pos;
        let tail = prg_base + (0xDFC0usize - FDS_PRG_RAM_BASE as usize);
        side[tail..tail + 0x1E].copy_from_slice(&[
            0xA9, 0x00, 0x8D, 0x00, 0x20, 0x85, 0xFF, 0xA9, 0x07, 0x8D, 0xFA, 0xDF, 0xA9, 0xA0,
            0x8D, 0xFB, 0xDF, 0xA9, 0x35, 0x8D, 0x02, 0x01, 0xA9, 0xAC, 0x8D, 0x03, 0x01, 0x6C,
            0xFC, 0xFF,
        ]);
        let vectors = prg_base + (0xDFFAusize - FDS_PRG_RAM_BASE as usize);
        side[vectors..vectors + 6].copy_from_slice(&[0xC0, 0xDF, 0xDE, 0xDF, 0xBC, 0xA0]);

        let mut image = vec![0; FDS_HEADER_SIZE];
        image[0..4].copy_from_slice(FDS_MAGIC);
        image[4] = 1;
        image.extend_from_slice(&side);

        let disk = FdsDiskImage::from_bytes(&image).expect("synthetic FDS image parses");
        let prg = disk.boot_prg_ram();
        let nmi_offset = 0xDFFAusize - FDS_PRG_RAM_BASE as usize;
        assert_eq!(&prg[nmi_offset..nmi_offset + 2], &[0x07, 0xA0]);
        assert_eq!(disk.fast_boot_pc(), Some(0xDFDE));
    }
}
