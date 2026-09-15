use kurosaki_core::bus::Bus;
use kurosaki_core::cart::Mirroring;
use kurosaki_core::mapper::NromMapper;
use kurosaki_core::trace::{TraceConfig, TraceSink};

// DMA writes 256 successive OAMDATA bytes and wraps the 8-bit destination cursor.
fn check_destination(start: u8) {
    let mapper = NromMapper::new(vec![0; 16 * 1024], vec![0; 8 * 1024], Mirroring::Horizontal);
    let mut bus = Bus::new(Box::new(mapper));
    for i in 0..256 {
        bus.ram[0x400 + i] = (i as u8).wrapping_mul(37).wrapping_add(17) & 0xE3;
    }
    bus.ppu.oam_addr = start;
    let mut sink = TraceSink::default();
    bus.write(0x4014, 4, 0, 0, TraceConfig::none(), &mut sink);
    for i in 0..256 {
        let destination = start.wrapping_add(i as u8) as usize;
        assert_eq!(
            bus.ppu.oam[destination],
            bus.ram[0x400 + i],
            "source byte {i}, OAM destination {destination}"
        );
    }
    assert_eq!(bus.ppu.oam_addr, start);
}

#[test]
fn dma_starts_at_zero() {
    check_destination(0);
}
#[test]
fn dma_starts_at_byte_one() {
    check_destination(1);
}
#[test]
fn dma_starts_at_entry_three() {
    check_destination(12);
}
#[test]
fn dma_wraps_near_the_end() {
    check_destination(253);
}
