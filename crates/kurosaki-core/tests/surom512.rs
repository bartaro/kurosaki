use kurosaki_core::cart::{Cartridge, Mirroring};
use kurosaki_core::diagnostic_events::diagnostic_events_from_trace_and_report;
use kurosaki_core::diagnostics::DiagnosticReport;
use kurosaki_core::mapper::{Mapper, Mmc1BoardVariant, Mmc1Config, Mmc1Mapper, NametableMirroring};
use kurosaki_core::trace::{TraceConfig, TraceSink};
use kurosaki_core::{audit_board, Emulator, Snapshot};
use serde_json::json;

const BANK_SIZE: usize = 16 * 1024;

// Build an original 512 KiB mapper-1 battery fixture with CHR RAM, bank-ID
// fill bytes and identical banks 15/31. These bytes exercise layout/state
// checks; they are not a complete executable game.
fn synthetic_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 16 + 32 * BANK_SIZE];
    rom[0..4].copy_from_slice(b"NES\x1A");
    rom[4] = 32;
    rom[5] = 0;
    rom[6] = 0x12;
    rom[8] = 1;
    for bank in 0..32 {
        rom[16 + bank * BANK_SIZE..16 + (bank + 1) * BANK_SIZE].fill(bank as u8);
    }
    let common = rom[16 + 15 * BANK_SIZE..16 + 16 * BANK_SIZE].to_vec();
    rom[16 + 31 * BANK_SIZE..16 + 32 * BANK_SIZE].copy_from_slice(&common);
    rom
}

// Construct an explicit SUROM mapper with uniquely filled physical banks
// and 8 KiB PRG/CHR RAM, independent of header inference.
fn surom_mapper() -> Mmc1Mapper {
    let prg_rom = (0..32)
        .flat_map(|bank| std::iter::repeat_n(bank as u8, BANK_SIZE))
        .collect();
    Mmc1Mapper::new_with_config(
        prg_rom,
        Vec::new(),
        Mirroring::Horizontal,
        Mmc1Config {
            board_variant: Mmc1BoardVariant::Surom512,
            prg_ram_size: 8 * 1024,
            chr_ram_size: 8 * 1024,
            submapper: 0,
            battery: true,
        },
    )
}

// Write five least-significant-first bits two CPU cycles apart with mapper
// tracing enabled, avoiding the consecutive-cycle suppression rule.
fn serial_write(
    mapper: &mut dyn Mapper,
    addr: u16,
    data: u8,
    first_cycle: u64,
    sink: &mut TraceSink,
) {
    let cfg = TraceConfig {
        mapper: true,
        ..TraceConfig::default()
    };
    for bit in 0..5 {
        mapper.write_prg(addr, (data >> bit) & 1, 0, first_cycle + bit * 2, cfg, sink);
    }
}

#[test]
// Check all four CPU PRG slots before and after selecting the outer bank,
// then require no physical ROM-bank label for the PRG-RAM window.
fn surom_reports_the_physical_8k_bank_for_each_cpu_prg_slot() {
    let mut mapper = surom_mapper();
    let mut sink = TraceSink::default();

    assert_eq!(mapper.physical_prg_bank_8k(0x8000), Some(0));
    assert_eq!(mapper.physical_prg_bank_8k(0xA000), Some(1));
    assert_eq!(mapper.physical_prg_bank_8k(0xC000), Some(30));
    assert_eq!(mapper.physical_prg_bank_8k(0xE000), Some(31));

    serial_write(&mut mapper, 0xA000, 0x10, 10, &mut sink);
    assert_eq!(mapper.physical_prg_bank_8k(0x8000), Some(32));
    assert_eq!(mapper.physical_prg_bank_8k(0xA000), Some(33));
    assert_eq!(mapper.physical_prg_bank_8k(0xC000), Some(62));
    assert_eq!(mapper.physical_prg_bank_8k(0xE000), Some(63));
    assert_eq!(mapper.physical_prg_bank_8k(0x6000), None);
}

#[test]
// Disable CPU PRG-RAM access after importing battery bytes, then verify
// physical sidecar export/import still works without changing mapper state.
fn battery_surom_export_retains_physical_ram_while_cpu_access_is_disabled() {
    let cart = Cartridge::from_bytes(&synthetic_rom()).unwrap();
    let mut emulator = Emulator::from_cartridge(cart).unwrap();
    let data: Vec<_> = (0..8192).map(|i| (i * 13) as u8).collect();
    emulator.import_battery_ram(&data).unwrap();
    serial_write(
        &mut *emulator.bus.mapper,
        0xe000,
        0x10,
        2,
        &mut TraceSink::default(),
    );
    assert_eq!(
        emulator.bus.mapper.debug_state().prg_ram_enabled,
        Some(false)
    );
    let before = emulator.snapshot();
    assert_eq!(emulator.battery_ram().unwrap(), Some(data.clone()));
    emulator.import_battery_ram(&data).unwrap();
    assert_eq!(emulator.snapshot().mapper_private, before.mapper_private);
}

#[test]
// Serially select the four MMC1 mirroring modes and check the returned
// policy; this does not render nametable pixels.
fn mmc1_control_drives_runtime_nametable_mirroring() {
    let mut mapper = surom_mapper();
    let mut sink = TraceSink::default();

    assert_eq!(
        mapper.nametable_mirroring(),
        NametableMirroring::SingleScreenLow
    );
    serial_write(&mut mapper, 0x8000, 0x0D, 10, &mut sink);
    assert_eq!(
        mapper.nametable_mirroring(),
        NametableMirroring::SingleScreenHigh
    );
    serial_write(&mut mapper, 0x8000, 0x0E, 30, &mut sink);
    assert_eq!(mapper.nametable_mirroring(), NametableMirroring::Vertical);
    serial_write(&mut mapper, 0x8000, 0x0F, 50, &mut sink);
    assert_eq!(mapper.nametable_mirroring(), NametableMirroring::Horizontal);
}

#[test]
// Check size-based board inference and a structurally matching metadata
// map, common replicas, vector bytes and nonempty odd-bank tails. Filled
// reset tails satisfy the audit presence test without executing reset code.
fn infers_and_audits_surom512_layout() {
    let cart = Cartridge::from_bytes(&synthetic_rom()).expect("synthetic ROM must parse");
    assert_eq!(cart.info.board_profile.as_deref(), Some("surom512"));
    assert_eq!(
        cart.info.board_profile_source.as_deref(),
        Some("size_inference")
    );
    assert_eq!(cart.info.prg_rom_size, 512 * 1024);
    assert_eq!(cart.info.chr_rom_size, 0);
    assert_eq!(cart.info.chr_ram_size, Some(8 * 1024));
    assert_eq!(cart.info.prg_ram_size, Some(8 * 1024));

    let mappings: Vec<_> = (1..=30)
        .map(|logical| {
            json!({
                "logical_bank": logical,
                "physical_bank": if logical <= 15 { logical - 1 } else { logical },
                "is_common_replica": false
            })
        })
        .collect();
    let metadata = json!({
        "bank_layout": {
            "common_physical_banks": [15, 31],
            "mappings": mappings
        }
    });
    let report = audit_board(&cart, "surom512", Some(&metadata));
    assert!(report.pass, "{:#?}", report.diagnostics);
    assert!(report.common_replica_equal);
    assert!(report.vector_replica_equal);
    assert!(report.reset_tail_coverage_odd_banks);
    assert_eq!(report.metadata_mapping_equal, Some(true));
    assert!(report.input_hash_redacted);
}

#[test]
// Select the outer half and verify all four PRG modes with inner bank five,
// then read the resulting lower/upper window fill bytes.
fn implements_outer_inner_and_per_half_fixed_windows() {
    let mut mapper = surom_mapper();
    let mut sink = TraceSink::default();

    serial_write(&mut mapper, 0xA000, 0x10, 100, &mut sink);
    for (mode, expected) in [
        (0, vec![20, 21]),
        (1, vec![20, 21]),
        (2, vec![16, 21]),
        (3, vec![21, 31]),
    ] {
        serial_write(
            &mut mapper,
            0x8000,
            mode << 2,
            200 + mode as u64 * 100,
            &mut sink,
        );
        serial_write(&mut mapper, 0xE000, 5, 250 + mode as u64 * 100, &mut sink);
        let state = mapper.debug_state();
        assert_eq!(state.outer_prg_bank, Some(1));
        assert_eq!(state.inner_prg_bank, Some(5));
        assert_eq!(state.prg_bank_window, expected);
    }
    assert_eq!(
        mapper.read_prg(0x8000, 0, 1000, TraceConfig::none(), &mut sink),
        21
    );
    assert_eq!(
        mapper.read_prg(0xC000, 0, 1000, TraceConfig::none(), &mut sink),
        31
    );
}

#[test]
// Check disabled RAM reads, suppression of a serial write on the next CPU
// cycle and its diagnostic mapping, then verify a reset-bit write bypasses
// that suppression. All timing values are supplied directly to the mapper.
fn implements_prg_ram_disable_and_consecutive_cycle_ignore() {
    let mut mapper = surom_mapper();
    let mut sink = TraceSink::default();
    let cfg = TraceConfig {
        mapper: true,
        ..TraceConfig::default()
    };

    serial_write(&mut mapper, 0xE000, 0x10, 100, &mut sink);
    assert_eq!(mapper.debug_state().prg_ram_enabled, Some(false));
    mapper.write_prg(0x6000, 0x5A, 0, 200, cfg, &mut sink);
    assert_eq!(mapper.read_prg(0x6000, 0, 201, cfg, &mut sink), 0xFF);

    mapper.write_prg(0xA000, 0, 0, 300, cfg, &mut sink);
    mapper.write_prg(0xA000, 1, 0, 301, cfg, &mut sink);
    assert!(sink
        .events
        .iter()
        .any(|event| event.kind == "mapper.mmc1_write_ignored_consecutive"));
    let diagnostics =
        diagnostic_events_from_trace_and_report(&sink, &DiagnosticReport::new("synthetic"));
    assert!(diagnostics.iter().any(|event| {
        event.event_type == "FC_MMC1_CONSECUTIVE_WRITE_IGNORED" && event.severity == "warn"
    }));

    let ignored_before_reset = sink
        .events
        .iter()
        .filter(|event| event.kind == "mapper.mmc1_write_ignored_consecutive")
        .count();
    mapper.write_prg(0xA000, 0, 0, 400, cfg, &mut sink);
    mapper.write_prg(0xA000, 0x80, 0, 401, cfg, &mut sink);
    assert_eq!(
        sink.events
            .iter()
            .filter(|event| event.kind == "mapper.mmc1_write_ignored_consecutive")
            .count(),
        ignored_before_reset
    );
    assert!(sink
        .events
        .iter()
        .any(|event| event.kind == "mapper.mmc1_shift_reset" && event.cpu_cycle == 401));
}

#[test]
// Select 4 KiB CHR mode with different CHR-bank high bits and require an
// unsafe-mode event while the modeled outer PRG bank stays derived from CHR0.
fn reports_unsafe_chr_mode_without_using_chr1_for_outer_bank() {
    let mut mapper = surom_mapper();
    let mut sink = TraceSink::default();
    serial_write(&mut mapper, 0x8000, 0x10, 100, &mut sink);
    serial_write(&mut mapper, 0xA000, 0x00, 200, &mut sink);
    serial_write(&mut mapper, 0xC000, 0x10, 300, &mut sink);
    mapper.notify_ppu_addr(
        0x1000,
        0,
        400,
        TraceConfig {
            mapper: true,
            ..TraceConfig::default()
        },
        &mut sink,
    );
    assert_eq!(mapper.debug_state().outer_prg_bank, Some(0));
    assert!(sink
        .events
        .iter()
        .any(|event| event.kind == "mapper.mmc1_chr_mode_unsafe"));
}

#[test]
// Construct generic 256 KiB MMC1 and verify bank three with the final bank
// fixed, retaining the generic profile rather than SUROM behavior.
fn generic_mmc1_up_to_256k_remains_last_bank_fixed() {
    let prg_rom = (0..16)
        .flat_map(|bank| std::iter::repeat_n(bank as u8, BANK_SIZE))
        .collect();
    let mut mapper = Mmc1Mapper::new(prg_rom, Vec::new(), Mirroring::Horizontal, false);
    let mut sink = TraceSink::default();
    serial_write(&mut mapper, 0xE000, 3, 100, &mut sink);
    let state = mapper.debug_state();
    assert_eq!(state.board_profile.as_deref(), Some("generic_sxrom"));
    assert_eq!(state.prg_bank_window, vec![3, 15]);
}

#[test]
// Seed selected CPU fields, counters, CPU/PRG/CHR RAM and outer bank, then
// strictly restore and compare those values, the observation hash and the
// complete mapper-private payload. No post-restore instructions are executed.
fn resumable_snapshot_restores_cpu_bus_and_mmc1_private_state() {
    let rom = synthetic_rom();
    let cart = Cartridge::from_bytes(&rom).expect("synthetic ROM must parse");
    let mut emulator = Emulator::from_cartridge(cart).expect("emulator must initialize");
    emulator.frame = 42;
    emulator.instructions = 1234;
    emulator.cpu.pc = 0xC123;
    emulator.cpu.a = 0x5A;
    emulator.bus.ram[0x123] = 0xA5;
    emulator.bus.mapper.write_prg(
        0x6000,
        0x3C,
        emulator.frame,
        emulator.cpu.cycles,
        TraceConfig::none(),
        &mut emulator.trace,
    );
    emulator.bus.mapper.write_chr(0x0123, 0x7E);
    let mut sink = TraceSink::default();
    serial_write(emulator.bus.mapper.as_mut(), 0xA000, 0x10, 100, &mut sink);

    let snapshot = emulator.snapshot();
    assert_eq!(snapshot.format, "kurosaki-snapshot-v2");
    let expected_hash = emulator.state_hash();
    let expected_mapper_private = snapshot.mapper_private.clone();

    let restored_cart = Cartridge::from_bytes(&rom).expect("synthetic ROM must parse twice");
    let mut restored =
        Emulator::from_snapshot(restored_cart, &snapshot).expect("snapshot must restore");
    assert_eq!(restored.frame, 42);
    assert_eq!(restored.instructions, 1234);
    assert_eq!(restored.cpu.pc, 0xC123);
    assert_eq!(restored.cpu.a, 0x5A);
    assert_eq!(restored.bus.ram[0x123], 0xA5);
    assert_eq!(
        restored.bus.mapper.read_prg(
            0x6000,
            restored.frame,
            restored.cpu.cycles,
            TraceConfig::none(),
            &mut restored.trace,
        ),
        0x3C
    );
    assert_eq!(restored.bus.mapper.read_chr(0x0123), 0x7E);
    assert_eq!(restored.state_hash(), expected_hash);
    assert_eq!(restored.snapshot().mapper_private, expected_mapper_private);
}

#[test]
// Restore seeded PRG RAM, reset the CPU, then check the byte persists and
// a reset event was recorded.
fn reset_after_snapshot_restore_preserves_mmc1_prg_ram() {
    let rom = synthetic_rom();
    let cart = Cartridge::from_bytes(&rom).expect("synthetic ROM must parse");
    let mut emulator = Emulator::from_cartridge(cart).expect("emulator must initialize");
    emulator.bus.mapper.write_prg(
        0x6000,
        0x3C,
        emulator.frame,
        emulator.cpu.cycles,
        TraceConfig::none(),
        &mut emulator.trace,
    );
    let snapshot = emulator.snapshot();

    let restored_cart = Cartridge::from_bytes(&rom).expect("synthetic ROM must parse twice");
    let mut restored =
        Emulator::from_snapshot(restored_cart, &snapshot).expect("snapshot must restore");
    restored.reset(TraceConfig::none());

    assert_eq!(
        restored.bus.mapper.read_prg(
            0x6000,
            restored.frame,
            restored.cpu.cycles,
            TraceConfig::none(),
            &mut restored.trace,
        ),
        0x3C
    );
    assert!(restored
        .trace
        .events
        .iter()
        .any(|event| event.kind == "emu.reset"));
}

#[test]
// Remove mapper-private bytes and the instruction field from serialized
// state and require deserialization failure. This tests required fields,
// not execution of an earlier snapshot format.
fn legacy_snapshot_without_private_state_is_rejected() {
    let rom = synthetic_rom();
    let cart = Cartridge::from_bytes(&rom).expect("synthetic ROM must parse");
    let emulator = Emulator::from_cartridge(cart).expect("emulator must initialize");
    let mut json = serde_json::to_value(emulator.snapshot()).expect("snapshot must serialize");
    let object = json.as_object_mut().expect("snapshot must be an object");
    object.remove("mapper_private");
    object.remove("instructions");
    assert!(serde_json::from_value::<Snapshot>(json).is_err());
}
