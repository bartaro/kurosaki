use crate::bus::Bus;
use crate::error::{KurosakiError, Result};
use crate::trace::{TraceConfig, TraceEvent, TraceSink};
use serde::{Deserialize, Serialize};

const CARRY: u8 = 0x01;
const ZERO: u8 = 0x02;
const IRQ_DISABLE: u8 = 0x04;
const DECIMAL: u8 = 0x08;
const BREAK_FLAG: u8 = 0x10;
const UNUSED: u8 = 0x20;
const OVERFLOW: u8 = 0x40;
const NEGATIVE: u8 = 0x80;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
// Serializable CPU registers, counters and the explicit CLI polling delay.
// Memory/device state lives on Bus rather than in this structure.
pub struct CpuState {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub sp: u8,
    pub pc: u16,
    pub p: u8,
    pub cycles: u64,
    pub stopped: bool,
    /// NMOS 6502 IRQ polling observes CLI's I-flag change one instruction late.
    #[serde(default)]
    pub irq_poll_delay: u8,
}

impl Default for CpuState {
    // Initialize zeroed A/X/Y/PC and counters with SP=$FD and the interrupt
    // mask/unused status bits set. Loading the reset vector is a separate step.
    fn default() -> Self {
        Self {
            a: 0,
            x: 0,
            y: 0,
            sp: 0xFD,
            pc: 0,
            p: IRQ_DISABLE | UNUSED,
            cycles: 0,
            stopped: false,
            irq_poll_delay: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
// Operand modes shared by arithmetic/load/store helpers. Control-flow
// instructions resolve their special operands directly in step.
enum AddrMode {
    Imm,
    Zp,
    ZpX,
    ZpY,
    Abs,
    AbsX,
    AbsY,
    IndX,
    IndY,
}

#[derive(Debug, Clone, Copy)]
// Effective address plus the carry into a new 256-byte page. Read helpers
// use the crossing flag for timing; stores use fixed opcode costs.
struct ResolvedAddr {
    addr: u16,
    page_crossed: bool,
}

impl CpuState {
    // Reset SP/status, read the reset vector using frame zero and the old
    // cycle timestamp, clear stopped/IRQ-delay state, then set cycles to seven.
    // A/X/Y and bus memory are retained; device clocks are not advanced here.
    pub fn reset(&mut self, bus: &mut Bus, cfg: TraceConfig, sink: &mut TraceSink) {
        self.sp = 0xFD;
        self.p = IRQ_DISABLE | UNUSED;
        self.pc = bus.read_u16(0xFFFC, 0, self.cycles, cfg, sink);
        self.stopped = false;
        self.irq_poll_delay = 0;
        self.cycles = 7;
    }

    // Push the current PC and status with the break bit cleared, mask IRQs,
    // read the NMI vector and add seven CPU cycles. This helper neither clocks
    // devices nor executes the handler instruction.
    pub fn service_nmi(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        self.push_u16(bus, self.pc, frame, cfg, sink);
        self.push(bus, self.p & !BREAK_FLAG, frame, cfg, sink);
        self.p |= IRQ_DISABLE;
        self.pc = bus.read_u16(0xFFFA, frame, self.cycles, cfg, sink);
        self.cycles += 7;
        if cfg.nmi {
            let mut event = TraceEvent::new("cpu.nmi", frame, self.cycles);
            event.pc = Some(self.pc);
            event.message = Some("NMI vector taken".to_string());
            sink.push(event);
        }
    }

    // Push PC/status, mask further IRQs and take the IRQ vector with seven
    // CPU cycles added. IRQ event emission shares the nmi trace selector.
    pub fn service_irq(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        self.push_u16(bus, self.pc, frame, cfg, sink);
        self.push(bus, self.p & !BREAK_FLAG, frame, cfg, sink);
        self.p |= IRQ_DISABLE;
        self.pc = bus.read_u16(0xFFFE, frame, self.cycles, cfg, sink);
        self.cycles += 7;
        if cfg.nmi {
            let mut event = TraceEvent::new("cpu.irq", frame, self.cycles);
            event.pc = Some(self.pc);
            event.message = Some("IRQ vector taken".to_string());
            sink.push(event);
        }
    }

    // Reject a stopped CPU, poll NMI before eligible IRQ, then execute one
    // instruction, including a handler instruction after interrupt entry. Return
    // only the instruction cycles: interrupt entry adds to self.cycles but is
    // not included in the count returned for external device clocking.
    pub fn step(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> Result<u8> {
        if self.stopped {
            return Err(KurosakiError::CpuStopped("CPU is stopped".to_string()));
        }
        bus.clear_trace_cpu_context();
        // Consume one pending CLI delay before polling. An NMI still takes
        // priority, and masked/deferred IRQs do not call take_irq.
        let defer_irq = self.irq_poll_delay != 0;
        self.irq_poll_delay = self.irq_poll_delay.saturating_sub(1);
        if bus.take_nmi() {
            self.service_nmi(bus, frame, cfg, sink);
        } else if !defer_irq && self.p & IRQ_DISABLE == 0 && bus.take_irq() {
            self.service_irq(bus, frame, cfg, sink);
        }

        // After interrupt entry, label the actual handler instruction with its
        // current physical bank. The bank label is retained through the instruction.
        let pc0 = self.pc;
        let prg_bank0 = bus.mapper.physical_prg_bank_8k(pc0);
        bus.set_trace_cpu_context(pc0, prg_bank0);
        let opcode = self.fetch(bus, frame, cfg, sink);
        bus.set_trace_cpu_opcode(opcode);
        let cycles = match opcode {
            0x00 => {
                // BRK
                // BRK has already fetched its opcode; skip its padding byte so the
                // stacked return PC points two bytes past the original instruction.
                self.pc = self.pc.wrapping_add(1);
                self.push_u16(bus, self.pc, frame, cfg, sink);
                self.push(bus, self.p | BREAK_FLAG | UNUSED, frame, cfg, sink);
                self.p |= IRQ_DISABLE;
                self.pc = bus.read_u16(0xFFFE, frame, self.cycles, cfg, sink);
                7
            }

            // ORA
            0x01 => self.op_ora(bus, frame, cfg, sink, AddrMode::IndX, 6),
            0x05 => self.op_ora(bus, frame, cfg, sink, AddrMode::Zp, 3),
            0x09 => self.op_ora(bus, frame, cfg, sink, AddrMode::Imm, 2),
            0x0D => self.op_ora(bus, frame, cfg, sink, AddrMode::Abs, 4),
            0x11 => self.op_ora_page(bus, frame, cfg, sink, AddrMode::IndY, 5),
            0x15 => self.op_ora(bus, frame, cfg, sink, AddrMode::ZpX, 4),
            0x19 => self.op_ora_page(bus, frame, cfg, sink, AddrMode::AbsY, 4),
            0x1D => self.op_ora_page(bus, frame, cfg, sink, AddrMode::AbsX, 4),

            // ASL
            0x06 => self.op_asl_mem(bus, frame, cfg, sink, AddrMode::Zp, 5),
            0x0A => {
                self.a = self.asl_value(self.a);
                2
            }
            0x0E => self.op_asl_mem(bus, frame, cfg, sink, AddrMode::Abs, 6),
            0x16 => self.op_asl_mem(bus, frame, cfg, sink, AddrMode::ZpX, 6),
            0x1E => self.op_asl_mem(bus, frame, cfg, sink, AddrMode::AbsX, 7),

            // Flags / stack / jumps
            0x08 => {
                self.push(bus, self.p | BREAK_FLAG | UNUSED, frame, cfg, sink);
                3
            }
            0x18 => {
                self.p &= !CARRY;
                self.p |= UNUSED;
                2
            }
            0x20 => {
                let addr = self.fetch_u16(bus, frame, cfg, sink);
                // JSR stores the address of its final operand byte. RTS adds one
                // after pulling this value to resume at the following instruction.
                self.push_u16(bus, self.pc.wrapping_sub(1), frame, cfg, sink);
                self.pc = addr;
                6
            }
            0x28 => {
                self.p = (self.pull(bus, frame, cfg, sink) | UNUSED) & !BREAK_FLAG;
                4
            }
            0x38 => {
                self.p |= CARRY | UNUSED;
                2
            }
            0x40 => {
                self.p = (self.pull(bus, frame, cfg, sink) | UNUSED) & !BREAK_FLAG;
                self.pc = self.pull_u16(bus, frame, cfg, sink);
                6
            }
            0x48 => {
                self.push(bus, self.a, frame, cfg, sink);
                3
            }
            0x58 => {
                // CLI schedules the modeled one-instruction IRQ polling delay only
                // when it actually changes I from set to clear.
                let was_disabled = self.p & IRQ_DISABLE != 0;
                self.p &= !IRQ_DISABLE;
                self.p |= UNUSED;
                if was_disabled {
                    self.irq_poll_delay = 1;
                }
                2
            }
            0x60 => {
                self.pc = self.pull_u16(bus, frame, cfg, sink).wrapping_add(1);
                6
            }
            0x68 => {
                self.a = self.pull(bus, frame, cfg, sink);
                self.set_zn(self.a);
                4
            }
            0x78 => {
                self.p |= IRQ_DISABLE | UNUSED;
                2
            }
            0x4C => {
                let addr = self.fetch_u16(bus, frame, cfg, sink);
                self.pc = addr;
                3
            }
            0x6C => {
                let ptr = self.fetch_u16(bus, frame, cfg, sink);
                // Use the page-wrapped high-byte read for indirect JMP instead of
                // ordinary 16-bit pointer incrementing.
                self.pc = bus.read_u16_zp_bug(ptr, frame, self.cycles, cfg, sink);
                5
            }

            // AND
            0x21 => self.op_and(bus, frame, cfg, sink, AddrMode::IndX, 6),
            0x25 => self.op_and(bus, frame, cfg, sink, AddrMode::Zp, 3),
            0x29 => self.op_and(bus, frame, cfg, sink, AddrMode::Imm, 2),
            0x2D => self.op_and(bus, frame, cfg, sink, AddrMode::Abs, 4),
            0x31 => self.op_and_page(bus, frame, cfg, sink, AddrMode::IndY, 5),
            0x35 => self.op_and(bus, frame, cfg, sink, AddrMode::ZpX, 4),
            0x39 => self.op_and_page(bus, frame, cfg, sink, AddrMode::AbsY, 4),
            0x3D => self.op_and_page(bus, frame, cfg, sink, AddrMode::AbsX, 4),

            // BIT
            0x24 => {
                let addr = self.resolve_addr(bus, frame, cfg, sink, AddrMode::Zp).addr;
                let v = bus.read_timed(addr, frame, self.cycles, 2, cfg, sink);
                self.bit(v);
                3
            }
            0x2C => {
                let addr = self.resolve_addr(bus, frame, cfg, sink, AddrMode::Abs).addr;
                let v = bus.read_timed(addr, frame, self.cycles, 3, cfg, sink);
                self.bit(v);
                4
            }

            // ROL
            0x26 => self.op_rol_mem(bus, frame, cfg, sink, AddrMode::Zp, 5),
            0x2A => {
                self.a = self.rol_value(self.a);
                2
            }
            0x2E => self.op_rol_mem(bus, frame, cfg, sink, AddrMode::Abs, 6),
            0x36 => self.op_rol_mem(bus, frame, cfg, sink, AddrMode::ZpX, 6),
            0x3E => self.op_rol_mem(bus, frame, cfg, sink, AddrMode::AbsX, 7),

            // EOR
            0x41 => self.op_eor(bus, frame, cfg, sink, AddrMode::IndX, 6),
            0x45 => self.op_eor(bus, frame, cfg, sink, AddrMode::Zp, 3),
            0x49 => self.op_eor(bus, frame, cfg, sink, AddrMode::Imm, 2),
            0x4D => self.op_eor(bus, frame, cfg, sink, AddrMode::Abs, 4),
            0x51 => self.op_eor_page(bus, frame, cfg, sink, AddrMode::IndY, 5),
            0x55 => self.op_eor(bus, frame, cfg, sink, AddrMode::ZpX, 4),
            0x59 => self.op_eor_page(bus, frame, cfg, sink, AddrMode::AbsY, 4),
            0x5D => self.op_eor_page(bus, frame, cfg, sink, AddrMode::AbsX, 4),

            // LSR
            0x46 => self.op_lsr_mem(bus, frame, cfg, sink, AddrMode::Zp, 5),
            0x4A => {
                self.a = self.lsr_value(self.a);
                2
            }
            0x4E => self.op_lsr_mem(bus, frame, cfg, sink, AddrMode::Abs, 6),
            0x56 => self.op_lsr_mem(bus, frame, cfg, sink, AddrMode::ZpX, 6),
            0x5E => self.op_lsr_mem(bus, frame, cfg, sink, AddrMode::AbsX, 7),

            // ADC
            0x61 => self.op_adc(bus, frame, cfg, sink, AddrMode::IndX, 6),
            0x65 => self.op_adc(bus, frame, cfg, sink, AddrMode::Zp, 3),
            0x69 => self.op_adc(bus, frame, cfg, sink, AddrMode::Imm, 2),
            0x6D => self.op_adc(bus, frame, cfg, sink, AddrMode::Abs, 4),
            0x71 => self.op_adc_page(bus, frame, cfg, sink, AddrMode::IndY, 5),
            0x75 => self.op_adc(bus, frame, cfg, sink, AddrMode::ZpX, 4),
            0x79 => self.op_adc_page(bus, frame, cfg, sink, AddrMode::AbsY, 4),
            0x7D => self.op_adc_page(bus, frame, cfg, sink, AddrMode::AbsX, 4),

            // ROR
            0x66 => self.op_ror_mem(bus, frame, cfg, sink, AddrMode::Zp, 5),
            0x6A => {
                self.a = self.ror_value(self.a);
                2
            }
            0x6E => self.op_ror_mem(bus, frame, cfg, sink, AddrMode::Abs, 6),
            0x76 => self.op_ror_mem(bus, frame, cfg, sink, AddrMode::ZpX, 6),
            0x7E => self.op_ror_mem(bus, frame, cfg, sink, AddrMode::AbsX, 7),

            // Store
            0x81 => self.op_sta(bus, frame, cfg, sink, AddrMode::IndX, 6),
            0x84 => self.op_sty(bus, frame, cfg, sink, AddrMode::Zp, 3),
            0x85 => self.op_sta(bus, frame, cfg, sink, AddrMode::Zp, 3),
            0x86 => self.op_stx(bus, frame, cfg, sink, AddrMode::Zp, 3),
            0x8C => self.op_sty(bus, frame, cfg, sink, AddrMode::Abs, 4),
            0x8D => self.op_sta(bus, frame, cfg, sink, AddrMode::Abs, 4),
            0x8E => self.op_stx(bus, frame, cfg, sink, AddrMode::Abs, 4),
            0x91 => self.op_sta(bus, frame, cfg, sink, AddrMode::IndY, 6),
            0x94 => self.op_sty(bus, frame, cfg, sink, AddrMode::ZpX, 4),
            0x95 => self.op_sta(bus, frame, cfg, sink, AddrMode::ZpX, 4),
            0x96 => self.op_stx(bus, frame, cfg, sink, AddrMode::ZpY, 4),
            0x99 => self.op_sta(bus, frame, cfg, sink, AddrMode::AbsY, 5),
            0x9D => self.op_sta(bus, frame, cfg, sink, AddrMode::AbsX, 5),
            0x9F => self.op_ahx_abs_y(bus, frame, cfg, sink),

            // Transfer / decrement / increment
            0x88 => {
                self.y = self.y.wrapping_sub(1);
                self.set_zn(self.y);
                2
            }
            0x8A => {
                self.a = self.x;
                self.set_zn(self.a);
                2
            }
            0x98 => {
                self.a = self.y;
                self.set_zn(self.a);
                2
            }
            0x9A => {
                // TXS copies the stack offset without updating Z/N, unlike TSX.
                self.sp = self.x;
                2
            }
            0xA8 => {
                self.y = self.a;
                self.set_zn(self.y);
                2
            }
            0xAA => {
                self.x = self.a;
                self.set_zn(self.x);
                2
            }
            0xBA => {
                self.x = self.sp;
                self.set_zn(self.x);
                2
            }
            0xC8 => {
                self.y = self.y.wrapping_add(1);
                self.set_zn(self.y);
                2
            }
            0xCA => {
                self.x = self.x.wrapping_sub(1);
                self.set_zn(self.x);
                2
            }
            0xE8 => {
                self.x = self.x.wrapping_add(1);
                self.set_zn(self.x);
                2
            }
            0xEA => 2,

            // LDY
            0xA0 => self.op_ldy(bus, frame, cfg, sink, AddrMode::Imm, 2),
            0xA4 => self.op_ldy(bus, frame, cfg, sink, AddrMode::Zp, 3),
            0xAC => self.op_ldy(bus, frame, cfg, sink, AddrMode::Abs, 4),
            0xB4 => self.op_ldy(bus, frame, cfg, sink, AddrMode::ZpX, 4),
            0xBC => self.op_ldy_page(bus, frame, cfg, sink, AddrMode::AbsX, 4),

            // LDA
            0xA1 => self.op_lda(bus, frame, cfg, sink, AddrMode::IndX, 6),
            0xA5 => self.op_lda(bus, frame, cfg, sink, AddrMode::Zp, 3),
            0xA9 => self.op_lda(bus, frame, cfg, sink, AddrMode::Imm, 2),
            0xAD => self.op_lda(bus, frame, cfg, sink, AddrMode::Abs, 4),
            0xB1 => self.op_lda_page(bus, frame, cfg, sink, AddrMode::IndY, 5),
            0xB5 => self.op_lda(bus, frame, cfg, sink, AddrMode::ZpX, 4),
            0xB9 => self.op_lda_page(bus, frame, cfg, sink, AddrMode::AbsY, 4),
            0xBD => self.op_lda_page(bus, frame, cfg, sink, AddrMode::AbsX, 4),

            // LDX
            0xA2 => self.op_ldx(bus, frame, cfg, sink, AddrMode::Imm, 2),
            0xA6 => self.op_ldx(bus, frame, cfg, sink, AddrMode::Zp, 3),
            0xAE => self.op_ldx(bus, frame, cfg, sink, AddrMode::Abs, 4),
            0xB6 => self.op_ldx(bus, frame, cfg, sink, AddrMode::ZpY, 4),
            0xBE => self.op_ldx_page(bus, frame, cfg, sink, AddrMode::AbsY, 4),

            // CMP / CPX / CPY
            0xC0 => self.op_cpy(bus, frame, cfg, sink, AddrMode::Imm, 2),
            0xC4 => self.op_cpy(bus, frame, cfg, sink, AddrMode::Zp, 3),
            0xCC => self.op_cpy(bus, frame, cfg, sink, AddrMode::Abs, 4),
            0xC1 => self.op_cmp(bus, frame, cfg, sink, AddrMode::IndX, 6),
            0xC5 => self.op_cmp(bus, frame, cfg, sink, AddrMode::Zp, 3),
            0xC9 => self.op_cmp(bus, frame, cfg, sink, AddrMode::Imm, 2),
            0xCD => self.op_cmp(bus, frame, cfg, sink, AddrMode::Abs, 4),
            0xD1 => self.op_cmp_page(bus, frame, cfg, sink, AddrMode::IndY, 5),
            0xD5 => self.op_cmp(bus, frame, cfg, sink, AddrMode::ZpX, 4),
            0xD9 => self.op_cmp_page(bus, frame, cfg, sink, AddrMode::AbsY, 4),
            0xDD => self.op_cmp_page(bus, frame, cfg, sink, AddrMode::AbsX, 4),
            0xE0 => self.op_cpx(bus, frame, cfg, sink, AddrMode::Imm, 2),
            0xE4 => self.op_cpx(bus, frame, cfg, sink, AddrMode::Zp, 3),
            0xEC => self.op_cpx(bus, frame, cfg, sink, AddrMode::Abs, 4),

            // DEC / INC
            0xC6 => self.op_dec_mem(bus, frame, cfg, sink, AddrMode::Zp, 5),
            0xCE => self.op_dec_mem(bus, frame, cfg, sink, AddrMode::Abs, 6),
            0xD6 => self.op_dec_mem(bus, frame, cfg, sink, AddrMode::ZpX, 6),
            0xDE => self.op_dec_mem(bus, frame, cfg, sink, AddrMode::AbsX, 7),
            0xE6 => self.op_inc_mem(bus, frame, cfg, sink, AddrMode::Zp, 5),
            0xEE => self.op_inc_mem(bus, frame, cfg, sink, AddrMode::Abs, 6),
            0xF6 => self.op_inc_mem(bus, frame, cfg, sink, AddrMode::ZpX, 6),
            0xFE => self.op_inc_mem(bus, frame, cfg, sink, AddrMode::AbsX, 7),

            // SBC
            0xE1 => self.op_sbc(bus, frame, cfg, sink, AddrMode::IndX, 6),
            0xE5 => self.op_sbc(bus, frame, cfg, sink, AddrMode::Zp, 3),
            0xE9 => self.op_sbc(bus, frame, cfg, sink, AddrMode::Imm, 2),
            0xED => self.op_sbc(bus, frame, cfg, sink, AddrMode::Abs, 4),
            0xF1 => self.op_sbc_page(bus, frame, cfg, sink, AddrMode::IndY, 5),
            0xF5 => self.op_sbc(bus, frame, cfg, sink, AddrMode::ZpX, 4),
            0xF9 => self.op_sbc_page(bus, frame, cfg, sink, AddrMode::AbsY, 4),
            0xFD => self.op_sbc_page(bus, frame, cfg, sink, AddrMode::AbsX, 4),

            // Branches
            0x10 => self.branch(bus, frame, cfg, sink, self.p & NEGATIVE == 0),
            0x30 => self.branch(bus, frame, cfg, sink, self.p & NEGATIVE != 0),
            0x50 => self.branch(bus, frame, cfg, sink, self.p & OVERFLOW == 0),
            0x70 => self.branch(bus, frame, cfg, sink, self.p & OVERFLOW != 0),
            0x90 => self.branch(bus, frame, cfg, sink, self.p & CARRY == 0),
            0xB0 => self.branch(bus, frame, cfg, sink, self.p & CARRY != 0),
            0xD0 => self.branch(bus, frame, cfg, sink, self.p & ZERO == 0),
            0xF0 => self.branch(bus, frame, cfg, sink, self.p & ZERO != 0),

            // Flags
            0xB8 => {
                self.p &= !OVERFLOW;
                self.p |= UNUSED;
                2
            }
            0xD8 => {
                self.p &= !DECIMAL;
                self.p |= UNUSED;
                2
            }
            0xF8 => {
                self.p |= DECIMAL | UNUSED;
                2
            }

            _ => {
                bus.clear_trace_cpu_context();
                // The opcode fetch already advanced PC. Clear trace context and
                // return before adding instruction cycles; the caller decides whether
                // to convert the error into a stopped CPU.
                return Err(KurosakiError::UnimplementedOpcode { opcode, pc: pc0 });
            }
        };
        // Add the selected opcode cost after its bus operations. DMA stall
        // accounting is held on Bus and is not consumed by this CPU step.
        self.cycles += cycles as u64;
        // Emit instruction context with the starting PC/bank and final
        // register values at the end-of-instruction CPU timestamp.
        if cfg.cpu {
            let mut event = TraceEvent::new("cpu.instruction", frame, self.cycles);
            event.pc = Some(pc0);
            event.prg_bank = prg_bank0;
            event.opcode = Some(opcode);
            event.mnemonic = Some(opcode_mnemonic(opcode).to_string());
            event.message = Some(format!(
                "A={:02X} X={:02X} Y={:02X} SP={:02X} P={:02X}",
                self.a, self.x, self.y, self.sp, self.p
            ));
            sink.push(event);
        }
        bus.clear_trace_cpu_context();
        Ok(cycles)
    }

    // Read the byte at PC through the bus and wrap PC to the next address.
    // Operand/opcode fetches use the current CPU timestamp without clocking.
    fn fetch(&mut self, bus: &mut Bus, frame: u64, cfg: TraceConfig, sink: &mut TraceSink) -> u8 {
        let v = bus.read(self.pc, frame, self.cycles, cfg, sink);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    // Fetch low then high operand bytes through PC and combine them little-endian.
    fn fetch_u16(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u16 {
        let lo = self.fetch(bus, frame, cfg, sink) as u16;
        let hi = self.fetch(bus, frame, cfg, sink) as u16;
        lo | (hi << 8)
    }

    // Write a byte to $0100|SP, then decrement SP with eight-bit wrapping.
    fn push(
        &mut self,
        bus: &mut Bus,
        value: u8,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        let addr = 0x0100 | self.sp as u16;
        bus.write(addr, value, frame, self.cycles, cfg, sink);
        self.sp = self.sp.wrapping_sub(1);
    }

    // Increment SP with eight-bit wrapping, then read the corresponding stack byte.
    fn pull(&mut self, bus: &mut Bus, frame: u64, cfg: TraceConfig, sink: &mut TraceSink) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        let addr = 0x0100 | self.sp as u16;
        bus.read(addr, frame, self.cycles, cfg, sink)
    }

    // Push the high byte before the low byte so the low byte is pulled first.
    fn push_u16(
        &mut self,
        bus: &mut Bus,
        value: u16,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        self.push(bus, (value >> 8) as u8, frame, cfg, sink);
        self.push(bus, value as u8, frame, cfg, sink);
    }

    // Pull low then high stack bytes and reconstruct the 16-bit value.
    fn pull_u16(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u16 {
        let lo = self.pull(bus, frame, cfg, sink) as u16;
        let hi = self.pull(bus, frame, cfg, sink) as u16;
        lo | (hi << 8)
    }

    // Consume address operands and resolve the selected mode. Zero-page index
    // and pointer arithmetic wrap within one byte; absolute indexing wraps at
    // 16 bits and records page crossing. Immediate mode leaves PC advancement
    // to read_mode. Indexed dummy bus accesses are not issued by this resolver.
    fn resolve_addr(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
    ) -> ResolvedAddr {
        match mode {
            AddrMode::Imm => ResolvedAddr {
                addr: self.pc,
                page_crossed: false,
            },
            AddrMode::Zp => ResolvedAddr {
                addr: self.fetch(bus, frame, cfg, sink) as u16,
                page_crossed: false,
            },
            AddrMode::ZpX => {
                let base = self.fetch(bus, frame, cfg, sink);
                ResolvedAddr {
                    addr: base.wrapping_add(self.x) as u16,
                    page_crossed: false,
                }
            }
            AddrMode::ZpY => {
                let base = self.fetch(bus, frame, cfg, sink);
                ResolvedAddr {
                    addr: base.wrapping_add(self.y) as u16,
                    page_crossed: false,
                }
            }
            AddrMode::Abs => ResolvedAddr {
                addr: self.fetch_u16(bus, frame, cfg, sink),
                page_crossed: false,
            },
            AddrMode::AbsX => {
                let base = self.fetch_u16(bus, frame, cfg, sink);
                let addr = base.wrapping_add(self.x as u16);
                ResolvedAddr {
                    addr,
                    page_crossed: (base & 0xFF00) != (addr & 0xFF00),
                }
            }
            AddrMode::AbsY => {
                let base = self.fetch_u16(bus, frame, cfg, sink);
                let addr = base.wrapping_add(self.y as u16);
                ResolvedAddr {
                    addr,
                    page_crossed: (base & 0xFF00) != (addr & 0xFF00),
                }
            }
            AddrMode::IndX => {
                let ptr = self.fetch(bus, frame, cfg, sink).wrapping_add(self.x);
                let lo = bus.read(ptr as u16, frame, self.cycles, cfg, sink) as u16;
                let hi = bus.read(ptr.wrapping_add(1) as u16, frame, self.cycles, cfg, sink) as u16;
                ResolvedAddr {
                    addr: lo | (hi << 8),
                    page_crossed: false,
                }
            }
            AddrMode::IndY => {
                let ptr = self.fetch(bus, frame, cfg, sink);
                let lo = bus.read(ptr as u16, frame, self.cycles, cfg, sink) as u16;
                let hi = bus.read(ptr.wrapping_add(1) as u16, frame, self.cycles, cfg, sink) as u16;
                let base = lo | (hi << 8);
                let addr = base.wrapping_add(self.y as u16);
                ResolvedAddr {
                    addr,
                    page_crossed: (base & 0xFF00) != (addr & 0xFF00),
                }
            }
        }
    }

    // Resolve and read the operand with an addressing-mode-specific timing
    // offset, including indexed page crossing. Advance PC explicitly for an
    // immediate operand and return both its value and the crossing flag.
    fn read_mode(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
    ) -> (u8, bool) {
        let r = self.resolve_addr(bus, frame, cfg, sink, mode);
        let cpu_read_offset = match mode {
            AddrMode::Imm => 1,
            AddrMode::Zp => 2,
            AddrMode::ZpX | AddrMode::ZpY => 3,
            AddrMode::Abs => 3,
            AddrMode::AbsX | AddrMode::AbsY => 3 + u8::from(r.page_crossed),
            AddrMode::IndX => 5,
            AddrMode::IndY => 4 + u8::from(r.page_crossed),
        };
        let v = bus.read_timed(r.addr, frame, self.cycles, cpu_read_offset, cfg, sink);
        if matches!(mode, AddrMode::Imm) {
            self.pc = self.pc.wrapping_add(1);
        }
        (v, r.page_crossed)
    }

    // Resolve the destination and write at the current CPU timestamp. Stores
    // use their caller's fixed cycle count rather than a read-style page penalty.
    fn write_mode(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        value: u8,
    ) {
        let r = self.resolve_addr(bus, frame, cfg, sink, mode);
        bus.write(r.addr, value, frame, self.cycles, cfg, sink);
    }

    // Update zero/sign flags from one byte and force the unused status bit high,
    // preserving the other flags.
    fn set_zn(&mut self, value: u8) {
        if value == 0 {
            self.p |= ZERO;
        } else {
            self.p &= !ZERO;
        }
        if value & 0x80 != 0 {
            self.p |= NEGATIVE;
        } else {
            self.p &= !NEGATIVE;
        }
        self.p |= UNUSED;
    }

    // Subtract without storing the result; carry means reg >= value and Z/N
    // come from the wrapped difference. Overflow is preserved.
    fn compare(&mut self, reg: u8, value: u8) {
        let r = reg.wrapping_sub(value);
        if reg >= value {
            self.p |= CARRY;
        } else {
            self.p &= !CARRY;
        }
        self.set_zn(r);
    }

    // Add the operand and carry to A, deriving carry from the ninth bit and
    // signed overflow from operand/result sign changes. Store the low byte and
    // update Z/N; the decimal flag does not select BCD arithmetic.
    fn adc(&mut self, value: u8) {
        // Ricoh 2A03 keeps the decimal flag but omits BCD arithmetic; ADC/SBC are binary.
        let carry = if self.p & CARRY != 0 { 1 } else { 0 };
        let sum = self.a as u16 + value as u16 + carry as u16;
        let result = sum as u8;
        if sum > 0xFF {
            self.p |= CARRY;
        } else {
            self.p &= !CARRY;
        }
        if (!(self.a ^ value) & (self.a ^ result) & 0x80) != 0 {
            self.p |= OVERFLOW;
        } else {
            self.p &= !OVERFLOW;
        }
        self.a = result;
        self.set_zn(self.a);
    }

    // Use ADC with the complemented operand so carry represents no borrow
    // and the existing ADC flag calculations implement binary subtraction.
    fn sbc(&mut self, value: u8) {
        self.adc(!value);
    }

    // Set zero from A AND operand, and copy operand bits six/seven into V/N.
    // A and carry are unchanged; the unused status bit is forced high.
    fn bit(&mut self, value: u8) {
        if self.a & value == 0 {
            self.p |= ZERO;
        } else {
            self.p &= !ZERO;
        }
        if value & 0x40 != 0 {
            self.p |= OVERFLOW;
        } else {
            self.p &= !OVERFLOW;
        }
        if value & 0x80 != 0 {
            self.p |= NEGATIVE;
        } else {
            self.p &= !NEGATIVE;
        }
        self.p |= UNUSED;
    }

    // Shift left, moving original bit seven to carry and updating Z/N from
    // the low-byte result.
    fn asl_value(&mut self, value: u8) -> u8 {
        if value & 0x80 != 0 {
            self.p |= CARRY;
        } else {
            self.p &= !CARRY;
        }
        let r = value << 1;
        self.set_zn(r);
        r
    }

    // Shift right with zero fill, moving original bit zero to carry and updating Z/N.
    fn lsr_value(&mut self, value: u8) -> u8 {
        if value & 0x01 != 0 {
            self.p |= CARRY;
        } else {
            self.p &= !CARRY;
        }
        let r = value >> 1;
        self.set_zn(r);
        r
    }

    // Rotate left through the previous carry, then set carry from original
    // bit seven and update Z/N.
    fn rol_value(&mut self, value: u8) -> u8 {
        let carry_in = if self.p & CARRY != 0 { 1 } else { 0 };
        if value & 0x80 != 0 {
            self.p |= CARRY;
        } else {
            self.p &= !CARRY;
        }
        let r = (value << 1) | carry_in;
        self.set_zn(r);
        r
    }

    // Rotate right through the previous carry, then set carry from original
    // bit zero and update Z/N.
    fn ror_value(&mut self, value: u8) -> u8 {
        let carry_in = if self.p & CARRY != 0 { 0x80 } else { 0 };
        if value & 0x01 != 0 {
            self.p |= CARRY;
        } else {
            self.p &= !CARRY;
        }
        let r = (value >> 1) | carry_in;
        self.set_zn(r);
        r
    }

    // Always consume the signed displacement. A taken branch updates PC
    // relative to the next instruction and costs three or four cycles depending
    // on page crossing; an untaken branch costs two.
    fn branch(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        take: bool,
    ) -> u8 {
        let offset = self.fetch(bus, frame, cfg, sink) as i8;
        if take {
            let old_pc = self.pc;
            self.pc = ((self.pc as i32) + (offset as i32)) as u16;
            if (old_pc & 0xFF00) != (self.pc & 0xFF00) {
                4
            } else {
                3
            }
        } else {
            2
        }
    }

    // Load the addressed operand into A and update Z/N.
    // Return the caller-supplied base cycles without a page-crossing penalty.
    fn op_lda(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, _) = self.read_mode(bus, frame, cfg, sink, mode);
        self.a = v;
        self.set_zn(self.a);
        base
    }
    // Load the addressed operand into A and update Z/N.
    // Add one cycle to the supplied base when the operand address crosses a page.
    fn op_lda_page(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, p) = self.read_mode(bus, frame, cfg, sink, mode);
        self.a = v;
        self.set_zn(self.a);
        base + p as u8
    }
    // Load the addressed operand into X and update Z/N.
    // Return the caller-supplied base cycles without a page-crossing penalty.
    fn op_ldx(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, _) = self.read_mode(bus, frame, cfg, sink, mode);
        self.x = v;
        self.set_zn(self.x);
        base
    }
    // Load the addressed operand into X and update Z/N.
    // Add one cycle to the supplied base when the operand address crosses a page.
    fn op_ldx_page(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, p) = self.read_mode(bus, frame, cfg, sink, mode);
        self.x = v;
        self.set_zn(self.x);
        base + p as u8
    }
    // Load the addressed operand into Y and update Z/N.
    // Return the caller-supplied base cycles without a page-crossing penalty.
    fn op_ldy(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, _) = self.read_mode(bus, frame, cfg, sink, mode);
        self.y = v;
        self.set_zn(self.y);
        base
    }
    // Load the addressed operand into Y and update Z/N.
    // Add one cycle to the supplied base when the operand address crosses a page.
    fn op_ldy_page(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, p) = self.read_mode(bus, frame, cfg, sink, mode);
        self.y = v;
        self.set_zn(self.y);
        base + p as u8
    }
    // Store A through the selected addressing mode without changing flags.
    // Return the supplied fixed cycle count, including any indexed-store cost.
    fn op_sta(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        self.write_mode(bus, frame, cfg, sink, mode, self.a);
        base
    }
    // Store X through the selected addressing mode without changing flags.
    // Return the supplied fixed cycle count, including any indexed-store cost.
    fn op_stx(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        self.write_mode(bus, frame, cfg, sink, mode, self.x);
        base
    }
    // Store Y through the selected addressing mode without changing flags.
    // Return the supplied fixed cycle count, including any indexed-store cost.
    fn op_sty(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        self.write_mode(bus, frame, cfg, sink, mode, self.y);
        base
    }

    /// NMOS 6502/2A03 unofficial AHX absolute,Y ($9F).
    ///
    /// AHX stores A & X & (operand high byte + 1). When indexing crosses a
    /// page, the masked value also becomes the effective address high byte.
    /// This intentionally models the deterministic digital behavior used by
    /// compatibility software; analog instability is outside this
    /// instruction-granularity core.
    fn op_ahx_abs_y(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u8 {
        let base = self.fetch_u16(bus, frame, cfg, sink);
        let low_sum = (base & 0x00ff) + self.y as u16;
        let value = self.a & self.x & ((base >> 8) as u8).wrapping_add(1);
        let addr = if low_sum > 0x00ff {
            ((value as u16) << 8) | (low_sum & 0x00ff)
        } else {
            base.wrapping_add(self.y as u16)
        };
        bus.write(addr, value, frame, self.cycles, cfg, sink);
        5
    }

    // OR the addressed operand into A and update Z/N, preserving carry/overflow.
    // Return the caller-supplied base cycles without a page-crossing penalty.
    fn op_ora(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, _) = self.read_mode(bus, frame, cfg, sink, mode);
        self.a |= v;
        self.set_zn(self.a);
        base
    }
    // OR the addressed operand into A and update Z/N, preserving carry/overflow.
    // Add one cycle to the supplied base when the operand address crosses a page.
    fn op_ora_page(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, p) = self.read_mode(bus, frame, cfg, sink, mode);
        self.a |= v;
        self.set_zn(self.a);
        base + p as u8
    }
    // AND the addressed operand into A and update Z/N, preserving carry/overflow.
    // Return the caller-supplied base cycles without a page-crossing penalty.
    fn op_and(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, _) = self.read_mode(bus, frame, cfg, sink, mode);
        self.a &= v;
        self.set_zn(self.a);
        base
    }
    // AND the addressed operand into A and update Z/N, preserving carry/overflow.
    // Add one cycle to the supplied base when the operand address crosses a page.
    fn op_and_page(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, p) = self.read_mode(bus, frame, cfg, sink, mode);
        self.a &= v;
        self.set_zn(self.a);
        base + p as u8
    }
    // XOR the addressed operand into A and update Z/N, preserving carry/overflow.
    // Return the caller-supplied base cycles without a page-crossing penalty.
    fn op_eor(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, _) = self.read_mode(bus, frame, cfg, sink, mode);
        self.a ^= v;
        self.set_zn(self.a);
        base
    }
    // XOR the addressed operand into A and update Z/N, preserving carry/overflow.
    // Add one cycle to the supplied base when the operand address crosses a page.
    fn op_eor_page(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, p) = self.read_mode(bus, frame, cfg, sink, mode);
        self.a ^= v;
        self.set_zn(self.a);
        base + p as u8
    }
    // Read the addressed operand and perform binary add-with-carry on A.
    // Return the caller-supplied base cycles without a page-crossing penalty.
    fn op_adc(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, _) = self.read_mode(bus, frame, cfg, sink, mode);
        self.adc(v);
        base
    }
    // Read the addressed operand and perform binary add-with-carry on A.
    // Add one cycle to the supplied base when the operand address crosses a page.
    fn op_adc_page(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, p) = self.read_mode(bus, frame, cfg, sink, mode);
        self.adc(v);
        base + p as u8
    }
    // Read the addressed operand and perform binary subtract-with-borrow on A.
    // Return the caller-supplied base cycles without a page-crossing penalty.
    fn op_sbc(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, _) = self.read_mode(bus, frame, cfg, sink, mode);
        self.sbc(v);
        base
    }
    // Read the addressed operand and perform binary subtract-with-borrow on A.
    // Add one cycle to the supplied base when the operand address crosses a page.
    fn op_sbc_page(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, p) = self.read_mode(bus, frame, cfg, sink, mode);
        self.sbc(v);
        base + p as u8
    }

    // Compare A with the addressed operand, updating C/Z/N without changing A.
    // Return the caller-supplied base cycles without a page-crossing penalty.
    fn op_cmp(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, _) = self.read_mode(bus, frame, cfg, sink, mode);
        self.compare(self.a, v);
        base
    }
    // Compare A with the addressed operand, updating C/Z/N without changing A.
    // Add one cycle to the supplied base when the operand address crosses a page.
    fn op_cmp_page(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, p) = self.read_mode(bus, frame, cfg, sink, mode);
        self.compare(self.a, v);
        base + p as u8
    }
    // Compare X with the addressed operand, updating C/Z/N and preserving
    // the register. Return the supplied fixed cycle count.
    fn op_cpx(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, _) = self.read_mode(bus, frame, cfg, sink, mode);
        self.compare(self.x, v);
        base
    }
    // Compare Y with the addressed operand, updating C/Z/N and preserving
    // the register. Return the supplied fixed cycle count.
    fn op_cpy(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let (v, _) = self.read_mode(bus, frame, cfg, sink, mode);
        self.compare(self.y, v);
        base
    }

    // Shift the addressed byte left, updating carry and Z/N.
    // This model performs one read and one final write at the current timestamp;
    // it does not emit an intermediate write of the original value. Return base cycles.
    fn op_asl_mem(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let r = self.resolve_addr(bus, frame, cfg, sink, mode);
        let v = bus.read(r.addr, frame, self.cycles, cfg, sink);
        let n = self.asl_value(v);
        bus.write(r.addr, n, frame, self.cycles, cfg, sink);
        base
    }
    // Shift the addressed byte right with zero fill, updating carry and Z/N.
    // This model performs one read and one final write at the current timestamp;
    // it does not emit an intermediate write of the original value. Return base cycles.
    fn op_lsr_mem(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let r = self.resolve_addr(bus, frame, cfg, sink, mode);
        let v = bus.read(r.addr, frame, self.cycles, cfg, sink);
        let n = self.lsr_value(v);
        bus.write(r.addr, n, frame, self.cycles, cfg, sink);
        base
    }
    // Rotate the addressed byte left through carry, updating carry and Z/N.
    // This model performs one read and one final write at the current timestamp;
    // it does not emit an intermediate write of the original value. Return base cycles.
    fn op_rol_mem(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let r = self.resolve_addr(bus, frame, cfg, sink, mode);
        let v = bus.read(r.addr, frame, self.cycles, cfg, sink);
        let n = self.rol_value(v);
        bus.write(r.addr, n, frame, self.cycles, cfg, sink);
        base
    }
    // Rotate the addressed byte right through carry, updating carry and Z/N.
    // This model performs one read and one final write at the current timestamp;
    // it does not emit an intermediate write of the original value. Return base cycles.
    fn op_ror_mem(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let r = self.resolve_addr(bus, frame, cfg, sink, mode);
        let v = bus.read(r.addr, frame, self.cycles, cfg, sink);
        let n = self.ror_value(v);
        bus.write(r.addr, n, frame, self.cycles, cfg, sink);
        base
    }
    // Decrement the addressed byte with wrapping and update Z/N.
    // This model performs one read and one final write at the current timestamp;
    // it does not emit an intermediate write of the original value. Return base cycles.
    fn op_dec_mem(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let r = self.resolve_addr(bus, frame, cfg, sink, mode);
        let n = bus
            .read(r.addr, frame, self.cycles, cfg, sink)
            .wrapping_sub(1);
        bus.write(r.addr, n, frame, self.cycles, cfg, sink);
        self.set_zn(n);
        base
    }
    // Increment the addressed byte with wrapping and update Z/N.
    // This model performs one read and one final write at the current timestamp;
    // it does not emit an intermediate write of the original value. Return base cycles.
    fn op_inc_mem(
        &mut self,
        bus: &mut Bus,
        frame: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
        mode: AddrMode,
        base: u8,
    ) -> u8 {
        let r = self.resolve_addr(bus, frame, cfg, sink, mode);
        let n = bus
            .read(r.addr, frame, self.cycles, cfg, sink)
            .wrapping_add(1);
        bus.write(r.addr, n, frame, self.cycles, cfg, sink);
        self.set_zn(n);
        base
    }
}

// Return the display-table entry for any opcode byte. A name in this table
// does not imply that the execution match implements that opcode.
pub fn opcode_mnemonic(op: u8) -> &'static str {
    OPCODE_NAMES[op as usize]
}

// Display names also encode operand placeholders used by disassembly:
// # for immediate, d for zero page, a for absolute and uppercase A for
// accumulator. Unofficial labels are descriptive and may remain unimplemented.
const OPCODE_NAMES: [&str; 256] = [
    "BRK",
    "ORA (d,X)",
    "*NOP/ILL",
    "*SLO/ILL",
    "*NOP/ILL",
    "ORA d",
    "ASL d",
    "*SLO/ILL",
    "PHP",
    "ORA #",
    "ASL A",
    "*ANC/ILL",
    "*NOP/ILL",
    "ORA a",
    "ASL a",
    "*SLO/ILL",
    "BPL",
    "ORA (d),Y",
    "*KIL/ILL",
    "*SLO/ILL",
    "*NOP/ILL",
    "ORA d,X",
    "ASL d,X",
    "*SLO/ILL",
    "CLC",
    "ORA a,Y",
    "*NOP/ILL",
    "*SLO/ILL",
    "*NOP/ILL",
    "ORA a,X",
    "ASL a,X",
    "*SLO/ILL",
    "JSR a",
    "AND (d,X)",
    "*NOP/ILL",
    "*RLA/ILL",
    "BIT d",
    "AND d",
    "ROL d",
    "*RLA/ILL",
    "PLP",
    "AND #",
    "ROL A",
    "*ANC/ILL",
    "BIT a",
    "AND a",
    "ROL a",
    "*RLA/ILL",
    "BMI",
    "AND (d),Y",
    "*KIL/ILL",
    "*RLA/ILL",
    "*NOP/ILL",
    "AND d,X",
    "ROL d,X",
    "*RLA/ILL",
    "SEC",
    "AND a,Y",
    "*NOP/ILL",
    "*RLA/ILL",
    "*NOP/ILL",
    "AND a,X",
    "ROL a,X",
    "*RLA/ILL",
    "RTI",
    "EOR (d,X)",
    "*NOP/ILL",
    "*SRE/ILL",
    "*NOP/ILL",
    "EOR d",
    "LSR d",
    "*SRE/ILL",
    "PHA",
    "EOR #",
    "LSR A",
    "*ALR/ILL",
    "JMP a",
    "EOR a",
    "LSR a",
    "*SRE/ILL",
    "BVC",
    "EOR (d),Y",
    "*KIL/ILL",
    "*SRE/ILL",
    "*NOP/ILL",
    "EOR d,X",
    "LSR d,X",
    "*SRE/ILL",
    "CLI",
    "EOR a,Y",
    "*NOP/ILL",
    "*SRE/ILL",
    "*NOP/ILL",
    "EOR a,X",
    "LSR a,X",
    "*SRE/ILL",
    "RTS",
    "ADC (d,X)",
    "*NOP/ILL",
    "*RRA/ILL",
    "*NOP/ILL",
    "ADC d",
    "ROR d",
    "*RRA/ILL",
    "PLA",
    "ADC #",
    "ROR A",
    "*ARR/ILL",
    "JMP (a)",
    "ADC a",
    "ROR a",
    "*RRA/ILL",
    "BVS",
    "ADC (d),Y",
    "*KIL/ILL",
    "*RRA/ILL",
    "*NOP/ILL",
    "ADC d,X",
    "ROR d,X",
    "*RRA/ILL",
    "SEI",
    "ADC a,Y",
    "*NOP/ILL",
    "*RRA/ILL",
    "*NOP/ILL",
    "ADC a,X",
    "ROR a,X",
    "*RRA/ILL",
    "*NOP/ILL",
    "STA (d,X)",
    "*NOP/ILL",
    "*SAX/ILL",
    "STY d",
    "STA d",
    "STX d",
    "*SAX/ILL",
    "DEY",
    "*NOP/ILL",
    "TXA",
    "*XAA/ILL",
    "STY a",
    "STA a",
    "STX a",
    "*SAX/ILL",
    "BCC",
    "STA (d),Y",
    "*KIL/ILL",
    "*AHX/ILL",
    "STY d,X",
    "STA d,X",
    "STX d,Y",
    "*SAX/ILL",
    "TYA",
    "STA a,Y",
    "TXS",
    "*TAS/ILL",
    "*SHY/ILL",
    "STA a,X",
    "*SHX/ILL",
    "*AHX a,Y",
    "LDY #",
    "LDA (d,X)",
    "LDX #",
    "*LAX/ILL",
    "LDY d",
    "LDA d",
    "LDX d",
    "*LAX/ILL",
    "TAY",
    "LDA #",
    "TAX",
    "*LAX/ILL",
    "LDY a",
    "LDA a",
    "LDX a",
    "*LAX/ILL",
    "BCS",
    "LDA (d),Y",
    "*KIL/ILL",
    "*LAX/ILL",
    "LDY d,X",
    "LDA d,X",
    "LDX d,Y",
    "*LAX/ILL",
    "CLV",
    "LDA a,Y",
    "TSX",
    "*LAS/ILL",
    "LDY a,X",
    "LDA a,X",
    "LDX a,Y",
    "*LAX/ILL",
    "CPY #",
    "CMP (d,X)",
    "*NOP/ILL",
    "*DCP/ILL",
    "CPY d",
    "CMP d",
    "DEC d",
    "*DCP/ILL",
    "INY",
    "CMP #",
    "DEX",
    "*AXS/ILL",
    "CPY a",
    "CMP a",
    "DEC a",
    "*DCP/ILL",
    "BNE",
    "CMP (d),Y",
    "*KIL/ILL",
    "*DCP/ILL",
    "*NOP/ILL",
    "CMP d,X",
    "DEC d,X",
    "*DCP/ILL",
    "CLD",
    "CMP a,Y",
    "*NOP/ILL",
    "*DCP/ILL",
    "*NOP/ILL",
    "CMP a,X",
    "DEC a,X",
    "*DCP/ILL",
    "CPX #",
    "SBC (d,X)",
    "*NOP/ILL",
    "*ISC/ILL",
    "CPX d",
    "SBC d",
    "INC d",
    "*ISC/ILL",
    "INX",
    "SBC #",
    "NOP",
    "SBC #*",
    "CPX a",
    "SBC a",
    "INC a",
    "*ISC/ILL",
    "BEQ",
    "SBC (d),Y",
    "*KIL/ILL",
    "*ISC/ILL",
    "*NOP/ILL",
    "SBC d,X",
    "INC d,X",
    "*ISC/ILL",
    "SED",
    "SBC a,Y",
    "*NOP/ILL",
    "*ISC/ILL",
    "*NOP/ILL",
    "SBC a,X",
    "INC a,X",
    "*ISC/ILL",
];
