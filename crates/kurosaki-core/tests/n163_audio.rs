use kurosaki_core::{
    apu::ApuState, mapper::Mapper, namco163::Namco163Mapper, Cartridge, Emulator, Mirroring,
    TraceConfig, TraceSink,
};
#[path = "common/n163_fixture.rs"]
mod fixture;
fn mapper() -> Namco163Mapper {
    Namco163Mapper::new(vec![0; 65536], vec![], Mirroring::Horizontal, false)
}
fn write(m: &mut Namco163Mapper, a: u16, v: u8) {
    m.write_prg(a, v, 0, 0, TraceConfig::none(), &mut TraceSink::default());
}
fn read(m: &mut Namco163Mapper, a: u16) -> u8 {
    m.read_prg(a, 0, 0, TraceConfig::none(), &mut TraceSink::default())
}
fn ram(m: &mut Namco163Mapper, a: u8, v: u8) {
    write(m, 0xf800, a);
    write(m, 0x4800, v);
}
fn get(m: &mut Namco163Mapper, a: u8) -> u8 {
    write(m, 0xf800, a);
    read(m, 0x4800)
}
fn phase(m: &mut Namco163Mapper, ch: u8) -> u32 {
    let b = 0x40 + ch * 8;
    get(m, b + 1) as u32 | (get(m, b + 3) as u32) << 8 | (get(m, b + 5) as u32) << 16
}
#[test]
fn address_port_mirrors_increment_and_wrap_on_reads_and_writes() {
    let mut m = mapper();
    write(&mut m, 0xffff, 0xfe);
    for value in [0x12, 0x34, 0x56] {
        write(&mut m, 0x4fff, value);
    }
    assert_eq!(get(&mut m, 0), 0x56);
    write(&mut m, 0xf800, 0xfe);
    assert_eq!(read(&mut m, 0x4800), 0x12);
    assert_eq!(read(&mut m, 0x4fff), 0x34);
    assert_eq!(read(&mut m, 0x4801), 0x56);
    write(&mut m, 0xf800, 0x7e);
    write(&mut m, 0xe000, 3);
    assert_eq!(
        read(&mut m, 0x4800),
        0x12,
        "PRG select must not change RAM pointer"
    );
}
#[test]
fn prg_registers_and_ram_protection_use_hardware_addresses() {
    let mut m = mapper();
    write(&mut m, 0xe000, 0xc4);
    write(&mut m, 0xe800, 0x85);
    write(&mut m, 0xf000, 0x46);
    for (a, b) in [(0x8000, 4), (0xa000, 5), (0xc000, 6), (0xe000, 7)] {
        assert_eq!(m.physical_prg_bank_8k(a), Some(b));
    }
    write(&mut m, 0xc000, 0xe1);
    assert_eq!(m.physical_prg_bank_8k(0x8000), Some(4));
    write(&mut m, 0x6000, 9);
    assert_eq!(read(&mut m, 0x6000), 0);
    write(&mut m, 0xf800, 0x40);
    write(&mut m, 0x6000, 9);
    assert_eq!(read(&mut m, 0x6000), 9);
    write(&mut m, 0xf800, 0x41);
    write(&mut m, 0x6000, 3);
    assert_eq!(read(&mut m, 0x6000), 9);
}
#[test]
fn fifteen_cpu_cycle_sequencer_updates_all_channel_counts() {
    for channels in 1..=8 {
        let mut m = mapper();
        for ch in 0..8 {
            ram(&mut m, 0x40 + ch * 8, 1);
        }
        ram(&mut m, 0x7f, (channels - 1) << 4);
        m.clock_expansion_audio(14);
        assert_eq!(phase(&mut m, 7), 0);
        m.clock_expansion_audio(1);
        assert_eq!(phase(&mut m, 7), 1);
        m.clock_expansion_audio(15 * channels as u64 - 15);
        for ch in 0..8 {
            assert_eq!(phase(&mut m, ch), u32::from(ch >= 8 - channels));
        }
    }
}

#[test]
fn all_eight_voices_keep_independent_full_eighteen_bit_frequencies() {
    let mut m = mapper();
    let frequencies = [
        1u32, 0x100, 0x10000, 0x20001, 0x30000, 0x3ffff, 0x12345, 0x23456,
    ];
    for (ch, frequency) in frequencies.iter().enumerate() {
        let base = 0x40 + ch as u8 * 8;
        ram(&mut m, base, *frequency as u8);
        ram(&mut m, base + 2, (frequency >> 8) as u8);
        ram(&mut m, base + 4, (frequency >> 16) as u8); // 256-sample length
    }
    ram(&mut m, 0x7f, 0x7f);
    m.clock_expansion_audio(480); // four complete multiplexing rounds
    for (ch, frequency) in frequencies.iter().enumerate() {
        assert_eq!(phase(&mut m, ch as u8), frequency * 4);
    }
}
#[test]
fn shared_phase_wave_length_nibble_order_offset_and_volume() {
    let mut m = mapper();
    ram(&mut m, 0, 0xf1);
    ram(&mut m, 0x7c, 0xfc);
    ram(&mut m, 0x7f, 15);
    m.clock_expansion_audio(15);
    m.expansion_audio_sample(44100);
    m.clock_expansion_audio(15);
    assert_eq!(m.expansion_audio_sample(44100), -7 * 15 * 180);
    ram(&mut m, 0x7e, 1);
    m.clock_expansion_audio(15);
    m.expansion_audio_sample(44100);
    m.clock_expansion_audio(15);
    assert_eq!(m.expansion_audio_sample(44100), 7 * 15 * 180);
    ram(&mut m, 0x7f, 3);
    m.clock_expansion_audio(15);
    m.expansion_audio_sample(44100);
    m.clock_expansion_audio(15);
    assert_eq!(m.expansion_audio_sample(44100), 7 * 3 * 180);
    // phase=$03ffff + freq=$000002 wraps at four samples, with RAM readback.
    ram(&mut m, 0x79, 255);
    ram(&mut m, 0x7b, 255);
    ram(&mut m, 0x7d, 3);
    ram(&mut m, 0x78, 2);
    m.clock_expansion_audio(15);
    assert_eq!(phase(&mut m, 7), 1);
    ram(&mut m, 0x7e, 255);
    ram(&mut m, 0x78, 0);
    ram(&mut m, 0x7d, 1);
    m.clock_expansion_audio(15);
    m.expansion_audio_sample(44100);
    m.clock_expansion_audio(15);
    assert_eq!(
        m.expansion_audio_sample(44100),
        -7 * 3 * 180,
        "sample address wraps at 256 nibbles"
    );
}
#[test]
fn disable_freezes_phase_and_volume_zero_is_silent() {
    let mut m = mapper();
    ram(&mut m, 0x78, 1);
    ram(&mut m, 0x7f, 15);
    m.clock_expansion_audio(30);
    m.expansion_audio_sample(44100);
    write(&mut m, 0xe000, 0x40);
    let before = phase(&mut m, 7);
    m.clock_expansion_audio(1500);
    assert_eq!(phase(&mut m, 7), before);
    assert_eq!(m.expansion_audio_sample(44100), 0);
    write(&mut m, 0xe000, 0);
    ram(&mut m, 0x7f, 0);
    m.clock_expansion_audio(30);
    m.expansion_audio_sample(44100);
    m.clock_expansion_audio(150);
    assert_eq!(m.expansion_audio_sample(44100), 0);
    for sub in [1, 2] {
        let mut m = Namco163Mapper::new_with_submapper(
            vec![0; 32768],
            vec![],
            Mirroring::Horizontal,
            false,
            sub,
        );
        ram(&mut m, 0x7f, 15);
        m.clock_expansion_audio(150);
        assert_eq!(m.expansion_audio_sample(44100), 0);
    }
}
#[test]
fn irq_saturates_at_7fff_and_both_writes_acknowledge() {
    let mut m = mapper();
    write(&mut m, 0x5000, 0xfe);
    write(&mut m, 0x5800, 0xff);
    m.clock_cpu(1, 0, 0, TraceConfig::none(), &mut TraceSink::default());
    assert!(m.irq_pending());
    m.clock_cpu(
        1_000_000,
        0,
        0,
        TraceConfig::none(),
        &mut TraceSink::default(),
    );
    assert_eq!(read(&mut m, 0x5001), 0xff);
    write(&mut m, 0x5002, 0);
    assert!(!m.irq_pending());
    m.clock_cpu(255, 0, 0, TraceConfig::none(), &mut TraceSink::default());
    assert!(m.irq_pending());
    write(&mut m, 0x5801, 0);
    assert!(!m.irq_pending());
}
fn timed_render(chunks: &[u64], rate: u32) -> (Vec<i16>, Vec<u8>) {
    let mut m = mapper();
    ram(&mut m, 0, 0xf1);
    ram(&mut m, 0x78, 137);
    ram(&mut m, 0x7a, 27);
    ram(&mut m, 0x7c, 0xfc);
    ram(&mut m, 0x7f, 15);
    let mut apu = ApuState::default();
    apu.sample_rate = rate;
    for n in chunks {
        apu.clock_cpu_cycles_with_timed_expansion(
            *n,
            0,
            0,
            TraceConfig::none(),
            &mut TraceSink::default(),
            |c, r| {
                m.clock_expansion_audio(c);
                r.map_or(0, |r| m.expansion_audio_sample(r))
            },
        );
    }
    (apu.sample_buffer, m.snapshot_bytes())
}
#[test]
fn pcm_does_not_depend_on_cpu_batch_size_and_phase_is_rate_independent() {
    let (bulk, state) = timed_render(&[60000], 44100);
    let (single, single_state) = timed_render(&vec![1; 60000], 44100);
    assert_eq!(bulk, single);
    assert_eq!(state, single_state);
    assert!(bulk.iter().any(|v| *v > 0) && bulk.iter().any(|v| *v < 0));
    let (_, other) = timed_render(&[60000], 48000);
    assert_eq!(&state[58..186], &other[58..186]);
}
#[test]
fn mapper_snapshot_restores_phase_and_partial_sample_and_rejects_bad_state() {
    let mut m = mapper();
    ram(&mut m, 0x78, 13);
    ram(&mut m, 0x7f, 15);
    m.clock_expansion_audio(47);
    let state = m.snapshot_bytes();
    let mut restored = mapper();
    restored.restore_snapshot_bytes(&state).unwrap();
    for n in [3, 40, 21, 15, 200] {
        m.clock_expansion_audio(n);
        restored.clock_expansion_audio(n);
        assert_eq!(
            m.expansion_audio_sample(44100),
            restored.expansion_audio_sample(44100)
        );
    }
    assert_eq!(m.snapshot_bytes(), restored.snapshot_bytes());
    let before = restored.snapshot_bytes();
    let mut bad = state;
    bad[36] = 0;
    assert!(restored.restore_snapshot_bytes(&bad).is_err());
    assert_eq!(restored.snapshot_bytes(), before);
}
#[test]
fn rom_execution_and_serialized_snapshot_resume_produce_identical_audio() {
    let cart = Cartridge::from_bytes(&fixture::nes(8)).unwrap();
    let mut emu = Emulator::from_cartridge(cart.clone()).unwrap();
    emu.reset(TraceConfig::none());
    for _ in 0..3 {
        emu.step_frame(TraceConfig::none(), false).unwrap();
    }
    assert!(emu.bus.apu.sample_buffer.iter().any(|v| *v != 0));
    let snap = serde_json::from_str(&serde_json::to_string(&emu.snapshot()).unwrap()).unwrap();
    let mut resume = Emulator::from_snapshot(cart, &snap).unwrap();
    for _ in 0..2 {
        emu.step_frame(TraceConfig::none(), false).unwrap();
        resume.step_frame(TraceConfig::none(), false).unwrap();
    }
    assert_eq!(emu.bus.apu.sample_buffer, resume.bus.apu.sample_buffer);
    assert_eq!(
        emu.bus.mapper.snapshot_bytes(),
        resume.bus.mapper.snapshot_bytes()
    );
}
