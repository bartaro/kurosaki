// Original synthetic fixtures: exercise mapper writes through the CPU bus and
// read the resulting nametable bytes through the real buffered PPUDATA port.
use kurosaki_core::bus::Bus;
use kurosaki_core::cart::Mirroring;
use kurosaki_core::mapper::AxromMapper;
use kurosaki_core::mapper_scaffolds::{Mmc5Mapper, Sunsoft5bMapper};
use kurosaki_core::trace::{TraceConfig, TraceSink};

fn write(bus: &mut Bus, address: u16, value: u8) {
    bus.write(
        address,
        value,
        0,
        0,
        TraceConfig::default(),
        &mut TraceSink::default(),
    );
}

fn read(bus: &mut Bus, address: u16) -> u8 {
    bus.read(
        address,
        0,
        0,
        TraceConfig::default(),
        &mut TraceSink::default(),
    )
}

// Reset the shared scroll/address latch before every independent transaction.
fn seek(bus: &mut Bus, address: u16) {
    read(bus, 0x2002);
    write(bus, 0x2006, (address >> 8) as u8);
    write(bus, 0x2006, address as u8);
}

fn put(bus: &mut Bus, address: u16, value: u8) {
    seek(bus, address);
    write(bus, 0x2007, value);
}

fn get(bus: &mut Bus, address: u16) -> u8 {
    seek(bus, address);
    read(bus, 0x2007); // Discard the previous buffered read.
    read(bus, 0x2007)
}

// Check tile and attribute addresses in all four logical tables, then modify
// one alias and require every alias of that physical page to see the write.
fn assert_pages(bus: &mut Bus, pages: [usize; 4]) {
    for (table, page) in pages.iter().enumerate() {
        let base = 0x2000 + table as u16 * 0x400;
        assert_eq!(get(bus, base + 9), [17, 34][*page]);
        assert_eq!(get(bus, base + 0x3FF), [51, 68][*page]);
        if table < 3 {
            assert_eq!(get(bus, base + 0x1009), [17, 34][*page]);
        }
    }
    put(bus, 0x2809, 85);
    for (table, page) in pages.iter().enumerate() {
        let expected = if *page == pages[2] {
            85
        } else {
            [17, 34][*page]
        };
        assert_eq!(get(bus, 0x2009 + table as u16 * 0x400), expected);
    }
}

#[test]
fn axrom_switches_single_screen_pages_without_losing_ciram_or_prg_selection() {
    let rom = (0..8).flat_map(|bank| vec![bank; 32768]).collect();
    let mut bus = Bus::new(Box::new(AxromMapper::new(rom, vec![])));
    for value in 0..=255 {
        write(&mut bus, 0x8000, value & !0x10);
        put(&mut bus, 0x2009, 17);
        put(&mut bus, 0x23FF, 51);
        write(&mut bus, 0xFFFF, value | 0x10);
        put(&mut bus, 0x2009, 34);
        put(&mut bus, 0x23FF, 68);
        write(&mut bus, 0x8000, value);
        assert_pages(&mut bus, [usize::from(value & 0x10 != 0); 4]);
        assert_eq!(read(&mut bus, 0x8000), value & 7);
        assert_eq!(read(&mut bus, 0xFFFF), value & 7);
    }
}

#[test]
fn mmc5_maps_every_ciram_only_page_arrangement() {
    let mut bus = Bus::new(Box::new(Mmc5Mapper::new(
        vec![0; 32768],
        vec![],
        Mirroring::Horizontal,
        false,
    )));
    for layout in 0..16u8 {
        write(&mut bus, 0x5105, 0);
        put(&mut bus, 0x2009, 17);
        put(&mut bus, 0x23FF, 51);
        write(&mut bus, 0x5105, 0x55);
        put(&mut bus, 0x2009, 34);
        put(&mut bus, 0x23FF, 68);
        let mut register = 0;
        let mut pages = [0; 4];
        for (table, page) in pages.iter_mut().enumerate() {
            *page = ((layout >> table) & 1) as usize;
            register |= (*page as u8) << (table * 2);
        }
        write(&mut bus, 0x5105, register);
        assert_pages(&mut bus, pages);
    }
}

#[test]
fn fme7_command_twelve_preserves_all_four_modes_and_ignores_upper_bits() {
    let mut bus = Bus::new(Box::new(Sunsoft5bMapper::new(
        vec![0; 32768],
        vec![],
        Mirroring::Vertical,
        false,
    )));
    let layouts = [[0, 1, 0, 1], [0, 0, 1, 1], [0; 4], [1; 4]];
    for value in 0..=255 {
        write(&mut bus, 0x9FFF, 0xFC); // Only the low four command bits matter.
        write(&mut bus, 0xBFFF, 2);
        put(&mut bus, 0x2009, 17);
        put(&mut bus, 0x23FF, 51);
        write(&mut bus, 0xA000, 3);
        put(&mut bus, 0x2009, 34);
        put(&mut bus, 0x23FF, 68);
        write(&mut bus, 0xA000, value);
        assert_pages(&mut bus, layouts[(value & 3) as usize]);
        assert_eq!(bus.mapper.snapshot_bytes().last(), Some(&(value & 3)));
    }
}
