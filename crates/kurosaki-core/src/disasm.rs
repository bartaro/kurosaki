use crate::cart::Cartridge;
use crate::cpu::opcode_mnemonic;
use crate::emulator::Emulator;
use crate::error::Result;
use crate::trace::TraceConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisasmLine {
    pub cpu_addr: u16,
    pub file_offset: usize,
    pub bytes: Vec<u8>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappedDisasmLine {
    pub cpu_addr: u16,
    pub bytes: Vec<u8>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhysicalPrgDisasmLine {
    pub cpu_addr: u16,
    pub prg_bank_8k: u16,
    pub file_offset: usize,
    pub bytes: Vec<u8>,
    pub text: String,
}

pub fn disassemble_reset_window(cart: &Cartridge, max_bytes: usize) -> Vec<DisasmLine> {
    let start = reset_vector(cart).unwrap_or(0x8000);
    disassemble_range(cart, start, max_bytes)
}

pub fn reset_vector(cart: &Cartridge) -> Option<u16> {
    if cart.prg_rom.len() < 4 {
        return None;
    }
    let base = if cart.prg_rom.len() <= 16 * 1024 {
        0x3FFC
    } else {
        0x7FFC
    };
    let lo = cart.prg_rom.get(base).copied()? as u16;
    let hi = cart.prg_rom.get(base + 1).copied()? as u16;
    Some(lo | (hi << 8))
}

pub fn disassemble_range(cart: &Cartridge, mut addr: u16, max_bytes: usize) -> Vec<DisasmLine> {
    let mut out = Vec::new();
    let end = addr.wrapping_add(max_bytes as u16);
    while addr != end && out.len() < max_bytes {
        let file_offset = prg_file_offset(cart, addr).unwrap_or(0);
        let op = read_prg(cart, addr).unwrap_or(0);
        let (len, text) = decode_one(cart, addr, op);
        let mut bytes = Vec::new();
        for i in 0..len {
            bytes.push(read_prg(cart, addr.wrapping_add(i as u16)).unwrap_or(0));
        }
        out.push(DisasmLine {
            cpu_addr: addr,
            file_offset,
            bytes,
            text,
        });
        addr = addr.wrapping_add(len as u16);
    }
    out
}

/// Disassembles through the currently restored CPU/mapper address space.
///
/// A private emulator clone is restored from a versioned snapshot so reads do
/// not alter the caller's state. This is the appropriate entry point for
/// bank-switched cartridges; `disassemble_range` intentionally retains its
/// original reset-window/static-ROM behavior for compatibility.
pub fn disassemble_mapped_range(
    emulator: &Emulator,
    mut addr: u16,
    max_bytes: usize,
) -> Result<Vec<MappedDisasmLine>> {
    let snapshot = emulator.snapshot();
    let mut probe = Emulator::from_snapshot(emulator.cartridge.clone(), &snapshot)?;
    let mut out = Vec::new();
    let mut consumed = 0usize;
    while consumed < max_bytes {
        let opcode = mapped_read(&mut probe, addr);
        let mnemonic = opcode_mnemonic(opcode);
        let len = instruction_len_from_mnemonic(opcode, mnemonic);
        let mut bytes = Vec::with_capacity(len);
        for offset in 0..len {
            bytes.push(mapped_read(&mut probe, addr.wrapping_add(offset as u16)));
        }
        out.push(MappedDisasmLine {
            cpu_addr: addr,
            text: render_instruction(addr, mnemonic, &bytes),
            bytes,
        });
        addr = addr.wrapping_add(len as u16);
        consumed = consumed.saturating_add(len);
    }
    Ok(out)
}

/// Disassembles bytes from one exact physical 8 KiB PRG-ROM bank.
///
/// This bypasses mapper state and is intended for correlating a trace's
/// `prg_bank` plus `pc` context with immutable cartridge bytes. The requested
/// range never wraps into a neighboring physical bank.
pub fn disassemble_physical_prg_bank_8k(
    cart: &Cartridge,
    prg_bank_8k: u16,
    mut addr: u16,
    max_bytes: usize,
) -> Result<Vec<PhysicalPrgDisasmLine>> {
    const BANK_SIZE: usize = 8 * 1024;
    if addr < 0x8000 {
        return Err(crate::error::KurosakiError::InvalidRom(format!(
            "physical PRG disassembly requires a CPU address at or above $8000, got ${addr:04X}"
        )));
    }
    let bank_start = usize::from(prg_bank_8k)
        .checked_mul(BANK_SIZE)
        .ok_or_else(|| {
            crate::error::KurosakiError::InvalidRom(format!(
                "physical PRG bank {prg_bank_8k} overflows the host address space"
            ))
        })?;
    if bank_start >= cart.prg_rom.len() {
        return Err(crate::error::KurosakiError::InvalidRom(format!(
            "physical PRG bank {prg_bank_8k} is outside {} available 8 KiB banks",
            cart.prg_rom.len().div_ceil(BANK_SIZE)
        )));
    }
    let within_bank = usize::from(addr & 0x1fff);
    let start_index = bank_start + within_bank;
    if start_index >= cart.prg_rom.len() {
        return Err(crate::error::KurosakiError::InvalidRom(format!(
            "CPU address ${addr:04X} is outside physical PRG bank {prg_bank_8k}"
        )));
    }
    let available = max_bytes
        .min(BANK_SIZE - within_bank)
        .min(cart.prg_rom.len() - start_index);
    let file_base = 16 + cart.trainer.as_ref().map_or(0, |_| 512);
    let mut consumed = 0usize;
    let mut out = Vec::new();
    while consumed < available {
        let opcode = cart.prg_rom[start_index + consumed];
        let mnemonic = opcode_mnemonic(opcode);
        let len = instruction_len_from_mnemonic(opcode, mnemonic);
        if consumed + len > available {
            break;
        }
        let bytes = cart.prg_rom[start_index + consumed..start_index + consumed + len].to_vec();
        out.push(PhysicalPrgDisasmLine {
            cpu_addr: addr,
            prg_bank_8k,
            file_offset: file_base + start_index + consumed,
            text: render_instruction(addr, mnemonic, &bytes),
            bytes,
        });
        addr = addr.wrapping_add(len as u16);
        consumed += len;
    }
    Ok(out)
}

fn mapped_read(emulator: &mut Emulator, addr: u16) -> u8 {
    emulator.bus.read(
        addr,
        emulator.frame,
        emulator.cpu.cycles,
        TraceConfig::none(),
        &mut emulator.trace,
    )
}

pub fn instruction_len_from_mnemonic(opcode: u8, mnemonic: &str) -> usize {
    if opcode & 0x1f == 0x10 {
        return 2;
    }
    if mnemonic.contains(" a") || mnemonic.contains("(a)") {
        3
    } else if mnemonic.contains('d') || mnemonic.contains('#') || mnemonic.contains("(d") {
        2
    } else {
        1
    }
}

fn render_instruction(addr: u16, mnemonic: &str, bytes: &[u8]) -> String {
    if bytes.len() == 2 && bytes[0] & 0x1f == 0x10 {
        return format!("{} {}", mnemonic, branch_target(addr, bytes[1]));
    }
    match bytes {
        [_, operand] => {
            if mnemonic.contains('#') {
                mnemonic.replacen('#', &format!("#${operand:02X}"), 1)
            } else if mnemonic.contains('d') {
                mnemonic.replacen('d', &format!("${operand:02X}"), 1)
            } else {
                mnemonic.to_owned()
            }
        }
        [_, lo, hi] => {
            let operand = u16::from_le_bytes([*lo, *hi]);
            if mnemonic.contains("(a)") {
                mnemonic.replacen("(a)", &format!("(${operand:04X})"), 1)
            } else if mnemonic.contains(" a") {
                mnemonic.replacen(" a", &format!(" ${operand:04X}"), 1)
            } else {
                mnemonic.to_owned()
            }
        }
        _ => mnemonic.to_owned(),
    }
}

fn prg_file_offset(cart: &Cartridge, addr: u16) -> Option<usize> {
    if addr < 0x8000 {
        return None;
    }
    let base = (addr as usize) - 0x8000;
    let mask = if cart.prg_rom.len() <= 16 * 1024 {
        0x3FFF
    } else {
        0x7FFF
    };
    Some(16 + cart.trainer.as_ref().map(|_| 512).unwrap_or(0) + (base & mask))
}

fn read_prg(cart: &Cartridge, addr: u16) -> Option<u8> {
    if addr < 0x8000 || cart.prg_rom.is_empty() {
        return None;
    }
    let base = (addr as usize) - 0x8000;
    let mask = if cart.prg_rom.len() <= 16 * 1024 {
        0x3FFF
    } else {
        0x7FFF
    };
    cart.prg_rom.get(base & mask).copied()
}

fn read_u16(cart: &Cartridge, addr: u16) -> u16 {
    let lo = read_prg(cart, addr).unwrap_or(0) as u16;
    let hi = read_prg(cart, addr.wrapping_add(1)).unwrap_or(0) as u16;
    lo | (hi << 8)
}

fn decode_one(cart: &Cartridge, addr: u16, op: u8) -> (usize, String) {
    match op {
        0x00 => (1, "BRK".to_string()),
        0x20 => (3, format!("JSR ${:04X}", read_u16(cart, addr + 1))),
        0x4C => (3, format!("JMP ${:04X}", read_u16(cart, addr + 1))),
        0x6C => (3, format!("JMP (${:04X})", read_u16(cart, addr + 1))),
        0x60 => (1, "RTS".to_string()),
        0x40 => (1, "RTI".to_string()),
        0x78 => (1, "SEI".to_string()),
        0x58 => (1, "CLI".to_string()),
        0xD8 => (1, "CLD".to_string()),
        0xEA => (1, "NOP".to_string()),
        0xA9 => (
            2,
            format!("LDA #${:02X}", read_prg(cart, addr + 1).unwrap_or(0)),
        ),
        0xA2 => (
            2,
            format!("LDX #${:02X}", read_prg(cart, addr + 1).unwrap_or(0)),
        ),
        0xA0 => (
            2,
            format!("LDY #${:02X}", read_prg(cart, addr + 1).unwrap_or(0)),
        ),
        0x8D => (3, format!("STA ${:04X}", read_u16(cart, addr + 1))),
        0x8E => (3, format!("STX ${:04X}", read_u16(cart, addr + 1))),
        0x8C => (3, format!("STY ${:04X}", read_u16(cart, addr + 1))),
        0x85 => (
            2,
            format!("STA ${:02X}", read_prg(cart, addr + 1).unwrap_or(0)),
        ),
        0x86 => (
            2,
            format!("STX ${:02X}", read_prg(cart, addr + 1).unwrap_or(0)),
        ),
        0x84 => (
            2,
            format!("STY ${:02X}", read_prg(cart, addr + 1).unwrap_or(0)),
        ),
        0xD0 => (
            2,
            format!(
                "BNE {}",
                branch_target(addr, read_prg(cart, addr + 1).unwrap_or(0))
            ),
        ),
        0xF0 => (
            2,
            format!(
                "BEQ {}",
                branch_target(addr, read_prg(cart, addr + 1).unwrap_or(0))
            ),
        ),
        0x90 => (
            2,
            format!(
                "BCC {}",
                branch_target(addr, read_prg(cart, addr + 1).unwrap_or(0))
            ),
        ),
        0xB0 => (
            2,
            format!(
                "BCS {}",
                branch_target(addr, read_prg(cart, addr + 1).unwrap_or(0))
            ),
        ),
        _ => (1, format!(".db ${op:02X}")),
    }
}

fn branch_target(addr: u16, off: u8) -> String {
    let target = ((addr.wrapping_add(2) as i32) + (off as i8 as i32)) as u16;
    format!("${target:04X}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_formatter_uses_full_opcode_table_operands() {
        assert_eq!(
            render_instruction(0x9000, "LDA #", &[0xa9, 0x2a]),
            "LDA #$2A"
        );
        assert_eq!(
            render_instruction(0x9000, "STA a,X", &[0x9d, 0x34, 0x12]),
            "STA $1234,X"
        );
        assert_eq!(
            render_instruction(0x9000, "BNE", &[0xd0, 0xfc]),
            "BNE $8FFE"
        );
    }

    #[test]
    fn official_instruction_shapes_have_expected_lengths() {
        assert_eq!(instruction_len_from_mnemonic(0xea, "NOP"), 1);
        assert_eq!(instruction_len_from_mnemonic(0xa5, "LDA d"), 2);
        assert_eq!(instruction_len_from_mnemonic(0xad, "LDA a"), 3);
        assert_eq!(instruction_len_from_mnemonic(0xd0, "BNE"), 2);
    }

    #[test]
    fn physical_prg_disassembly_selects_exact_8k_bank_without_wrapping() {
        let mut rom = vec![0_u8; 16 + 4 * 16 * 1024];
        rom[0..4].copy_from_slice(b"NES\x1A");
        rom[4] = 4;
        rom[5] = 0;
        let bank_start = 16 + 2 * 8 * 1024;
        rom[bank_start..bank_start + 3].copy_from_slice(&[0xa9, 0x2a, 0x60]);
        let cart = Cartridge::from_bytes(&rom).unwrap();

        let lines = disassemble_physical_prg_bank_8k(&cart, 2, 0x8000, 3).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].prg_bank_8k, 2);
        assert_eq!(lines[0].file_offset, bank_start);
        assert_eq!(lines[0].text, "LDA #$2A");
        assert_eq!(lines[1].text, "RTS");
        assert!(disassemble_physical_prg_bank_8k(&cart, 8, 0x8000, 1).is_err());
    }
}
