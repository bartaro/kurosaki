use kurosaki_core::{Cartridge, Emulator, RunOptions};

#[test]
fn nrom_exposes_header_nametable_mirroring_to_the_ppu() {
    use kurosaki_core::cart::Mirroring;
    use kurosaki_core::mapper::{Mapper, NametableMirroring, NromMapper};

    let vertical = NromMapper::new(vec![0; 16 * 1024], vec![0; 8 * 1024], Mirroring::Vertical);
    let horizontal = NromMapper::new(vec![0; 16 * 1024], vec![0; 8 * 1024], Mirroring::Horizontal);

    assert_eq!(vertical.nametable_mirroring(), NametableMirroring::Vertical);
    assert_eq!(
        horizontal.nametable_mirroring(),
        NametableMirroring::Horizontal
    );
    assert_eq!(vertical.physical_prg_bank_8k(0x8000), Some(0));
    assert_eq!(vertical.physical_prg_bank_8k(0xA000), Some(1));
    assert_eq!(vertical.physical_prg_bank_8k(0xC000), Some(0));
    assert_eq!(vertical.physical_prg_bank_8k(0xE000), Some(1));
    assert_eq!(vertical.physical_prg_bank_8k(0x7FFF), None);
}

#[test]
fn synthetic_nrom_dma_smoke_runs_without_unimplemented_official_opcode_failures() {
    // Original synthetic 6502 program: SEI; LDA #2; STA $4014; JMP $8006.
    // Generated in memory so this test needs no externally supplied ROM file.
    let rom = synthetic_nrom_with_program(&[0x78, 0xA9, 0x02, 0x8D, 0x14, 0x40, 0x4C, 0x06, 0x80]);
    let cart = Cartridge::from_bytes(&rom).expect("synthetic fixture must be a valid iNES ROM");
    assert_eq!(cart.info.mapper, 0);
    let mut emu = Emulator::from_cartridge(cart).expect("NROM mapper must be executable");
    let summary = emu.run(RunOptions {
        frames: 2,
        allow_unimplemented_opcode: false,
        ..RunOptions::default()
    });
    assert!(
        !summary.stopped,
        "unexpected stop: {:?}",
        summary.stop_reason
    );
    assert!(
        summary.instructions > 100,
        "smoke ROM should execute a visible loop"
    );
    assert!(
        summary
            .diagnostics
            .items
            .iter()
            .any(|d| d.code == "KS-DMA-0001"),
        "OAM DMA diagnostic should be observed"
    );
}

#[test]
fn official_2a03_opcodes_are_named_as_supported() {
    let official: &[u8] = &[
        0x00, 0x01, 0x05, 0x06, 0x08, 0x09, 0x0A, 0x0D, 0x0E, 0x10, 0x11, 0x15, 0x16, 0x18, 0x19,
        0x1D, 0x1E, 0x20, 0x21, 0x24, 0x25, 0x26, 0x28, 0x29, 0x2A, 0x2C, 0x2D, 0x2E, 0x30, 0x31,
        0x35, 0x36, 0x38, 0x39, 0x3D, 0x3E, 0x40, 0x41, 0x45, 0x46, 0x48, 0x49, 0x4A, 0x4C, 0x4D,
        0x4E, 0x50, 0x51, 0x55, 0x56, 0x58, 0x59, 0x5D, 0x5E, 0x60, 0x61, 0x65, 0x66, 0x68, 0x69,
        0x6A, 0x6C, 0x6D, 0x6E, 0x70, 0x71, 0x75, 0x76, 0x78, 0x79, 0x7D, 0x7E, 0x81, 0x84, 0x85,
        0x86, 0x88, 0x8A, 0x8C, 0x8D, 0x8E, 0x90, 0x91, 0x94, 0x95, 0x96, 0x98, 0x99, 0x9A, 0x9D,
        0xA0, 0xA1, 0xA2, 0xA4, 0xA5, 0xA6, 0xA8, 0xA9, 0xAA, 0xAC, 0xAD, 0xAE, 0xB0, 0xB1, 0xB4,
        0xB5, 0xB6, 0xB8, 0xB9, 0xBA, 0xBC, 0xBD, 0xBE, 0xC0, 0xC1, 0xC4, 0xC5, 0xC6, 0xC8, 0xC9,
        0xCA, 0xCC, 0xCD, 0xCE, 0xD0, 0xD1, 0xD5, 0xD6, 0xD8, 0xD9, 0xDD, 0xDE, 0xE0, 0xE1, 0xE4,
        0xE5, 0xE6, 0xE8, 0xE9, 0xEA, 0xEC, 0xED, 0xEE, 0xF0, 0xF1, 0xF5, 0xF6, 0xF8, 0xF9, 0xFD,
        0xFE,
    ];
    for &op in official {
        assert!(
            !kurosaki_core::cpu::opcode_mnemonic(op).contains("ILL"),
            "opcode {op:02X} should be official/supported"
        );
    }
}

fn synthetic_nrom_with_program(program: &[u8]) -> Vec<u8> {
    let mut prg = vec![0xEA; 16 * 1024];
    prg[..program.len()].copy_from_slice(program);
    let end = prg.len();
    for offset in [end - 6, end - 4, end - 2] {
        prg[offset] = 0x00;
        prg[offset + 1] = 0x80;
    }
    let mut rom = vec![0x4E, 0x45, 0x53, 0x1A, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    rom.extend_from_slice(&prg);
    rom
}

#[test]
fn cpu_and_memory_write_trace_share_the_executing_pc_and_physical_bank() {
    let rom = synthetic_nrom_with_program(&[
        0xA9, 0x5A, // LDA #$5A at $8000
        0x8D, 0x02, 0x00, // STA $0002 at $8002
        0x4C, 0x05, 0x80, // JMP $8005
    ]);
    let cart = Cartridge::from_bytes(&rom).expect("synthetic trace ROM must parse");
    let mut emu = Emulator::from_cartridge(cart).expect("NROM mapper must instantiate");
    let trace = kurosaki_core::TraceConfig {
        cpu: true,
        mem_read: true,
        mem_write: true,
        ..kurosaki_core::TraceConfig::default()
    };

    emu.reset(trace);
    emu.step_instruction(trace, false)
        .expect("LDA should execute");
    emu.step_instruction(trace, false)
        .expect("STA should execute");

    let instruction = emu
        .trace
        .events
        .iter()
        .find(|event| event.kind == "cpu.instruction" && event.pc == Some(0x8002))
        .expect("STA instruction trace should be present");
    assert_eq!(instruction.prg_bank, Some(0));

    let write = emu
        .trace
        .events
        .iter()
        .find(|event| event.kind == "mem.write" && event.addr == Some(0x0002))
        .expect("STA memory-write trace should be present");
    assert_eq!(write.pc, Some(0x8002));
    assert_eq!(write.prg_bank, Some(0));
    assert_eq!(write.opcode, Some(0x8d));
}

#[test]
fn ines_trainer_is_preloaded_into_cpu_7000_window() {
    let mut rom = synthetic_nrom_with_program(&[
        0xAD, 0x00, 0x70, // LDA $7000
        0x8D, 0x00, 0x00, // STA $0000
        0x4C, 0x06, 0x80, // JMP $8006
    ]);
    rom[6] |= 0x04;
    let trainer: Vec<u8> = (0..512)
        .map(|index| (index as u8).wrapping_add(0x5A))
        .collect();
    rom.splice(16..16, trainer.iter().copied());

    let cart = Cartridge::from_bytes(&rom).expect("synthetic trainer ROM must parse");
    assert_eq!(cart.trainer.as_deref(), Some(trainer.as_slice()));
    let mut emu = Emulator::from_cartridge(cart).expect("NROM mapper must instantiate");
    assert_eq!(
        emu.bus.read(
            0x7000,
            0,
            0,
            kurosaki_core::TraceConfig::none(),
            &mut emu.trace,
        ),
        0x5A
    );
    assert_eq!(
        emu.bus.read(
            0x71FF,
            0,
            0,
            kurosaki_core::TraceConfig::none(),
            &mut emu.trace,
        ),
        0x59
    );

    emu.reset(kurosaki_core::TraceConfig::none());
    emu.step_instruction(kurosaki_core::TraceConfig::none(), false)
        .expect("trainer-backed load must execute");
    emu.step_instruction(kurosaki_core::TraceConfig::none(), false)
        .expect("trainer-backed store must execute");
    assert_eq!(emu.bus.ram[0], 0x5A);
}

#[test]
fn nrom_snapshot_restores_trainer_backed_prg_ram() {
    let mut rom = synthetic_nrom_with_program(&[0x4C, 0x00, 0x80]);
    rom[6] |= 0x04;
    let trainer = vec![0xA5; 512];
    rom.splice(16..16, trainer);
    let cart = Cartridge::from_bytes(&rom).expect("synthetic trainer ROM must parse");
    let mut emu = Emulator::from_cartridge(cart.clone()).expect("NROM mapper must instantiate");
    emu.reset(kurosaki_core::TraceConfig::none());
    let snapshot = emu.snapshot();

    emu.bus.mapper.write_prg(
        0x7000,
        0x00,
        0,
        0,
        kurosaki_core::TraceConfig::none(),
        &mut emu.trace,
    );
    assert_eq!(
        emu.bus.mapper.read_prg(
            0x7000,
            0,
            0,
            kurosaki_core::TraceConfig::none(),
            &mut emu.trace,
        ),
        0x00
    );

    let mut restored = Emulator::from_snapshot(cart, &snapshot).expect("NROM snapshot must resume");
    assert_eq!(
        restored.bus.mapper.read_prg(
            0x7000,
            0,
            0,
            kurosaki_core::TraceConfig::none(),
            &mut restored.trace,
        ),
        0xA5
    );
}

#[test]
fn unofficial_ahx_abs_y_stores_masked_value_without_page_crossing() {
    let rom = synthetic_nrom_with_program(&[
        0xA9, 0xF3, // LDA #$F3
        0xA2, 0xAF, // LDX #$AF
        0xA0, 0x02, // LDY #$02
        0x9F, 0x00, 0x02, // AHX $0200,Y -> $0202 = $03
        0x4C, 0x09, 0x80, // JMP $8009
    ]);
    let cart = Cartridge::from_bytes(&rom).expect("synthetic AHX ROM must parse");
    let mut emu = Emulator::from_cartridge(cart).expect("NROM mapper must instantiate");
    emu.reset(kurosaki_core::TraceConfig::none());
    for _ in 0..4 {
        emu.step_instruction(kurosaki_core::TraceConfig::none(), false)
            .expect("AHX sequence must execute");
    }
    assert_eq!(emu.bus.ram[0x0202], 0x03);
    assert_eq!(emu.cpu.pc, 0x8009);
    assert_eq!(kurosaki_core::cpu::opcode_mnemonic(0x9F), "*AHX a,Y");
}

#[test]
fn unofficial_ahx_abs_y_uses_masked_high_byte_on_page_crossing() {
    let rom = synthetic_nrom_with_program(&[
        0xA9, 0xF0, // LDA #$F0
        0xA2, 0xF0, // LDX #$F0
        0xA0, 0x02, // LDY #$02
        0x9F, 0xFF, 0x12, // AHX $12FF,Y -> $1001 = $10
        0x4C, 0x09, 0x80, // JMP $8009
    ]);
    let cart = Cartridge::from_bytes(&rom).expect("synthetic AHX ROM must parse");
    let mut emu = Emulator::from_cartridge(cart).expect("NROM mapper must instantiate");
    emu.reset(kurosaki_core::TraceConfig::none());
    for _ in 0..4 {
        emu.step_instruction(kurosaki_core::TraceConfig::none(), false)
            .expect("AHX page-cross sequence must execute");
    }
    assert_eq!(emu.bus.ram[0x0001], 0x10);
    assert_eq!(emu.bus.ram[0x0301], 0x00);
    assert_eq!(emu.cpu.pc, 0x8009);
}

#[test]
fn brk_vectors_and_rti_returns_after_the_padding_byte() {
    let mut rom = synthetic_nrom_with_program(&[
        0x00, 0xEA, // BRK plus padding byte
        0xA9, 0x42, // LDA #$42 after RTI
        0x4C, 0x04, 0x80, // JMP $8004
    ]);
    let prg_start = 16;
    rom[prg_start + 0x0100] = 0x40; // RTI at $8100
    let vector = prg_start + 16 * 1024 - 2;
    rom[vector] = 0x00;
    rom[vector + 1] = 0x81;

    let cart = Cartridge::from_bytes(&rom).expect("synthetic BRK ROM must parse");
    let mut emu = Emulator::from_cartridge(cart).expect("NROM mapper must instantiate");
    let trace = kurosaki_core::TraceConfig::none();
    emu.reset(trace);
    emu.step_instruction(trace, false)
        .expect("BRK must enter its vector without stopping");
    assert!(!emu.cpu.stopped);
    assert_eq!(emu.cpu.pc, 0x8100);
    assert_eq!(emu.cpu.sp, 0xFA);
    assert_eq!(emu.bus.ram[0x01FD], 0x80);
    assert_eq!(emu.bus.ram[0x01FC], 0x02);
    assert_ne!(emu.bus.ram[0x01FB] & 0x10, 0, "BRK must push B=1");

    emu.step_instruction(trace, false)
        .expect("RTI must restore the BRK return state");
    assert_eq!(emu.cpu.pc, 0x8002);
    assert_eq!(emu.cpu.sp, 0xFD);
    emu.step_instruction(trace, false)
        .expect("instruction after BRK padding must execute");
    assert_eq!(emu.cpu.a, 0x42);
}

#[test]
fn cli_defers_a_pending_irq_for_one_instruction() {
    let mut rom = synthetic_nrom_with_program(&[
        0x58, // CLI
        0xEA, // this instruction must execute before IRQ polling sees I=0
        0xEA, // return point after IRQ/RTI
        0x4C, 0x03, 0x80, // JMP $8003
    ]);
    let prg_start = 16;
    rom[prg_start + 0x0100] = 0x40; // RTI at $8100
    let vector = prg_start + 16 * 1024 - 2;
    rom[vector] = 0x00;
    rom[vector + 1] = 0x81;

    let cart = Cartridge::from_bytes(&rom).expect("synthetic CLI-delay ROM must parse");
    let mut emu = Emulator::from_cartridge(cart).expect("NROM mapper must instantiate");
    let trace = kurosaki_core::TraceConfig::full();
    emu.reset(trace);
    emu.bus.irq_pending = true;

    emu.step_instruction(trace, false)
        .expect("CLI must execute");
    assert_eq!(emu.cpu.pc, 0x8001);
    emu.step_instruction(trace, false)
        .expect("instruction after CLI must execute before pending IRQ");
    assert_eq!(emu.cpu.pc, 0x8002);
    emu.step_instruction(trace, false)
        .expect("pending IRQ handler and RTI must execute next");
    assert_eq!(emu.cpu.pc, 0x8002);
    assert!(emu.trace.events.iter().any(|event| event.kind == "cpu.irq"));
}

fn synthetic_rom_with_mapper(mapper: u8) -> Vec<u8> {
    let mut prg = vec![0xEA; 16 * 1024];
    prg[0] = 0x4C;
    prg[1] = 0x00;
    prg[2] = 0x80; // JMP $8000
    let end = prg.len();
    prg[end - 6] = 0x00;
    prg[end - 5] = 0x80;
    prg[end - 4] = 0x00;
    prg[end - 3] = 0x80;
    prg[end - 2] = 0x00;
    prg[end - 1] = 0x80;
    let flags6 = (mapper & 0x0F) << 4;
    let flags7 = mapper & 0xF0;
    let mut rom = vec![
        0x4E, 0x45, 0x53, 0x1A, 1, 0, flags6, flags7, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    rom.extend_from_slice(&prg);
    rom
}

fn synthetic_banked_rom(mapper: u8, prg_32k_banks: usize, chr_8k_banks: usize) -> Vec<u8> {
    let mut rom = vec![
        0x4E,
        0x45,
        0x53,
        0x1A,
        (prg_32k_banks * 2) as u8,
        chr_8k_banks as u8,
        (mapper & 0x0F) << 4,
        mapper & 0xF0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    for bank in 0..prg_32k_banks {
        rom.extend(std::iter::repeat_n(bank as u8, 32 * 1024));
    }
    for bank in 0..chr_8k_banks {
        rom.extend(std::iter::repeat_n((0x80 + bank) as u8, 8 * 1024));
    }
    rom
}

fn synthetic_uxrom_rom(mapper: u8, prg_16k_banks: usize) -> Vec<u8> {
    let mut rom = vec![
        0x4E,
        0x45,
        0x53,
        0x1A,
        prg_16k_banks as u8,
        1,
        (mapper & 0x0F) << 4,
        mapper & 0xF0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    for bank in 0..prg_16k_banks {
        rom.extend(std::iter::repeat_n(bank as u8, 16 * 1024));
    }
    rom.extend(std::iter::repeat_n(0xCC, 8 * 1024));
    rom
}

fn synthetic_mmc2_mmc4_rom(mapper: u8) -> Vec<u8> {
    let mut rom = vec![
        0x4E,
        0x45,
        0x53,
        0x1A,
        8,
        8,
        (mapper & 0x0F) << 4,
        mapper & 0xF0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    for bank in 0..16 {
        rom.extend(std::iter::repeat_n(bank as u8, 8 * 1024));
    }
    for bank in 0..16 {
        rom.extend(std::iter::repeat_n((0x40 + bank) as u8, 4 * 1024));
    }
    rom
}

#[test]
fn simple_bank_mappers_switch_prg_and_chr_windows() {
    use kurosaki_core::TraceConfig;

    for (mapper, write_value, expected_prg, expected_chr) in [
        (11u8, 0x21u8, 2u8, 1u8),
        (34u8, 0x02u8, 2u8, 0u8),
        (66u8, 0x21u8, 2u8, 1u8),
    ] {
        let rom = synthetic_banked_rom(mapper, 4, 16);
        let cart = Cartridge::from_bytes(&rom).expect("synthetic banked ROM must parse");
        let mut emu = Emulator::from_cartridge(cart).expect("simple mapper must instantiate");
        assert!(!emu.bus.mapper.debug_state().probe_only);
        let mut trace = kurosaki_core::TraceSink::default();
        emu.bus
            .mapper
            .write_prg(0x8000, write_value, 0, 0, TraceConfig::none(), &mut trace);
        assert_eq!(
            emu.bus
                .mapper
                .read_prg(0x8000, 0, 0, TraceConfig::none(), &mut trace),
            expected_prg
        );
        assert_eq!(emu.bus.mapper.read_chr(0), 0x80 + expected_chr);

        let snapshot = emu.snapshot();
        let restored_cart = Cartridge::from_bytes(&rom).expect("ROM must parse again");
        let mut restored = Emulator::from_snapshot(restored_cart, &snapshot)
            .expect("simple mapper snapshot must restore");
        assert_eq!(
            restored
                .bus
                .mapper
                .read_prg(0x8000, 0, 0, TraceConfig::none(), &mut trace),
            expected_prg
        );
    }
}

#[test]
fn shifted_uxrom_mappers_switch_lower_prg_window_and_restore() {
    for (mapper, write_value, expected_bank) in [(71u8, 3u8, 3u8), (94u8, 0x0Cu8, 3u8)] {
        let rom = synthetic_uxrom_rom(mapper, 8);
        let cart = Cartridge::from_bytes(&rom).expect("synthetic UxROM ROM must parse");
        let mut emu = Emulator::from_cartridge(cart).expect("shifted UxROM must instantiate");
        let mut trace = kurosaki_core::TraceSink::default();
        emu.bus.mapper.write_prg(
            0x8000,
            write_value,
            0,
            0,
            kurosaki_core::TraceConfig::none(),
            &mut trace,
        );
        assert_eq!(
            emu.bus
                .mapper
                .read_prg(0x8000, 0, 0, kurosaki_core::TraceConfig::none(), &mut trace,),
            expected_bank
        );
        assert_eq!(
            emu.bus
                .mapper
                .read_prg(0xC000, 0, 0, kurosaki_core::TraceConfig::none(), &mut trace,),
            7
        );

        let snapshot = emu.snapshot();
        let restored_cart = Cartridge::from_bytes(&rom).expect("ROM must parse again");
        let mut restored = Emulator::from_snapshot(restored_cart, &snapshot)
            .expect("shifted UxROM snapshot must restore");
        assert_eq!(
            restored.bus.mapper.read_prg(
                0x8000,
                0,
                0,
                kurosaki_core::TraceConfig::none(),
                &mut trace,
            ),
            expected_bank
        );
    }
}

#[test]
fn mmc2_mmc4_latches_select_chr_banks_from_ppu_fetch_addresses() {
    for mapper in [9u8, 10u8] {
        let rom = synthetic_mmc2_mmc4_rom(mapper);
        let cart = Cartridge::from_bytes(&rom).expect("synthetic MMC2/MMC4 ROM must parse");
        let mut emu = Emulator::from_cartridge(cart).expect("MMC2/MMC4 must instantiate");
        let mut trace = kurosaki_core::TraceSink::default();
        for (addr, value) in [(0xB000, 2), (0xC000, 3), (0xD000, 4), (0xE000, 5)] {
            emu.bus.mapper.write_prg(
                addr,
                value,
                0,
                0,
                kurosaki_core::TraceConfig::none(),
                &mut trace,
            );
        }
        emu.bus.mapper.notify_ppu_addr(
            0x0FD8,
            0,
            0,
            kurosaki_core::TraceConfig::none(),
            &mut trace,
        );
        assert_eq!(emu.bus.mapper.read_chr(0), 0x42);
        emu.bus.mapper.notify_ppu_addr(
            0x0FE8,
            0,
            0,
            kurosaki_core::TraceConfig::none(),
            &mut trace,
        );
        assert_eq!(emu.bus.mapper.read_chr(0), 0x43);
        emu.bus.mapper.notify_ppu_addr(
            0x1FD8,
            0,
            0,
            kurosaki_core::TraceConfig::none(),
            &mut trace,
        );
        assert_eq!(emu.bus.mapper.read_chr(0x1000), 0x44);
        emu.bus.mapper.notify_ppu_addr(
            0x1FE8,
            0,
            0,
            kurosaki_core::TraceConfig::none(),
            &mut trace,
        );
        assert_eq!(emu.bus.mapper.read_chr(0x1000), 0x45);

        let prg_write = if mapper == 9 { 3 } else { 2 };
        let expected_prg = if mapper == 9 { 3 } else { 4 };
        emu.bus.mapper.write_prg(
            0xA000,
            prg_write,
            0,
            0,
            kurosaki_core::TraceConfig::none(),
            &mut trace,
        );
        assert_eq!(
            emu.bus
                .mapper
                .read_prg(0x8000, 0, 0, kurosaki_core::TraceConfig::none(), &mut trace,),
            expected_prg
        );
    }
}

#[test]
fn mmc3_variant_118_119_execute_chr_variant_rules() {
    use kurosaki_core::mapper::NametableMirroring;
    for mapper in [118u8, 119u8] {
        let rom = synthetic_banked_rom(mapper, 4, 16);
        let cart = Cartridge::from_bytes(&rom).expect("MMC3 variant header should parse");
        let mut emu = Emulator::from_cartridge(cart).expect("MMC3 variant should instantiate");
        assert_eq!(emu.bus.mapper.mapper_id(), mapper as u16);

        // Select CHR register 0 and point it at a bank with the variant bit set.
        let cfg = kurosaki_core::TraceConfig::none();
        let mut trace = kurosaki_core::TraceSink::default();
        emu.bus
            .mapper
            .write_prg(0x8000, 0x00, 0, 1, cfg, &mut trace);
        emu.bus
            .mapper
            .write_prg(0x8001, 0x40, 0, 2, cfg, &mut trace);
        if mapper == 118 {
            assert_eq!(
                emu.bus.mapper.nametable_mirroring(),
                NametableMirroring::Horizontal
            );
        } else {
            emu.bus.mapper.write_chr(0, 0xA5);
            assert_eq!(emu.bus.mapper.read_chr(0), 0xA5);
        }
        let snapshot = emu.bus.mapper.snapshot_bytes();
        let mut restored = Emulator::from_cartridge(Cartridge::from_bytes(&rom).unwrap()).unwrap();
        restored
            .bus
            .mapper
            .restore_snapshot_bytes(&snapshot)
            .expect("variant snapshot restore");
        assert_eq!(restored.bus.mapper.mapper_id(), mapper as u16);
    }
}

#[test]
fn cprom_switches_only_the_upper_chr_ram_half() {
    let rom = synthetic_rom_with_mapper(13);
    let cart = Cartridge::from_bytes(&rom).expect("CPROM header should parse");
    let mut emu = Emulator::from_cartridge(cart).expect("CPROM should instantiate");
    let cfg = kurosaki_core::TraceConfig::none();
    let mut trace = kurosaki_core::TraceSink::default();
    emu.bus.mapper.write_chr(0x0000, 0x11);
    emu.bus.mapper.write_chr(0x1000, 0x22);
    emu.bus.mapper.write_prg(0x8000, 1, 0, 1, cfg, &mut trace);
    emu.bus.mapper.write_chr(0x1000, 0x33);
    assert_eq!(emu.bus.mapper.read_chr(0x0000), 0x11);
    assert_eq!(emu.bus.mapper.read_chr(0x1000), 0x33);
}

#[test]
fn nina03_register_selects_prg_and_chr_banks_from_expansion_space() {
    let rom = synthetic_banked_rom(79, 4, 8);
    let cart = Cartridge::from_bytes(&rom).expect("NINA header should parse");
    let mut emu = Emulator::from_cartridge(cart).expect("NINA should instantiate");
    let mut trace = kurosaki_core::TraceSink::default();
    emu.bus.mapper.write_prg(
        0x4100,
        0x13,
        0,
        0,
        kurosaki_core::TraceConfig::none(),
        &mut trace,
    );
    assert_eq!(
        emu.bus
            .mapper
            .read_prg(0x8000, 0, 0, kurosaki_core::TraceConfig::none(), &mut trace),
        2
    );
    assert_eq!(emu.bus.mapper.read_chr(0), 0x83);
}

#[test]
fn crazy_climber_switches_the_upper_prg_window_only() {
    let rom = synthetic_uxrom_rom(180, 8);
    let cart = Cartridge::from_bytes(&rom).expect("Mapper 180 header should parse");
    let mut emu = Emulator::from_cartridge(cart).expect("Mapper 180 should instantiate");
    let mut trace = kurosaki_core::TraceSink::default();
    emu.bus.mapper.write_prg(
        0x8000,
        3,
        0,
        0,
        kurosaki_core::TraceConfig::none(),
        &mut trace,
    );
    assert_eq!(
        emu.bus
            .mapper
            .read_prg(0x8000, 0, 0, kurosaki_core::TraceConfig::none(), &mut trace),
        0
    );
    assert_eq!(
        emu.bus
            .mapper
            .read_prg(0xC000, 0, 0, kurosaki_core::TraceConfig::none(), &mut trace),
        3
    );
}

#[test]
fn vrc3_uses_16k_prg_windows_and_latched_counter_irq() {
    let rom = synthetic_uxrom_rom(73, 8);
    let cart = Cartridge::from_bytes(&rom).expect("VRC3 header should parse");
    let mut emu = Emulator::from_cartridge(cart).expect("VRC3 should instantiate");
    let mut trace = kurosaki_core::TraceSink::default();
    let cfg = kurosaki_core::TraceConfig::none();
    emu.bus.mapper.write_prg(0xF000, 3, 0, 0, cfg, &mut trace);
    assert_eq!(emu.bus.mapper.read_prg(0x8000, 0, 0, cfg, &mut trace), 3);
    assert_eq!(emu.bus.mapper.read_prg(0xC000, 0, 0, cfg, &mut trace), 7);
    for addr in [0x8000, 0x9000, 0xA000, 0xB000] {
        emu.bus.mapper.write_prg(addr, 0x0F, 0, 0, cfg, &mut trace);
    }
    emu.bus
        .mapper
        .write_prg(0xC000, 0x02, 0, 0, cfg, &mut trace);
    emu.bus.mapper.clock_cpu(1, 0, 0, cfg, &mut trace);
    assert!(emu.bus.mapper.irq_pending());
    let snapshot = emu.bus.mapper.snapshot_bytes();
    let cart = Cartridge::from_bytes(&rom).unwrap();
    let mut restored = Emulator::from_cartridge(cart).unwrap();
    restored
        .bus
        .mapper
        .restore_snapshot_bytes(&snapshot)
        .unwrap();
    assert!(restored.bus.mapper.irq_pending());
}

#[test]
fn supported_mapper_factory_accepts_phase_0_3_targets() {
    for mapper in [
        0u8, 1, 2, 3, 4, 5, 7, 16, 18, 19, 20, 21, 22, 23, 24, 25, 26, 32, 33, 48, 64, 68, 69, 73,
        75, 76, 85, 87,
    ] {
        let rom = synthetic_rom_with_mapper(mapper);
        let cart = Cartridge::from_bytes(&rom).expect("synthetic header should parse");
        assert_eq!(cart.info.mapper, mapper as u16);
        let emu = Emulator::from_cartridge(cart);
        assert!(emu.is_ok(), "mapper {mapper} should instantiate");
    }
}

#[test]
fn complex_mapper_scaffolds_are_not_probe_only() {
    for mapper in [5u8, 16, 18, 19, 21, 24, 25, 26, 69, 85] {
        let rom = synthetic_rom_with_mapper(mapper);
        let cart =
            Cartridge::from_bytes(&rom).expect("synthetic complex mapper header should parse");
        let emu =
            Emulator::from_cartridge(cart).expect("complex mapper scaffold should instantiate");
        let state = emu.bus.mapper.debug_state();
        assert!(
            !state.probe_only,
            "mapper {mapper} should use a mapper-specific scaffold"
        );
    }
}

#[test]
fn probe_only_mapper_factory_still_accepts_unknown_mapper() {
    let rom = synthetic_rom_with_mapper(111);
    let cart = Cartridge::from_bytes(&rom).expect("synthetic mapper 111 header should parse");
    assert_eq!(cart.info.mapper, 111);
    let mut emu = Emulator::from_cartridge(cart)
        .expect("probe-only mapper should instantiate for inspection-oriented execution");
    let state = emu.bus.mapper.debug_state();
    assert!(
        state.probe_only,
        "mapper 111 should remain probe-only unless a scaffold is added"
    );
    let summary = emu.run(RunOptions {
        frames: 1,
        allow_unimplemented_opcode: false,
        ..RunOptions::default()
    });
    assert!(
        summary
            .diagnostics
            .items
            .iter()
            .any(|d| d.code == "KS-MAP-PROBE"),
        "probe-only diagnostics should be present"
    );
}

#[test]
fn audio_pipeline_generates_2a03_samples_with_length_and_envelope_state() {
    use kurosaki_core::apu::ApuState;
    use kurosaki_core::{TraceConfig, TraceSink};

    let mut apu = ApuState::default();
    let mut trace = TraceSink::default();
    let cfg = TraceConfig::none();
    apu.write_register(0x4015, 0x01, 0, 0, cfg, &mut trace);
    apu.write_register(0x4000, 0b0011_1111, 0, 0, cfg, &mut trace);
    apu.write_register(0x4002, 0x80, 0, 0, cfg, &mut trace);
    apu.write_register(0x4003, 0x08, 0, 0, cfg, &mut trace);
    apu.clock_cpu_cycles(120_000, 0, 0, cfg, &mut trace);

    assert!(apu.generated_samples > 0);
    assert!(apu.sample_buffer.iter().any(|sample| *sample != 0));
    assert_eq!(apu.read_register(0x4015) & 0x01, 0x01);
}

#[test]
fn vrc6_mapper_audio_sample_becomes_nonzero_after_register_writes() {
    use kurosaki_core::mapper::Mapper;
    use kurosaki_core::mapper_scaffolds::VrcFamilyMapper;
    use kurosaki_core::{Mirroring, TraceConfig, TraceSink};

    let mut mapper = VrcFamilyMapper::new(24, vec![0; 32 * 1024], vec![], Mirroring::Horizontal);
    let mut trace = TraceSink::default();
    let cfg = TraceConfig::none();
    mapper.write_prg(0x9000, 0x8F, 0, 0, cfg, &mut trace);
    mapper.write_prg(0x9001, 0x80, 0, 0, cfg, &mut trace);
    mapper.write_prg(0x9002, 0x80, 0, 0, cfg, &mut trace);

    let total: i32 = (0..256)
        .map(|_| mapper.expansion_audio_sample(44_100).abs() as i32)
        .sum();
    assert!(total > 0, "VRC6 pulse should contribute expansion audio");
}

#[test]
fn fds_mapper_audio_sample_becomes_nonzero_after_wave_and_frequency_writes() {
    use kurosaki_core::mapper::{FdsMapper, Mapper};
    use kurosaki_core::{TraceConfig, TraceSink};

    let mut mapper = FdsMapper::new(vec![], vec![]);
    let mut trace = TraceSink::default();
    let cfg = TraceConfig::none();
    mapper.write_prg(0x4089, 0x80, 0, 0, cfg, &mut trace);
    for i in 0..64u16 {
        mapper.write_prg(0x4040 + i, (i & 0x3F) as u8, 0, 0, cfg, &mut trace);
    }
    mapper.write_prg(0x4080, 0x20, 0, 0, cfg, &mut trace);
    mapper.write_prg(0x4082, 0x80, 0, 0, cfg, &mut trace);
    mapper.write_prg(0x4083, 0x00, 0, 0, cfg, &mut trace);
    mapper.write_prg(0x4089, 0x00, 0, 0, cfg, &mut trace);

    let total: i32 = (0..512)
        .map(|_| mapper.expansion_audio_sample(44_100).abs() as i32)
        .sum();
    assert!(total > 0, "FDS wavetable should contribute expansion audio");
}
