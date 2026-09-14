use crate::apu::ApuState;
use crate::error::{KurosakiError, Result};
use crate::mapper::Mapper;
use crate::ppu::PpuState;
use crate::trace::{TraceConfig, TraceEvent, TraceSink};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
// Serializable bus-owned state. Array lengths are checked on restoration;
// mapper state and transient CPU trace annotations are not included here.
pub struct BusSnapshot {
    pub ram: Vec<u8>,
    pub ppu: PpuState,
    pub apu: ApuState,
    pub io_regs: Vec<u8>,
    pub controller_state: Vec<u8>,
    pub controller_shift: Vec<u8>,
    pub controller_strobe: bool,
    pub nmi_pending: bool,
    pub irq_pending: bool,
    pub dma_stall_cycles: u64,
    pub joypad_reads: u64,
}

// CPU address decoder and device coordination state. Pending interrupt
// latches and DMA stall accounting are consumed by the CPU/emulator runner.
pub struct Bus {
    pub ram: [u8; 2048],
    pub ppu: PpuState,
    pub apu: ApuState,
    pub io_regs: [u8; 32],
    pub mapper: Box<dyn Mapper>,
    pub controller_state: [u8; 2],
    pub controller_shift: [u8; 2],
    pub controller_strobe: bool,
    pub nmi_pending: bool,
    pub irq_pending: bool,
    pub dma_stall_cycles: u64,
    pub joypad_reads: u64,
    trace_cpu_pc: Option<u16>,
    trace_prg_bank: Option<u16>,
    trace_cpu_opcode: Option<u8>,
}

impl Bus {
    // Initialize RAM, input and pending events to zero and adopt the mapper
    // nametable layout. This constructor does not reset or execute the CPU.
    pub fn new(mapper: Box<dyn Mapper>) -> Self {
        let mut ppu = PpuState::default();
        ppu.set_nametable_mirroring(mapper.nametable_mirroring());
        Self {
            ram: [0; 2048],
            ppu,
            apu: ApuState::default(),
            io_regs: [0; 32],
            mapper,
            controller_state: [0; 2],
            controller_shift: [0; 2],
            controller_strobe: false,
            nmi_pending: false,
            irq_pending: false,
            dma_stall_cycles: 0,
            joypad_reads: 0,
            trace_cpu_pc: None,
            trace_prg_bank: None,
            trace_cpu_opcode: None,
        }
    }

    // Attach the current instruction PC and physical bank to later memory events;
    // clear any opcode retained from the preceding instruction.
    pub(crate) fn set_trace_cpu_context(&mut self, pc: u16, prg_bank: Option<u16>) {
        self.trace_cpu_pc = Some(pc);
        self.trace_prg_bank = prg_bank;
        self.trace_cpu_opcode = None;
    }

    // Attach the fetched opcode to subsequent memory events for this instruction.
    pub(crate) fn set_trace_cpu_opcode(&mut self, opcode: u8) {
        self.trace_cpu_opcode = Some(opcode);
    }

    // Remove transient instruction labels, including after snapshot restoration.
    pub(crate) fn clear_trace_cpu_context(&mut self) {
        self.trace_cpu_pc = None;
        self.trace_prg_bank = None;
        self.trace_cpu_opcode = None;
    }

    // Perform a normal side-effecting bus read with no intra-instruction offset.
    pub fn read(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u8 {
        self.read_timed(addr, frame, cycle, 0, cfg, sink)
    }

    // Decode mirrored CPU RAM, PPU registers, controllers, APU and mapper space.
    // Add the read offset to event time and pass it to the PPU timing helper; this
    // does not itself advance the device clocks. Register reads can change state.
    pub fn read_timed(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cpu_read_offset: u8,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u8 {
        let effective_cycle = cycle.saturating_add(cpu_read_offset as u64);
        let value = match addr {
            0x0000..=0x1FFF => self.ram[(addr as usize) & 0x07FF],
            0x2000..=0x3FFF => {
                self.ppu
                    .set_nametable_mirroring(self.mapper.nametable_mirroring());
                self.ppu.read_register_timed(
                    addr,
                    frame,
                    effective_cycle,
                    cpu_read_offset,
                    cfg,
                    sink,
                )
            }
            0x4016 | 0x4017 => self.read_joypad(addr, frame, effective_cycle, cfg, sink),
            0x4000..=0x4017 => {
                let value = self.apu.read_register(addr);
                // After the APU status-read side effects, clear the bus IRQ latch only
                // when neither current source still requests an interrupt.
                if addr == 0x4015 && !self.apu.irq_pending() && !self.mapper.irq_pending() {
                    self.irq_pending = false;
                }
                value
            }
            0x4018..=0x5FFF => self
                .mapper
                .read_prg(addr, frame, effective_cycle, cfg, sink),
            0x6000..=0xFFFF => self
                .mapper
                .read_prg(addr, frame, effective_cycle, cfg, sink),
        };
        if cfg.mem_read {
            let mut event = TraceEvent::new("mem.read", frame, effective_cycle);
            event.pc = self.trace_cpu_pc;
            event.prg_bank = self.trace_prg_bank;
            event.opcode = self.trace_cpu_opcode;
            event.addr = Some(addr);
            event.value = Some(value);
            sink.push(event);
        }
        value
    }

    // Route CPU writes to RAM or the owning device and then emit the generic
    // memory event when requested. PPU pattern writes also update mapper CHR state;
    // $4014 performs the complete approximate OAM transfer synchronously.
    pub fn write(
        &mut self,
        addr: u16,
        value: u8,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        match addr {
            0x0000..=0x1FFF => self.ram[(addr as usize) & 0x07FF] = value,
            0x2000..=0x3FFF => {
                self.ppu
                    .set_nametable_mirroring(self.mapper.nametable_mirroring());
                if let Some((chr_addr, chr_value)) = self
                    .ppu
                    .write_register(addr, value, frame, cycle, cfg, sink)
                {
                    self.mapper.write_chr(chr_addr, chr_value);
                }
            }
            0x4000..=0x4013 | 0x4015 | 0x4017 => self
                .apu
                .write_register(addr, value, frame, cycle, cfg, sink),
            0x4014 => self.oam_dma(value, frame, cycle, cfg, sink),
            0x4016 => {
                self.io_regs[(addr - 0x4000) as usize] = value;
                // This model latches on a high write; a low write retains the previously
                // latched buttons rather than taking another sample.
                self.controller_strobe = value & 1 != 0;
                if self.controller_strobe {
                    self.latch_controllers();
                }
                if cfg.mem_write {
                    let mut event = TraceEvent::new("input.strobe", frame, cycle);
                    event.addr = Some(addr);
                    event.value = Some(value);
                    event.message = Some(format!("Controller strobe={}", self.controller_strobe));
                    sink.push(event);
                }
            }
            0x4018..=0x5FFF => self.mapper.write_prg(addr, value, frame, cycle, cfg, sink),
            0x6000..=0xFFFF => self.mapper.write_prg(addr, value, frame, cycle, cfg, sink),
        }
        if cfg.mem_write {
            let mut event = TraceEvent::new("mem.write", frame, cycle);
            event.pc = self.trace_cpu_pc;
            event.prg_bank = self.trace_prg_bank;
            event.opcode = self.trace_cpu_opcode;
            event.addr = Some(addr);
            event.value = Some(value);
            sink.push(event);
        }
    }

    // Complete one pending sample fetch through the current memory mapping and
    // accumulate four CPU stall cycles. This helper does not clock those cycles
    // recursively and does not perform PPU/APU register-read side effects.
    fn service_dmc_fetch(
        &mut self,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        if let Some(addr) = self.apu.pending_dmc_fetch_addr() {
            // DMC sample DMA always reads CPU memory. In this first-pass model the
            // actual stall is reported and accumulated, while the byte is fetched
            // through the current PRG mapper window so banked DMC samples are at
            // least deterministic for KITAQFC regression tests.
            let value = match addr {
                0x0000..=0x1FFF => self.ram[(addr as usize) & 0x07FF],
                0x2000..=0x3FFF => 0,
                0x4000..=0x401F => 0,
                0x4020..=0xFFFF => {
                    self.mapper
                        .read_prg(addr, frame, cycle, TraceConfig::none(), sink)
                }
            };
            self.apu
                .complete_dmc_sample_fetch(value, frame, cycle, cfg, sink);
            self.dma_stall_cycles = self.dma_stall_cycles.saturating_add(4);
        }
    }

    // Read a little-endian word with full 16-bit address wrapping. Both byte
    // reads use the supplied timestamp and retain ordinary bus side effects.
    pub fn read_u16(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u16 {
        let lo = self.read(addr, frame, cycle, cfg, sink) as u16;
        let hi = self.read(addr.wrapping_add(1), frame, cycle, cfg, sink) as u16;
        lo | (hi << 8)
    }

    // Read a little-endian pointer while retaining the original page for the
    // high byte, including for indirect-JMP use outside zero page. The existing
    // addr + 1 expression assumes addr is below $FFFF in overflow-checked builds.
    pub fn read_u16_zp_bug(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u16 {
        let lo = self.read(addr, frame, cycle, cfg, sink) as u16;
        let hi_addr = (addr & 0xFF00) | ((addr + 1) & 0x00FF);
        let hi = self.read(hi_addr, frame, cycle, cfg, sink) as u16;
        lo | (hi << 8)
    }

    // Service DMC requests around the APU chunk, then advance PPU and mapper
    // clocks. PPU fetch callbacks notify the mapper before reading CHR. Pending
    // NMI/IRQ flags are latched here; an inactive source does not clear the latch.
    pub fn clock_devices(
        &mut self,
        cpu_cycles: u64,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        self.ppu
            .set_nametable_mirroring(self.mapper.nametable_mirroring());
        self.service_dmc_fetch(frame, cycle, cfg, sink);
        {
            let apu = &mut self.apu;
            let mapper = &mut self.mapper;
            apu.clock_cpu_cycles_with_expansion(
                cpu_cycles,
                frame,
                cycle,
                cfg,
                sink,
                |sample_rate| mapper.expansion_audio_sample(sample_rate),
            );
        }
        self.service_dmc_fetch(frame, cycle.saturating_add(cpu_cycles), cfg, sink);
        {
            let mapper = &mut self.mapper;
            let nmi = self.ppu.clock_cpu_cycles(
                cpu_cycles,
                frame,
                cycle,
                cfg,
                sink,
                |addr, f, c, tc, ts| {
                    mapper.notify_ppu_addr(addr, f, c, tc, ts);
                    mapper.read_chr(addr)
                },
            );
            if nmi {
                self.nmi_pending = true;
            }
        }
        self.mapper.clock_cpu(cpu_cycles, frame, cycle, cfg, sink);
        if self.mapper.irq_pending() || self.apu.irq_pending() {
            self.irq_pending = true;
        }
    }

    // Request the PPU vblank transition and latch any NMI it generates.
    pub fn begin_vblank(&mut self, frame: u64, cycle: u64, cfg: TraceConfig, sink: &mut TraceSink) {
        if self.ppu.begin_vblank(frame, cycle, cfg, sink) {
            self.nmi_pending = true;
        }
    }

    // Consume the bus NMI latch exactly once; this does not change PPU status.
    pub fn take_nmi(&mut self) -> bool {
        let v = self.nmi_pending;
        self.nmi_pending = false;
        v
    }

    // Combine the bus latch with current mapper/APU IRQ sources, then clear the
    // bus latch and acknowledge a pending mapper IRQ. APU sources are left for
    // their register-defined acknowledgment paths.
    pub fn take_irq(&mut self) -> bool {
        let mapper_irq = self.mapper.irq_pending();
        let apu_irq = self.apu.irq_pending();
        let v = self.irq_pending || mapper_irq || apu_irq;
        self.irq_pending = false;
        if mapper_irq {
            self.mapper.clear_irq();
        }
        // DMC IRQ intentionally remains pending until software clears it through
        // $4015/$4010 semantics, matching the behavior games expect.
        v
    }

    // Clone RAM, device and controller state plus pending events and counters.
    // Mapper state is stored separately by the emulator; trace labels are omitted.
    pub fn to_snapshot(&self) -> BusSnapshot {
        BusSnapshot {
            ram: self.ram.to_vec(),
            ppu: self.ppu.clone(),
            apu: self.apu.clone(),
            io_regs: self.io_regs.to_vec(),
            controller_state: self.controller_state.to_vec(),
            controller_shift: self.controller_shift.to_vec(),
            controller_strobe: self.controller_strobe,
            nmi_pending: self.nmi_pending,
            irq_pending: self.irq_pending,
            dma_stall_cycles: self.dma_stall_cycles,
            joypad_reads: self.joypad_reads,
        }
    }

    // Check all fixed-array lengths before changing bus state. Restore device
    // and input data, reapply the current mapper mirroring and clear trace labels.
    // The caller restores mapper state separately; nested device data is trusted.
    pub fn restore_snapshot(&mut self, snapshot: &BusSnapshot) -> Result<()> {
        if snapshot.ram.len() != self.ram.len() {
            return Err(KurosakiError::SnapshotFormat(format!(
                "CPU RAM length mismatch: expected {}, got {}",
                self.ram.len(),
                snapshot.ram.len()
            )));
        }
        if snapshot.io_regs.len() != self.io_regs.len() {
            return Err(KurosakiError::SnapshotFormat(format!(
                "I/O register length mismatch: expected {}, got {}",
                self.io_regs.len(),
                snapshot.io_regs.len()
            )));
        }
        if snapshot.controller_state.len() != self.controller_state.len()
            || snapshot.controller_shift.len() != self.controller_shift.len()
        {
            return Err(KurosakiError::SnapshotFormat(
                "controller state length mismatch".to_string(),
            ));
        }

        self.ram.copy_from_slice(&snapshot.ram);
        self.ppu = snapshot.ppu.clone();
        self.ppu
            .set_nametable_mirroring(self.mapper.nametable_mirroring());
        self.apu = snapshot.apu.clone();
        self.io_regs.copy_from_slice(&snapshot.io_regs);
        self.controller_state
            .copy_from_slice(&snapshot.controller_state);
        self.controller_shift
            .copy_from_slice(&snapshot.controller_shift);
        self.controller_strobe = snapshot.controller_strobe;
        self.nmi_pending = snapshot.nmi_pending;
        self.irq_pending = snapshot.irq_pending;
        self.dma_stall_cycles = snapshot.dma_stall_cycles;
        self.joypad_reads = snapshot.joypad_reads;
        self.clear_trace_cpu_context();
        Ok(())
    }

    // Replace both live button masks; refresh serial latches immediately only
    // while strobe is high.
    pub fn set_controller_state(&mut self, pad1: u8, pad2: u8) {
        self.controller_state = [pad1, pad2];
        if self.controller_strobe {
            self.latch_controllers();
        }
    }

    // Copy live button masks into the two serial shift registers.
    fn latch_controllers(&mut self) {
        self.controller_shift = self.controller_state;
    }

    // Return bit zero with bit six forced high. High strobe reads the live mask;
    // low strobe shifts the latch right and fills with ones. Count the read and
    // notify the APU before optionally emitting the input event.
    fn read_joypad(
        &mut self,
        addr: u16,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u8 {
        self.joypad_reads = self.joypad_reads.saturating_add(1);
        self.apu.observe_joypad_read(frame, cycle, cfg, sink);
        let pad = if addr == 0x4016 { 0 } else { 1 };
        let bit = if self.controller_strobe {
            self.controller_state[pad] & 1
        } else {
            let value = self.controller_shift[pad] & 1;
            self.controller_shift[pad] = (self.controller_shift[pad] >> 1) | 0x80;
            value
        };
        let value = 0x40 | bit;
        if cfg.mem_read {
            let mut event = TraceEvent::new("input.joypad_read", frame, cycle);
            event.addr = Some(addr);
            event.value = Some(value);
            event.message = Some(format!(
                "Joypad read count={} pad={} shift={:02X}",
                self.joypad_reads,
                pad + 1,
                self.controller_shift[pad]
            ));
            sink.push(event);
        }
        value
    }

    // Read 256 bytes through the bus into OAM indices 0..255, suppressing read
    // traces but retaining read side effects. This model ignores OAM start offset
    // and CPU parity, adds a fixed 513-cycle stall, and clocks devices immediately.
    fn oam_dma(
        &mut self,
        page: u8,
        frame: u64,
        cycle: u64,
        cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        let base = (page as u16) << 8;
        for i in 0..=255u16 {
            let value = self.read(
                base.wrapping_add(i),
                frame,
                cycle,
                TraceConfig::none(),
                sink,
            );
            self.ppu.oam[i as usize] = value;
        }
        self.dma_stall_cycles += 513;
        self.clock_devices(513, frame, cycle, cfg, sink);
        if cfg.dma {
            let mut event = TraceEvent::new("dma.oam", frame, cycle);
            event.addr = Some(base);
            event.value = Some(page);
            event.message = Some("OAM DMA executed; CPU stall approximated as 513 cycles and clocked through PPU/APU/mapper".to_string());
            sink.push(event);
        }
    }
}
