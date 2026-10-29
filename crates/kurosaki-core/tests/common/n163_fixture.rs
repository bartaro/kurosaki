//! Original synthetic wavetable music; no commercial ROM or music data.
#![allow(dead_code)]
pub fn write(code: &mut Vec<u8>, addr: u16, value: u8) {
    code.extend_from_slice(&[0xa9, value, 0x8d, addr as u8, (addr >> 8) as u8]);
}
pub fn sound_ram(code: &mut Vec<u8>, addr: u8, value: u8) {
    write(code, 0xf800, addr);
    write(code, 0x4800, value);
}
pub fn program(channels: u8) -> Vec<u8> {
    let mut code = vec![0x85, 0x00]; // retain zero-based NSF song
    write(&mut code, 0xe000, 0x40); // disable while programming
    write(&mut code, 0xf800, 0x80);
    // 32-sample symmetric square, centered around the DAC bias (8).
    for byte in (0..16).map(|i| if i < 8 { 0x11 } else { 0xff }) {
        write(&mut code, 0x4800, byte);
    }
    for channel in 8 - channels..8 {
        let base = 0x40 + channel * 8;
        let frequency =
            ((440.0 * 32.0 * 65536.0 * 15.0 * channels as f64 / 1_789_773.0).round()) as u32;
        for (offset, value) in [
            (0, frequency as u8),
            (1, 0),
            (2, (frequency >> 8) as u8),
            (3, 0),
            (4, 0xe0 | (frequency >> 16) as u8),
            (5, 0),
            (6, 0),
            (7, 15 | if channel == 7 { (channels - 1) << 4 } else { 0 }),
        ] {
            sound_ram(&mut code, base + offset, value);
        }
    }
    // Song 2 is a deliberate rest, for song selection/zero initialization tests.
    code.extend_from_slice(&[0xa5, 0x00, 0xf0, 6]);
    write(&mut code, 0xe000, 0x40);
    code.push(0x60);
    write(&mut code, 0xe000, 0);
    code.push(0x60);
    code
}
pub fn nes(channels: u8) -> Vec<u8> {
    let mut data = vec![0xea; 16 + 32768];
    data[..16].fill(0);
    data[..4].copy_from_slice(b"NES\x1a");
    data[4] = 2;
    data[6] = 0x30;
    data[7] = 0x10;
    let reset = [
        0x78, 0xa2, 0xff, 0x9a, 0xa9, 0, 0x20, 0x00, 0xe1, 0x4c, 0x09, 0xe0,
    ];
    data[16 + 0x6000..16 + 0x6000 + reset.len()].copy_from_slice(&reset);
    let init = program(channels);
    data[16 + 0x6100..16 + 0x6100 + init.len()].copy_from_slice(&init);
    for v in [0x7ffa, 0x7ffc, 0x7ffe] {
        data[16 + v..18 + v].copy_from_slice(&[0, 0xe0]);
    }
    data
}
pub fn nsf(channels: u8, banked: bool) -> Vec<u8> {
    let mut data = vec![0; 128 + 8192];
    data[..5].copy_from_slice(b"NESM\x1a");
    data[5] = 1;
    data[6] = 2;
    data[7] = 1;
    data[8..14].copy_from_slice(&[0, 0xe0, 0, 0xe1, 0, 0xe0]);
    let title = b"KUROSAKI N163 original fixture";
    data[14..14 + title.len()].copy_from_slice(title);
    data[123] = 16;
    if banked {
        data[119] = 1;
    }
    data[128] = 0x60; // PLAY returns; sustained tone established by INIT
    let code = program(channels);
    data[128 + 256..128 + 256 + code.len()].copy_from_slice(&code);
    data
}
