//! Original FDS read channel. .fds omits gaps/CRC; reconstruct them per block.
use crate::fds::{FdsDiskImage, FDS_HEADER_SIZE, FDS_SIDE_SIZE};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FdsDrive {
    pub sides: Vec<Vec<u8>>,
    pub inserted: Option<usize>,
    pub control: u8,
    pub position: usize,
    pub countdown: u64,
    pub scanning: bool,
    pub end: bool,
    pub data: u8,
    pub ready: bool,
    pub irq: bool,
    pub crc_error: bool,
    pub reads: u64,
    in_block: bool,
    crc: u16,
    previous_crc: bool,
    #[serde(default)]
    crc_bytes_seen: u8,
}

pub(crate) fn crc_byte(mut crc: u16, value: u8) -> u16 {
    crc ^= value as u16;
    for _ in 0..8 {
        crc = (crc >> 1) ^ if crc & 1 != 0 { 0x8408 } else { 0 };
    }
    crc
}

impl Default for FdsDrive {
    fn default() -> Self {
        Self {
            sides: Vec::new(),
            inserted: None,
            control: 0,
            position: 0,
            countdown: 0,
            scanning: false,
            end: true,
            data: 0,
            ready: false,
            irq: false,
            crc_error: false,
            reads: 0,
            in_block: false,
            crc: 0,
            previous_crc: false,
            crc_bytes_seen: 0,
        }
    }
}

impl FdsDrive {
    pub fn load(disk: &FdsDiskImage) -> Self {
        let offset = if disk.has_header { FDS_HEADER_SIZE } else { 0 };
        let sides = disk.raw[offset..]
            .chunks(FDS_SIDE_SIZE)
            .take(disk.side_count as usize)
            .map(Self::expand_side)
            .collect::<Vec<_>>();
        Self {
            inserted: if sides.is_empty() { None } else { Some(0) },
            sides,
            ..Self::default()
        }
    }

    fn expand_side(raw: &[u8]) -> Vec<u8> {
        let mut stream = vec![0; 3537];
        let mut pos = 0;
        let mut file_size = 0;
        while pos < raw.len() {
            let size = match raw[pos] {
                1 => 56,
                2 => 2,
                3 => 16,
                4 => file_size + 1,
                _ => break,
            };
            if size == 0 || pos + size > raw.len() {
                break;
            }
            if raw[pos] == 3 {
                file_size = u16::from_le_bytes([raw[pos + 13], raw[pos + 14]]) as usize;
            }
            stream.push(0x80);
            let block = &raw[pos..pos + size];
            let crc = block.iter().fold(crc_byte(0, 0x80), |c, &b| crc_byte(c, b));
            stream.extend_from_slice(block);
            stream.extend_from_slice(&crc.to_le_bytes());
            stream.resize(stream.len() + 122, 0);
            pos += size;
        }
        stream.resize(stream.len() + 122, 0);
        stream
    }

    pub fn set_control(&mut self, value: u8) {
        self.irq = false;
        self.control = value;
        if value & 0x40 == 0 {
            // The BIOS can drop and reassert START between two byte clocks.
            // Re-arm sync detection on the register write, not on a later byte.
            self.in_block = false;
            self.crc = 0;
            self.previous_crc = false;
            self.crc_bytes_seen = 0;
            self.crc_error = false;
            self.ready = false;
        }
        if value & 1 == 0 {
            self.end = true;
            self.scanning = false;
            self.ready = false;
        }
    }

    pub fn status(&self) -> u8 {
        if self.inserted.is_none() {
            7
        } else if !self.scanning {
            2
        } else {
            0
        }
    }

    pub fn read_data(&mut self) -> u8 {
        self.irq = false;
        self.ready = false;
        self.reads = self.reads.saturating_add(1);
        self.data
    }

    pub fn clock(&mut self, cycles: u64) {
        if self.inserted.is_none() || self.control & 1 == 0 {
            return;
        }
        if self.end {
            if self.control & 2 != 0 {
                return;
            }
            self.end = false;
            self.position = 0;
            self.scanning = false;
            self.countdown = 50_000;
            self.in_block = false;
            self.crc = 0;
            self.crc_error = false;
            self.previous_crc = false;
            self.crc_bytes_seen = 0;
        }
        let mut left = cycles;
        while left >= self.countdown && !self.end {
            left -= self.countdown;
            self.scanning = true;
            self.transfer();
            self.countdown = 150;
        }
        self.countdown = self.countdown.saturating_sub(left);
    }

    fn transfer(&mut self) {
        let Some(side) = self.inserted.and_then(|n| self.sides.get(n)) else {
            return;
        };
        if self.position >= side.len() {
            self.end = true;
            self.scanning = false;
            self.control &= !1;
            return;
        }
        let byte = side[self.position];
        self.position += 1;
        // Read-only media: write requests never alter an attached image.
        if self.control & 4 == 0 {
            return;
        }
        let crc_phase = self.control & 0x10 != 0;
        let mut sync = false;
        if self.control & 0x40 == 0 {
            self.in_block = false;
            self.crc = 0;
            self.crc_error = false;
            self.crc_bytes_seen = 0;
        } else if !self.in_block && byte != 0 {
            self.in_block = true;
            sync = true;
        }
        if self.in_block {
            self.crc = crc_byte(self.crc, byte);
            self.data = byte;
            // Gap-ending sync is consumed by the drive, not returned as block data.
            self.ready = !sync;
            if !sync && self.control & 0x80 != 0 {
                self.irq = true;
            }
        }
        if crc_phase {
            if !self.previous_crc {
                self.crc_bytes_seen = 0;
                self.crc_error = false;
            }
            if self.crc_bytes_seen < 2 {
                self.crc_bytes_seen += 1;
                if self.crc_bytes_seen == 2 {
                    self.crc_error = self.crc != 0;
                }
            }
        }
        self.previous_crc = crc_phase;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_result_waits_for_both_disk_bytes_and_detects_corruption() {
        let mut side = vec![0; FDS_SIDE_SIZE];
        side[0] = 1;
        side[1..15].copy_from_slice(b"*NINTENDO-HVC*");
        side[56] = 2;
        let disk = FdsDiskImage::from_bytes(&side).unwrap();
        for corrupt in [false, true] {
            let mut drive = FdsDrive::load(&disk);
            if corrupt {
                drive.sides[0][3537 + 1 + 56] ^= 1;
            }
            drive.set_control(0xc5);
            drive.clock(50_000 + (3537 + 56) * 150);
            assert_eq!(drive.position, 3537 + 1 + 56);
            drive.set_control(0xd5);
            drive.clock(150);
            assert!(!drive.crc_error, "CRC result is premature");
            drive.clock(150);
            assert_eq!(drive.crc_error, corrupt);
            drive.set_control(0x25);
            drive.set_control(0xe5);
            drive.clock(150);
            assert!(!drive.in_block);
            assert!(!drive.ready);
        }
    }
}
