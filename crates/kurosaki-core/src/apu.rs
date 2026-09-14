use crate::trace::{TraceConfig, TraceEvent, TraceSink};
use serde::{Deserialize, Serialize};

const CPU_HZ_NTSC: f64 = 1_789_773.0;
const DEFAULT_SAMPLE_RATE: u32 = 44_100;
const MAX_RECENT_AUDIO_SECONDS: usize = 1;
const FRAME_4STEP_PERIOD_CPU_CYCLES: f64 = 29_830.0;
const FRAME_5STEP_PERIOD_CPU_CYCLES: f64 = 37_282.0;
// Frame event tuples contain CPU-cycle position, quarter-clock, half-clock
// and IRQ flags. Both schedules below use the NTSC clock policy.
const FRAME_4STEP_EVENTS: [(f64, bool, bool, bool); 4] = [
    (7_457.0, true, false, false),
    (14_913.0, true, true, false),
    (22_371.0, true, false, false),
    (29_829.0, true, true, true),
];
const FRAME_5STEP_EVENTS: [(f64, bool, bool, bool); 4] = [
    (7_457.0, true, false, false),
    (14_913.0, true, true, false),
    (22_371.0, true, false, false),
    (37_281.0, true, true, false),
];

// Binary DAC gates for each of the four eight-step pulse duty patterns.
const PULSE_DUTY_TABLE: [[u8; 8]; 4] = [
    [0, 1, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 0, 0, 0],
    [1, 0, 0, 1, 1, 1, 1, 1],
];

// Thirty-two DAC levels for one triangle sequence cycle, read by phase index.
const TRIANGLE_SEQUENCE: [u8; 32] = [
    15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    13, 14, 15,
];

const NOISE_PERIOD_CPU_CYCLES: [f64; 16] = [
    4.0, 8.0, 16.0, 32.0, 64.0, 96.0, 128.0, 160.0, 202.0, 254.0, 380.0, 508.0, 762.0, 1016.0,
    2034.0, 4068.0,
];

// Length-counter reload values selected by bits 3..7 of a length-load write.
const LENGTH_TABLE: [u8; 32] = [
    10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, 12, 16, 24, 18, 48, 20, 96, 22,
    192, 24, 72, 26, 16, 28, 32, 30,
];

const DMC_RATE_CPU_CYCLES_NTSC: [u16; 16] = [
    428, 380, 340, 320, 286, 254, 226, 214, 190, 160, 142, 128, 106, 85, 72, 54,
];

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
// Divider/decay and deferred restart request for one pulse or noise envelope.
struct EnvelopeState {
    divider: u8,
    decay: u8,
    start: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// Serializable synthesizer and diagnostic state. Public register/sample
// fields are assumed well-formed by internal helpers; deserialization itself
// does not validate their lengths or clock parameters.
pub struct ApuState {
    /// Raw $4000-$4017 register mirror. This stays public because existing JSON
    /// snapshots and diagnostics consume it directly.
    pub regs: Vec<u8>,
    pub frame_counter: u8,
    pub write_count: u64,
    pub sample_rate: u32,
    pub sample_buffer: Vec<i16>,
    /// Opt-in offline capture; normal execution retains only recent audio.
    #[serde(skip)]
    pub capture_full_audio: bool,
    pub sample_clock_accum: f64,
    pub generated_samples: u64,
    pub dmc_enabled: bool,
    pub dmc_dma_stall_cycles: u64,
    pub joypad_reads_while_dmc_enabled: u64,
    pub dmc_joypad_conflicts: u64,

    /// Phase/counter state for the clean-room 2A03 audio synthesizer.
    pulse_phase: [f64; 2],
    triangle_phase: f64,
    noise_lfsr: u16,
    noise_clock_accum: f64,
    length_counter: [u8; 4],
    envelopes: [EnvelopeState; 3],
    triangle_linear_counter: u8,
    triangle_linear_reload: bool,
    pulse_sweep_divider: [u8; 2],
    pulse_sweep_reload: [bool; 2],
    pulse_timer_accum: [f64; 2],
    pulse_seq_step: [u8; 2],
    triangle_timer_accum: f64,
    triangle_seq_step: u8,
    frame_cycle_accum: f64,
    frame_step: u8,
    frame_irq_flag: bool,
    pub frame_irq_events: u64,

    /// First-pass DMC playback unit. This models sample address/length, bit
    /// shifting, looping, output level, and fetch requests so DMC no longer acts
    /// only as a direct $4011 level. CPU-cycle-exact DMA timing is still surfaced
    /// as diagnostics rather than stalling every individual CPU microcycle.
    dmc_irq_flag: bool,
    dmc_loop_flag: bool,
    dmc_output_level: u8,
    dmc_sample_addr: u16,
    dmc_sample_length: u16,
    dmc_current_addr: u16,
    dmc_bytes_remaining: u16,
    dmc_sample_buffer: Option<u8>,
    dmc_shift_reg: u8,
    dmc_bits_remaining: u8,
    dmc_silence: bool,
    dmc_timer_accum: f64,
    pub dmc_sample_fetches: u64,
    pub dmc_irq_events: u64,
    pub dmc_bits_output: u64,
    pub dmc_silence_ticks: u64,

    /// DC blocker for the approximate non-linear NES mixer. The hardware audio
    /// path is AC-coupled; this keeps deterministic debug WAVs from drifting
    /// upward when many channels output positive DAC values.
    dc_last_input: f64,
    dc_last_output: f64,
}

impl Default for ApuState {
    // Initialize silent channels and empty recent audio at 44.1 kHz, seed the
    // noise LFSR with one and clear sequencer, DMC, IRQ and filter state.
    fn default() -> Self {
        Self {
            regs: vec![0; 0x18],
            frame_counter: 0,
            write_count: 0,
            sample_rate: DEFAULT_SAMPLE_RATE,
            sample_buffer: Vec::new(),
            capture_full_audio: false,
            sample_clock_accum: 0.0,
            generated_samples: 0,
            dmc_enabled: false,
            dmc_dma_stall_cycles: 0,
            joypad_reads_while_dmc_enabled: 0,
            dmc_joypad_conflicts: 0,
            pulse_phase: [0.0, 0.0],
            triangle_phase: 0.0,
            noise_lfsr: 1,
            noise_clock_accum: 0.0,
            length_counter: [0; 4],
            envelopes: [EnvelopeState::default(); 3],
            triangle_linear_counter: 0,
            triangle_linear_reload: false,
            pulse_sweep_divider: [0; 2],
            pulse_sweep_reload: [false; 2],
            pulse_timer_accum: [0.0; 2],
            pulse_seq_step: [0; 2],
            triangle_timer_accum: 0.0,
            triangle_seq_step: 0,
            frame_cycle_accum: 0.0,
            frame_step: 0,
            frame_irq_flag: false,
            frame_irq_events: 0,
            dmc_irq_flag: false,
            dmc_loop_flag: false,
            dmc_output_level: 0,
            dmc_sample_addr: 0xC000,
            dmc_sample_length: 1,
            dmc_current_addr: 0xC000,
            dmc_bytes_remaining: 0,
            dmc_sample_buffer: None,
            dmc_shift_reg: 0,
            dmc_bits_remaining: 0,
            dmc_silence: true,
            dmc_timer_accum: 0.0,
            dmc_sample_fetches: 0,
            dmc_irq_events: 0,
            dmc_bits_output: 0,
            dmc_silence_ticks: 0,
            dc_last_input: 0.0,
            dc_last_output: 0.0,
        }
    }
}

impl ApuState {
    // Build $4015 status from length counters, remaining DMC bytes and IRQ
    // flags, then clear only the frame IRQ latch. Other addresses read as zero.
    pub fn read_register(&mut self, addr: u16) -> u8 {
        match addr {
            0x4015 => {
                let mut status = 0u8;
                if self.length_counter[0] > 0 {
                    status |= 0x01;
                }
                if self.length_counter[1] > 0 {
                    status |= 0x02;
                }
                if self.length_counter[2] > 0 {
                    status |= 0x04;
                }
                if self.length_counter[3] > 0 {
                    status |= 0x08;
                }
                if self.dmc_bytes_remaining > 0 {
                    status |= 0x10;
                }
                if self.frame_irq_flag {
                    status |= 0x40;
                }
                if self.dmc_irq_flag {
                    status |= 0x80;
                }
                self.frame_irq_flag = false;
                status
            }
            _ => 0,
        }
    }

    // Mirror writes in $4000-$4017, apply the listed channel/control effects,
    // and optionally emit an APU event. Writes are applied immediately rather
    // than scheduled within a CPU-cycle pipeline; other addresses are ignored.
    pub fn write_register(
        &mut self,
        addr: u16,
        value: u8,
        frame: u64,
        cycle: u64,
        trace_cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        if (0x4000..=0x4017).contains(&addr) {
            self.regs[(addr - 0x4000) as usize] = value;
            match addr {
                0x4000 => self.envelopes[0].start = true,
                0x4001 => self.pulse_sweep_reload[0] = true,
                0x4003 => {
                    self.load_length(0, value);
                    self.envelopes[0].start = true;
                    self.pulse_phase[0] = 0.0;
                    self.pulse_timer_accum[0] = 0.0;
                    self.pulse_seq_step[0] = 0;
                }
                0x4004 => self.envelopes[1].start = true,
                0x4005 => self.pulse_sweep_reload[1] = true,
                0x4007 => {
                    self.load_length(1, value);
                    self.envelopes[1].start = true;
                    self.pulse_phase[1] = 0.0;
                    self.pulse_timer_accum[1] = 0.0;
                    self.pulse_seq_step[1] = 0;
                }
                0x400B => {
                    self.load_length(2, value);
                    self.triangle_linear_reload = true;
                    self.triangle_phase = 0.0;
                    self.triangle_timer_accum = 0.0;
                    self.triangle_seq_step = 0;
                }
                0x400C => self.envelopes[2].start = true,
                0x400F => {
                    self.load_length(3, value);
                    self.envelopes[2].start = true;
                    self.noise_lfsr = 1;
                }
                0x4010 => {
                    self.dmc_loop_flag = value & 0x40 != 0;
                    if value & 0x80 == 0 {
                        self.dmc_irq_flag = false;
                    }
                }
                0x4011 => self.dmc_output_level = value & 0x7F,
                0x4012 => self.dmc_sample_addr = 0xC000u16.wrapping_add((value as u16) << 6),
                0x4013 => self.dmc_sample_length = ((value as u16) << 4).wrapping_add(1),
                // Clear the DMC IRQ and disable selected length counters immediately.
                // A newly enabled DMC restarts timing; disabled playback clears remaining
                // bytes and its pending buffer but retains the DAC output level.
                0x4015 => {
                    self.dmc_irq_flag = false;
                    let was_dmc_enabled = self.dmc_enabled;
                    self.dmc_enabled = value & 0x10 != 0;
                    for (idx, bit) in [0x01, 0x02, 0x04, 0x08].iter().enumerate() {
                        if value & *bit == 0 {
                            self.length_counter[idx] = 0;
                        }
                    }
                    if self.dmc_enabled {
                        if !was_dmc_enabled {
                            self.dmc_timer_accum = 0.0;
                            self.dmc_bits_remaining = 0;
                            self.dmc_silence = true;
                        }
                        if self.dmc_bytes_remaining == 0 {
                            self.restart_dmc_sample();
                        }
                    } else {
                        self.dmc_bytes_remaining = 0;
                        self.dmc_sample_buffer = None;
                    }
                }
                // Reset the frame-sequencer phase immediately. Five-step selection also
                // clocks quarter/half units now; hardware write-delay timing is not modeled.
                0x4017 => {
                    self.frame_counter = value;
                    self.frame_cycle_accum = 0.0;
                    self.frame_step = 0;
                    if value & 0x40 != 0 {
                        self.frame_irq_flag = false;
                    }
                    if value & 0x80 != 0 {
                        self.clock_quarter_frame();
                        self.clock_half_frame();
                    }
                }
                _ => {}
            }
            self.write_count += 1;
            if trace_cfg.apu {
                let mut event = TraceEvent::new("apu.reg_write", frame, cycle);
                event.addr = Some(addr);
                event.value = Some(value);
                if (0x4010..=0x4013).contains(&addr) || addr == 0x4015 {
                    event.message =
                        Some(format!("DMC state changed; enabled={}", self.dmc_enabled));
                }
                sink.push(event);
            }
        }
    }

    // Advance the APU using a zero-valued expansion-audio callback.
    pub fn clock_cpu_cycles(
        &mut self,
        cpu_cycles: u64,
        frame: u64,
        cycle: u64,
        trace_cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        self.clock_cpu_cycles_with_expansion(
            cpu_cycles,
            frame,
            cycle,
            trace_cfg,
            sink,
            |_sample_rate| 0,
        );
    }

    // Advance frame, DMC and oscillator state for the whole CPU-cycle chunk,
    // then emit each due sample from that resulting state. This chunked ordering
    // is not per-sample interpolation; use bounded chunks for normal execution.
    // Keep recent audio unless full capture is explicitly enabled.
    pub fn clock_cpu_cycles_with_expansion<F>(
        &mut self,
        cpu_cycles: u64,
        frame: u64,
        cycle: u64,
        trace_cfg: TraceConfig,
        sink: &mut TraceSink,
        mut expansion_sample: F,
    ) where
        F: FnMut(u32) -> i16,
    {
        if cpu_cycles == 0 {
            return;
        }
        self.clock_frame_sequencer(cpu_cycles);
        self.clock_dmc_output(cpu_cycles);
        self.clock_oscillators(cpu_cycles);

        // Convert CPU time to sample deadlines. The public rate is used directly;
        // callers should provide a sensible nonzero rate before clocking.
        let cycles_per_sample = CPU_HZ_NTSC / self.sample_rate as f64;
        self.sample_clock_accum += cpu_cycles as f64;
        let mut produced = 0u32;
        while self.sample_clock_accum >= cycles_per_sample {
            self.sample_clock_accum -= cycles_per_sample;
            // Invoke the mapper callback once for every emitted sample, after the
            // base channel state has advanced for this entire input chunk.
            let exp = expansion_sample(self.sample_rate);
            let sample = self.mix_sample(exp);
            self.sample_buffer.push(sample);
            self.generated_samples += 1;
            produced += 1;

            // Keep snapshots and JSON reports bounded while preserving recent audio.
            let max_recent = (self.sample_rate as usize * MAX_RECENT_AUDIO_SECONDS).max(1024);
            if !self.capture_full_audio && self.sample_buffer.len() > max_recent {
                let drain = self.sample_buffer.len() - max_recent;
                self.sample_buffer.drain(0..drain);
            }
        }
        if produced > 0 && trace_cfg.apu && self.generated_samples % 2048 < produced as u64 {
            let mut event = TraceEvent::new("apu.samples", frame, cycle);
            event.message = Some(format!(
                "Generated {} audio samples total; recent buffer={} samples",
                self.generated_samples,
                self.sample_buffer.len()
            ));
            sink.push(event);
        }
    }

    // Count every controller read while DMC is enabled as a conservative
    // conflict candidate, regardless of actual DMA overlap. Emitted events use
    // $4016 as a fixed label because this helper receives no controller address.
    pub fn observe_joypad_read(
        &mut self,
        frame: u64,
        cycle: u64,
        trace_cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        if self.dmc_enabled {
            self.joypad_reads_while_dmc_enabled += 1;
            // Conservative diagnostic signal: any joypad read while DMC is enabled is worth surfacing.
            self.dmc_joypad_conflicts += 1;
            if trace_cfg.apu || trace_cfg.dma {
                let mut event = TraceEvent::new("apu.dmc_joypad_conflict", frame, cycle);
                event.addr = Some(0x4016);
                event.severity = Some("warn".to_string());
                event.message = Some("Joypad read while DMC is enabled; this is a known NES hardware hazard and should be validated with a DMC-safe input routine.".to_string());
                sink.push(event);
            }
        }
    }

    // Load the indexed length-table entry only when the channel is enabled
    // in the $4015 mirror. Channel is an internal index in 0..4.
    fn load_length(&mut self, channel: usize, value: u8) {
        let enable_mask = [0x01, 0x02, 0x04, 0x08][channel];
        if self.regs[0x15] & enable_mask != 0 {
            let idx = (value >> 3) as usize;
            self.length_counter[channel] = LENGTH_TABLE[idx & 31];
        }
    }

    // Split the elapsed chunk at four/five-step period boundaries, clock all
    // crossed frame events in order, and retain the fractional-cycle remainder.
    fn clock_frame_sequencer(&mut self, cpu_cycles: u64) {
        let mode_5step = self.frame_counter & 0x80 != 0;
        let period = if mode_5step {
            FRAME_5STEP_PERIOD_CPU_CYCLES
        } else {
            FRAME_4STEP_PERIOD_CPU_CYCLES
        };
        let mut start = self.frame_cycle_accum;
        let mut end = self.frame_cycle_accum + cpu_cycles as f64;
        while end >= period {
            self.clock_frame_events_between(start, period, mode_5step);
            end -= period;
            start = 0.0;
        }
        self.clock_frame_events_between(start, end, mode_5step);
        self.frame_cycle_accum = end;
    }

    // Process points in the half-open/closed interval (start,end], clocking
    // quarter before half events and asserting enabled frame IRQs afterward.
    fn clock_frame_events_between(&mut self, start: f64, end: f64, mode_5step: bool) {
        let events = if mode_5step {
            &FRAME_5STEP_EVENTS[..]
        } else {
            &FRAME_4STEP_EVENTS[..]
        };
        for (point, quarter, half, irq) in events {
            if *point > start && *point <= end {
                if *quarter {
                    self.clock_quarter_frame();
                }
                if *half {
                    self.clock_half_frame();
                }
                if *irq && self.frame_counter & 0x40 == 0 {
                    self.frame_irq_flag = true;
                    self.frame_irq_events = self.frame_irq_events.saturating_add(1);
                }
            }
        }
    }

    // Clock both pulse envelopes, the noise envelope and triangle linear counter.
    fn clock_quarter_frame(&mut self) {
        self.clock_envelope(0, self.regs[0x00]);
        self.clock_envelope(1, self.regs[0x04]);
        self.clock_envelope(2, self.regs[0x0C]);
        self.clock_triangle_linear();
    }

    // Clock all four length counters followed by both pulse sweep units.
    fn clock_half_frame(&mut self) {
        self.clock_length_counter(0, self.regs[0x00]);
        self.clock_length_counter(1, self.regs[0x04]);
        self.clock_length_counter(2, self.regs[0x08]);
        self.clock_length_counter(3, self.regs[0x0C]);
        self.clock_sweep(0);
        self.clock_sweep(1);
    }

    // On restart, load decay 15 and the divider; otherwise count down, decay
    // on divider expiry and reload decay only when the loop bit is enabled.
    fn clock_envelope(&mut self, idx: usize, control: u8) {
        let env = &mut self.envelopes[idx];
        let period = control & 0x0F;
        let loop_flag = control & 0x20 != 0;
        if env.start {
            env.start = false;
            env.decay = 15;
            env.divider = period;
        } else if env.divider == 0 {
            env.divider = period;
            if env.decay > 0 {
                env.decay -= 1;
            } else if loop_flag {
                env.decay = 15;
            }
        } else {
            env.divider -= 1;
        }
    }

    // Reload or decrement the linear counter, then clear its reload request
    // when the control/halt bit is unset.
    fn clock_triangle_linear(&mut self) {
        let control = self.regs[0x08];
        if self.triangle_linear_reload {
            self.triangle_linear_counter = control & 0x7F;
        } else if self.triangle_linear_counter > 0 {
            self.triangle_linear_counter -= 1;
        }
        if control & 0x80 == 0 {
            self.triangle_linear_reload = false;
        }
    }

    // Decrement a nonzero length unless halted. Triangle uses control bit
    // seven; pulse/noise use the envelope-loop bit five.
    fn clock_length_counter(&mut self, channel: usize, control: u8) {
        let halt = if channel == 2 {
            control & 0x80 != 0
        } else {
            control & 0x20 != 0
        };
        if !halt && self.length_counter[channel] > 0 {
            self.length_counter[channel] -= 1;
        }
    }

    // Handle reload/divider timing first, then apply enabled nonzero shifts
    // within the accepted timer range. Negation subtracts one extra unit on
    // pulse one; an out-of-range target leaves the current timer unchanged.
    fn clock_sweep(&mut self, channel: usize) {
        let base = channel * 4;
        let sweep = self.regs[base + 1];
        let enabled = sweep & 0x80 != 0;
        let period = ((sweep >> 4) & 0x07) + 1;
        let shift = sweep & 0x07;
        if self.pulse_sweep_reload[channel] {
            self.pulse_sweep_divider[channel] = period;
            self.pulse_sweep_reload[channel] = false;
            return;
        }
        if self.pulse_sweep_divider[channel] > 0 {
            self.pulse_sweep_divider[channel] -= 1;
            return;
        }
        self.pulse_sweep_divider[channel] = period;
        if enabled && shift > 0 {
            let timer = self.pulse_timer(channel);
            if timer >= 8 {
                let change = timer >> shift;
                let target = if sweep & 0x08 != 0 {
                    timer as i32 - change as i32 - if channel == 0 { 1 } else { 0 }
                } else {
                    timer as i32 + change as i32
                };
                if (8..=0x07FF).contains(&target) {
                    self.set_pulse_timer(channel, target as u16);
                }
            }
        }
    }

    // Advance the two pulse sequencers, triangle sequencer and noise LFSR
    // using the same elapsed CPU-cycle chunk.
    fn clock_oscillators(&mut self, cpu_cycles: u64) {
        self.clock_pulse_oscillator(0, cpu_cycles);
        self.clock_pulse_oscillator(1, cpu_cycles);
        self.clock_triangle_oscillator(cpu_cycles);
        self.clock_noise_oscillator(cpu_cycles);
    }

    // Advance the eight-step duty sequencer every 2*(timer+1) cycles while
    // the channel is enabled, has length and is not sweep-muted. Disabled or
    // muted paths return without advancing their phase accumulator.
    fn clock_pulse_oscillator(&mut self, channel: usize, cpu_cycles: u64) {
        let status_bit = if channel == 0 { 0x01 } else { 0x02 };
        if self.regs[0x15] & status_bit == 0
            || self.length_counter[channel] == 0
            || self.pulse_sweep_mutes(channel)
        {
            return;
        }
        let timer = self.pulse_timer(channel);
        if timer < 8 {
            return;
        }
        let period = 2.0 * (timer as f64 + 1.0);
        self.pulse_timer_accum[channel] += cpu_cycles as f64;
        while self.pulse_timer_accum[channel] >= period {
            self.pulse_timer_accum[channel] -= period;
            self.pulse_seq_step[channel] = self.pulse_seq_step[channel].wrapping_add(1) & 7;
        }
    }

    // Advance the 32-step sequence every timer+1 cycles while enable, length
    // and linear counters permit it. Timers below two return without advancing.
    fn clock_triangle_oscillator(&mut self, cpu_cycles: u64) {
        if self.regs[0x15] & 0x04 == 0
            || self.length_counter[2] == 0
            || self.triangle_linear_counter == 0
        {
            return;
        }
        let timer = self.regs[0x0A] as u16 | (((self.regs[0x0B] & 0x07) as u16) << 8);
        if timer < 2 {
            return;
        }
        let period = timer as f64 + 1.0;
        self.triangle_timer_accum += cpu_cycles as f64;
        while self.triangle_timer_accum >= period {
            self.triangle_timer_accum -= period;
            self.triangle_seq_step = self.triangle_seq_step.wrapping_add(1) & 31;
        }
    }

    // Use the selected period to advance a 15-bit shift register with tap one
    // or six. This model freezes disabled/expired noise and reseeds a zero LFSR.
    fn clock_noise_oscillator(&mut self, cpu_cycles: u64) {
        if self.regs[0x15] & 0x08 == 0 || self.length_counter[3] == 0 {
            return;
        }
        let period_index = (self.regs[0x0E] & 0x0F) as usize;
        let period = NOISE_PERIOD_CPU_CYCLES[period_index];
        self.noise_clock_accum += cpu_cycles as f64 / period;
        while self.noise_clock_accum >= 1.0 {
            self.noise_clock_accum -= 1.0;
            let tap_bit = if self.regs[0x0E] & 0x80 != 0 { 6 } else { 1 };
            let feedback = (self.noise_lfsr ^ (self.noise_lfsr >> tap_bit)) & 1;
            self.noise_lfsr = (self.noise_lfsr >> 1) | (feedback << 14);
            if self.noise_lfsr == 0 {
                self.noise_lfsr = 1;
            }
        }
    }

    // Choose the control nibble for constant volume or the current envelope
    // decay level, returning unscaled DAC units.
    fn envelope_volume_units(&self, idx: usize, control: u8) -> f64 {
        let value = if control & 0x10 != 0 {
            control & 0x0F
        } else {
            self.envelopes[idx].decay
        };
        value as f64
    }

    // Combine pulse and triangle/noise/DMC through approximate nonlinear
    // response curves, scale and DC-block the base audio, then add the mapper
    // sample and clamp to signed 16-bit mono. Expansion audio bypasses this filter.
    fn mix_sample(&mut self, expansion: i16) -> i16 {
        let pulse1 = self.pulse_sample(0);
        let pulse2 = self.pulse_sample(1);
        let triangle = self.triangle_sample();
        let noise = self.noise_sample();
        let dmc = self.dmc_sample();

        // NES-like non-linear mixer approximation. This follows the commonly used
        // pulse and TND response curves instead of simply summing channels. It is
        // still clean-room/debug-oriented, but it preserves relative loudness much
        // better when DMC, triangle, noise, VRC6, and FDS play together.
        let pulse_sum = pulse1 + pulse2;
        let pulse_out = if pulse_sum > 0.0 {
            95.88 / ((8128.0 / pulse_sum) + 100.0)
        } else {
            0.0
        };
        let tnd_input = triangle / 8227.0 + noise / 12241.0 + dmc / 22638.0;
        let tnd_out = if tnd_input > 0.0 {
            159.79 / ((1.0 / tnd_input) + 100.0)
        } else {
            0.0
        };
        let mono = (pulse_out + tnd_out) * 45_000.0;
        let nes = self.dc_block(mono);
        let mixed = nes + expansion as f64;
        mixed.clamp(i16::MIN as f64, i16::MAX as f64) as i16
    }

    // Apply the stateful first-order DC blocker with fixed coefficient 0.995.
    // Its coefficient is not recalculated when the public sample rate changes.
    fn dc_block(&mut self, input: f64) -> f64 {
        let out = input - self.dc_last_input + 0.995 * self.dc_last_output;
        self.dc_last_input = input;
        self.dc_last_output = out;
        out
    }

    // Combine the pulse timer low byte with the three high register bits.
    fn pulse_timer(&self, channel: usize) -> u16 {
        let base = channel * 4;
        self.regs[base + 2] as u16 | (((self.regs[base + 3] & 0x07) as u16) << 8)
    }

    // Replace only the eleven timer bits, preserving the other high-register bits.
    fn set_pulse_timer(&mut self, channel: usize, timer: u16) {
        let base = channel * 4;
        self.regs[base + 2] = timer as u8;
        self.regs[base + 3] = (self.regs[base + 3] & !0x07) | ((timer >> 8) as u8 & 0x07);
    }

    // Mute low timers or enabled nonzero-shift sweeps whose calculated target
    // falls outside 8..$7FF. Disabled/zero-shift sweeps do not cause target muting
    // in this model.
    fn pulse_sweep_mutes(&self, channel: usize) -> bool {
        let timer = self.pulse_timer(channel);
        let sweep = self.regs[channel * 4 + 1];
        let shift = sweep & 0x07;
        if timer < 8 {
            return true;
        }
        if shift == 0 || sweep & 0x80 == 0 {
            return false;
        }
        let change = timer >> shift;
        let target = if sweep & 0x08 != 0 {
            timer as i32 - change as i32 - if channel == 0 { 1 } else { 0 }
        } else {
            timer as i32 + change as i32
        };
        !(8..=0x07FF).contains(&target)
    }

    // Read the current duty-table step and envelope/constant DAC level after
    // enable, length and sweep checks. Sampling does not advance the sequencer.
    fn pulse_sample(&mut self, channel: usize) -> f64 {
        let status_bit = if channel == 0 { 0x01 } else { 0x02 };
        if self.regs[0x15] & status_bit == 0 || self.length_counter[channel] == 0 {
            return 0.0;
        }
        if self.pulse_sweep_mutes(channel) {
            return 0.0;
        }
        let base = channel * 4;
        let control = self.regs[base];
        let duty = ((control >> 6) & 0x03) as usize;
        let volume = self.envelope_volume_units(channel, control);
        if volume <= 0.0 {
            return 0.0;
        }
        let step = self.pulse_seq_step[channel] as usize & 7;
        if PULSE_DUTY_TABLE[duty][step] != 0 {
            volume
        } else {
            0.0
        }
    }

    // Return the current triangle-table level while enable/counters/timer
    // permit it. This model returns zero when gated rather than holding a DAC level.
    fn triangle_sample(&mut self) -> f64 {
        if self.regs[0x15] & 0x04 == 0
            || self.length_counter[2] == 0
            || self.triangle_linear_counter == 0
        {
            return 0.0;
        }
        let timer = self.regs[0x0A] as u16 | (((self.regs[0x0B] & 0x07) as u16) << 8);
        if timer < 2 {
            return 0.0;
        }
        let step = self.triangle_seq_step as usize & 31;
        TRIANGLE_SEQUENCE[step] as f64
    }

    // Return envelope/constant volume when enabled with nonzero length and
    // LFSR bit zero clear; otherwise return zero without advancing the LFSR.
    fn noise_sample(&mut self) -> f64 {
        if self.regs[0x15] & 0x08 == 0 || self.length_counter[3] == 0 {
            return 0.0;
        }
        let control = self.regs[0x0C];
        let volume = self.envelope_volume_units(2, control);
        if volume <= 0.0 {
            return 0.0;
        }
        if self.noise_lfsr & 1 == 0 {
            volume
        } else {
            0.0
        }
    }

    // Return the stored seven-bit DMC DAC level, including while sample playback
    // is disabled. The mixer DC blocker handles its steady component.
    fn dmc_sample(&self) -> f64 {
        self.dmc_output_level as f64
    }

    // Report either DMC or frame IRQ without acknowledging either latch.
    pub fn irq_pending(&self) -> bool {
        self.dmc_irq_flag || self.frame_irq_flag
    }

    // Request a byte only while DMC is enabled, bytes remain and the one-byte
    // buffer is empty. The bus performs the actual mapper read.
    pub fn pending_dmc_fetch_addr(&self) -> Option<u16> {
        if self.dmc_enabled && self.dmc_bytes_remaining > 0 && self.dmc_sample_buffer.is_none() {
            Some(self.dmc_current_addr)
        } else {
            None
        }
    }

    // Accept a pending byte, count the four-cycle DMA cost and advance its
    // address with $FFFF-to-$8000 wrapping. At the final fetched byte restart
    // a loop or set the enabled IRQ; this occurs before that byte finishes playing.
    pub fn complete_dmc_sample_fetch(
        &mut self,
        value: u8,
        frame: u64,
        cycle: u64,
        trace_cfg: TraceConfig,
        sink: &mut TraceSink,
    ) {
        // Ignore unsolicited completions with no remaining bytes or a full
        // buffer. Normal callers first consult pending_dmc_fetch_addr.
        if self.dmc_bytes_remaining == 0 || self.dmc_sample_buffer.is_some() {
            return;
        }
        let fetched_addr = self.dmc_current_addr;
        self.dmc_sample_buffer = Some(value);
        self.dmc_sample_fetches = self.dmc_sample_fetches.saturating_add(1);
        self.dmc_dma_stall_cycles = self.dmc_dma_stall_cycles.saturating_add(4);
        self.dmc_current_addr = if self.dmc_current_addr == 0xFFFF {
            0x8000
        } else {
            self.dmc_current_addr.wrapping_add(1)
        };
        self.dmc_bytes_remaining = self.dmc_bytes_remaining.saturating_sub(1);
        if self.dmc_bytes_remaining == 0 {
            if self.dmc_loop_flag {
                self.restart_dmc_sample();
            } else if self.regs[0x10] & 0x80 != 0 {
                self.dmc_irq_flag = true;
                self.dmc_irq_events = self.dmc_irq_events.saturating_add(1);
            }
        }
        if trace_cfg.apu || trace_cfg.dma {
            let mut event = TraceEvent::new("apu.dmc_fetch", frame, cycle);
            event.addr = Some(fetched_addr);
            event.value = Some(value);
            event.message = Some(format!(
                "DMC fetched sample byte; remaining={} loop={} irq_flag={} fetches={}",
                self.dmc_bytes_remaining,
                self.dmc_loop_flag,
                self.dmc_irq_flag,
                self.dmc_sample_fetches
            ));
            sink.push(event);
        }
    }

    // Reload the configured start address and nonzero byte count, preserving
    // the current output shifter, buffered byte and DAC level.
    fn restart_dmc_sample(&mut self) {
        self.dmc_current_addr = self.dmc_sample_addr;
        self.dmc_bytes_remaining = self.dmc_sample_length.max(1);
    }

    // Advance the selected NTSC bit timer while enabled. Refill an exhausted
    // shifter from the byte buffer or enter an eight-bit silent interval; consume
    // least-significant bits to adjust the bounded DAC by two. Fetches happen
    // externally, so a long chunk can outlast the single buffered byte.
    fn clock_dmc_output(&mut self, cpu_cycles: u64) {
        if !self.dmc_enabled {
            return;
        }
        let period = DMC_RATE_CPU_CYCLES_NTSC[(self.regs[0x10] & 0x0F) as usize] as f64;
        self.dmc_timer_accum += cpu_cycles as f64;
        while self.dmc_timer_accum >= period {
            self.dmc_timer_accum -= period;

            // The output unit reloads the shifter when its 8-bit shift register is
            // exhausted. The previous implementation waited through an initial
            // silent byte, which made short DMC clicks and percussion too late.
            if self.dmc_bits_remaining == 0 {
                if let Some(sample) = self.dmc_sample_buffer.take() {
                    self.dmc_shift_reg = sample;
                    self.dmc_bits_remaining = 8;
                    self.dmc_silence = false;
                } else {
                    self.dmc_bits_remaining = 8;
                    self.dmc_silence = true;
                }
            }

            if !self.dmc_silence {
                if self.dmc_shift_reg & 1 != 0 {
                    if self.dmc_output_level <= 125 {
                        self.dmc_output_level += 2;
                    }
                } else if self.dmc_output_level >= 2 {
                    self.dmc_output_level -= 2;
                }
            } else {
                self.dmc_silence_ticks = self.dmc_silence_ticks.saturating_add(1);
            }
            self.dmc_shift_reg >>= 1;
            self.dmc_bits_remaining = self.dmc_bits_remaining.saturating_sub(1);
            self.dmc_bits_output = self.dmc_bits_output.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // Program an original one-byte DMC sample and check that enable requests
    // the expected $C080 fetch address.
    fn dmc_registers_start_sample_request() {
        let mut apu = ApuState::default();
        let mut sink = TraceSink::default();
        apu.write_register(0x4012, 0x02, 0, 0, TraceConfig::none(), &mut sink);
        apu.write_register(0x4013, 0x00, 0, 0, TraceConfig::none(), &mut sink);
        apu.write_register(0x4015, 0x10, 0, 0, TraceConfig::none(), &mut sink);
        assert_eq!(apu.pending_dmc_fetch_addr(), Some(0xC080));
    }

    #[test]
    // Feed an all-one byte and check nondecreasing DAC output plus generated
    // samples and consumed bits. This test does not compare a waveform reference.
    fn dmc_fetch_changes_output_over_time() {
        let mut apu = ApuState::default();
        let mut sink = TraceSink::default();
        apu.write_register(0x4010, 0x0F, 0, 0, TraceConfig::none(), &mut sink);
        apu.write_register(0x4011, 64, 0, 0, TraceConfig::none(), &mut sink);
        apu.write_register(0x4012, 0x00, 0, 0, TraceConfig::none(), &mut sink);
        apu.write_register(0x4013, 0x00, 0, 0, TraceConfig::none(), &mut sink);
        apu.write_register(0x4015, 0x10, 0, 0, TraceConfig::none(), &mut sink);
        apu.complete_dmc_sample_fetch(0xFF, 0, 0, TraceConfig::none(), &mut sink);
        let before = apu.dmc_output_level;
        apu.clock_cpu_cycles(54 * 10, 0, 0, TraceConfig::none(), &mut sink);
        assert!(apu.dmc_output_level >= before);
        assert!(apu.generated_samples > 0);
        assert!(apu.dmc_bits_output > 0);
    }

    #[test]
    // Enable pulse one with timer eight and require its duty step to change
    // after one 18-cycle sequencer period.
    fn pulse_timer_advances_duty_sequence_by_cpu_cycles() {
        let mut apu = ApuState::default();
        let mut sink = TraceSink::default();
        apu.write_register(0x4015, 0x01, 0, 0, TraceConfig::none(), &mut sink);
        apu.write_register(0x4000, 0b0101_1111, 0, 0, TraceConfig::none(), &mut sink);
        apu.write_register(0x4002, 8, 0, 0, TraceConfig::none(), &mut sink);
        apu.write_register(0x4003, 0, 0, 0, TraceConfig::none(), &mut sink);
        let step0 = apu.pulse_seq_step[0];
        apu.clock_cpu_cycles(2 * (8 + 1), 0, 0, TraceConfig::none(), &mut sink);
        assert_ne!(apu.pulse_seq_step[0], step0);
    }

    #[test]
    // Provide a byte before the first bit tick and require one consumed bit
    // with no initial silent tick.
    fn dmc_first_fetched_byte_is_not_delayed_by_a_silent_byte() {
        let mut apu = ApuState::default();
        let mut sink = TraceSink::default();
        apu.write_register(0x4010, 0x0F, 0, 0, TraceConfig::none(), &mut sink);
        apu.write_register(0x4011, 64, 0, 0, TraceConfig::none(), &mut sink);
        apu.write_register(0x4012, 0x00, 0, 0, TraceConfig::none(), &mut sink);
        apu.write_register(0x4013, 0x00, 0, 0, TraceConfig::none(), &mut sink);
        apu.write_register(0x4015, 0x10, 0, 0, TraceConfig::none(), &mut sink);
        apu.complete_dmc_sample_fetch(0x01, 0, 0, TraceConfig::none(), &mut sink);
        apu.clock_cpu_cycles(54, 0, 0, TraceConfig::none(), &mut sink);
        assert_eq!(apu.dmc_silence_ticks, 0);
        assert_eq!(apu.dmc_bits_output, 1);
    }

    #[test]
    // Cross the four-step IRQ point, then check that the first status read
    // reports bit six and the next read observes it cleared.
    fn frame_irq_appears_in_status_and_clears_on_read() {
        let mut apu = ApuState::default();
        let mut sink = TraceSink::default();
        apu.write_register(0x4017, 0x00, 0, 0, TraceConfig::none(), &mut sink);
        apu.clock_cpu_cycles(29_830, 0, 0, TraceConfig::none(), &mut sink);
        assert!(apu.irq_pending());
        assert_eq!(apu.read_register(0x4015) & 0x40, 0x40);
        assert_eq!(apu.read_register(0x4015) & 0x40, 0x00);
    }

    #[test]
    // Compare full capture with default recent buffering over synthetic clock
    // chunks: full capture retains every generated sample, while default keeps
    // 44100. Channels stay unprogrammed, so this checks retention, not sound quality.
    fn offline_capture_keeps_more_than_one_second_without_changing_default() {
        for full in [false, true] {
            let mut apu = ApuState { capture_full_audio: full, ..ApuState::default() };
            let mut sink = TraceSink::default();
            for _ in 0..120 {
                apu.clock_cpu_cycles(29_830, 0, 0, TraceConfig::none(), &mut sink);
            }
            if full {
                assert_eq!(apu.sample_buffer.len() as u64, apu.generated_samples);
                assert!(apu.sample_buffer.len() > 80_000);
            } else {
                assert_eq!(apu.sample_buffer.len(), 44_100);
            }
        }
    }
}
