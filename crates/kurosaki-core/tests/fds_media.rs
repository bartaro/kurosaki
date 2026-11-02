use kurosaki_core::{
    fds::{FdsDiskImage, FDS_SIDE_SIZE},
    mapper::{FdsMapper, Mapper, NametableMirroring},
    TraceConfig, TraceSink,
};
fn media() -> Vec<u8> {
    let mut side = vec![0; FDS_SIDE_SIZE];
    side[0] = 1;
    side[1..15].copy_from_slice(b"*NINTENDO-HVC*");
    side[56] = 2;
    side[57] = 1;
    side[58] = 3;
    side[71] = 4;
    side[74] = 4;
    side[75..79].copy_from_slice(&[0x5a, 0x37, 0xa1, 0xfe]);
    side
}

#[test]
fn cpu_boots_at_bios_and_receives_disk_bytes_through_registers() {
    use kurosaki_core::{Cartridge, Emulator, RunOptions};
    // Original tiny firmware: poll data-ready, read two bytes into $6000, loop.
    let program = [
        0x78, 0xa9, 3, 0x8d, 0x23, 0x40, 0xa9, 0x45, 0x8d, 0x25, 0x40, 0xa2, 0, 0xad, 0x30, 0x40,
        0x29, 0x80, 0xf0, 0xf9, 0xad, 0x31, 0x40, 0x9d, 0, 0x60, 0xe8, 0xe0, 2, 0xd0, 0xee, 0x4c,
        0x1f, 0xe0,
    ];
    let mut bios = vec![0; 8192];
    bios[..program.len()].copy_from_slice(&program);
    bios[0x1ffc..0x1ffe].copy_from_slice(&[0, 0xe0]);
    let cart = Cartridge::from_fds_bytes_with_bios(&media(), bios).unwrap();
    assert_eq!(cart.fds_fast_boot_pc, None);
    let mut emu = Emulator::from_cartridge(cart).unwrap();
    emu.reset(TraceConfig::none());
    assert_eq!(emu.cpu.pc, 0xe000);
    let options = RunOptions {
        frames: 64,
        ..RunOptions::default()
    };
    let summary = emu.run_current(options);
    assert!(!summary.stopped, "{:?}", summary.stop_reason);
    let mut sink = TraceSink::default();
    assert_eq!(
        emu.bus
            .mapper
            .read_prg(0x6000, 0, 0, TraceConfig::none(), &mut sink),
        1
    );
    assert_eq!(
        emu.bus
            .mapper
            .read_prg(0x6001, 0, 0, TraceConfig::none(), &mut sink),
        b'*'
    );
    assert_eq!(emu.cpu.pc, 0xe01f);
}
fn mapper() -> FdsMapper {
    FdsMapper::new_with_disk(vec![0; 8192], FdsDiskImage::from_bytes(&media()).unwrap())
}
fn write(m: &mut FdsMapper, a: u16, v: u8) {
    m.write_prg(a, v, 0, 0, TraceConfig::none(), &mut TraceSink::default());
}
fn read(m: &mut FdsMapper, a: u16) -> u8 {
    m.read_prg(a, 0, 0, TraceConfig::none(), &mut TraceSink::default())
}
fn clock(m: &mut FdsMapper, n: u64) {
    m.clock_cpu(n, 0, 0, TraceConfig::none(), &mut TraceSink::default());
}
#[test]
fn disk_reads_stream_real_blocks_and_hold_until_next_byte() {
    let mut m = mapper();
    write(&mut m, 0x4023, 3);
    write(&mut m, 0x4025, 0xc5);
    clock(&mut m, 50_000 + 3537 * 150 + 150); // sync then block 1
    assert_eq!(read(&mut m, 0x4030) & 2, 2); // BIOS polls D1 for a transferred byte.
    assert_eq!(read(&mut m, 0x4031), 1);
    assert_eq!(read(&mut m, 0x4031), 1);
    clock(&mut m, 150);
    assert_eq!(read(&mut m, 0x4031), b'*');
    assert!(!m.irq_pending());
    clock(&mut m, 150);
    assert!(m.irq_pending());
    m.clear_irq();
    assert!(m.irq_pending());
    read(&mut m, 0x4030);
    assert!(!m.irq_pending());
    write(&mut m, 0x4025, 0xcd);
    assert_eq!(m.nametable_mirroring(), NametableMirroring::Horizontal);
    write(&mut m, 0x4023, 0);
    clock(&mut m, 300);
    assert!(!m.irq_pending());
}
#[test]
fn media_snapshot_continues_mid_byte_identically_and_timer_is_independent() {
    let mut a = mapper();
    write(&mut a, 0x4025, 0xc5);
    clock(&mut a, 584_999);
    let mut b = mapper();
    b.restore_snapshot_bytes(&a.snapshot_bytes()).unwrap();
    for _ in 0..40 {
        clock(&mut a, 149);
        clock(&mut b, 149);
        assert_eq!(read(&mut a, 0x4031), read(&mut b, 0x4031));
    }
    assert_eq!(a.snapshot_bytes(), b.snapshot_bytes());
    write(&mut a, 0x4020, 2);
    write(&mut a, 0x4022, 2);
    clock(&mut a, 3);
    read(&mut a, 0x4031);
    assert_eq!(read(&mut a, 0x4030) & 1, 1);
}
#[test]
fn side_selection_and_write_protection_are_explicit() {
    let mut disk = media();
    let mut second = media();
    second[1] = b'Z';
    disk.extend(second);
    let mut m = FdsMapper::new_with_disk(vec![0; 8192], FdsDiskImage::from_bytes(&disk).unwrap());
    m.insert_fds_side(None).unwrap();
    assert_eq!(read(&mut m, 0x4032) & 7, 7);
    assert!(m.insert_fds_side(Some(2)).is_err());
    m.insert_fds_side(Some(1)).unwrap();
    write(&mut m, 0x4025, 0xc5);
    clock(&mut m, 50_000 + 3537 * 150 + 300);
    assert_eq!(read(&mut m, 0x4031), b'Z');
    assert_eq!(read(&mut m, 0x4032) & 4, 4);
    let mut sink = TraceSink::default();
    write(&mut m, 0x4025, 1);
    m.write_prg(0x4024, 99, 0, 0, TraceConfig::full(), &mut sink);
    assert!(sink.events.iter().any(|e| e.kind == "fds.write_protected"));
}
