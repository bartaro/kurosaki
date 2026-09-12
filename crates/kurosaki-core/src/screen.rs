use crate::emulator::Emulator;
use crate::error::Result;
use crate::ppu::{PpuState, NES_HEIGHT, NES_WIDTH};
use std::fs;
use std::path::Path;

const NES_RGB: [[u8; 3]; 64] = [
    [102, 102, 102],
    [0, 42, 136],
    [20, 18, 167],
    [59, 0, 164],
    [92, 0, 126],
    [110, 0, 64],
    [108, 6, 0],
    [86, 29, 0],
    [51, 53, 0],
    [11, 72, 0],
    [0, 82, 0],
    [0, 79, 8],
    [0, 64, 77],
    [0, 0, 0],
    [0, 0, 0],
    [0, 0, 0],
    [173, 173, 173],
    [21, 95, 217],
    [66, 64, 255],
    [117, 39, 254],
    [160, 26, 204],
    [183, 30, 123],
    [181, 49, 32],
    [153, 78, 0],
    [107, 109, 0],
    [56, 135, 0],
    [12, 147, 0],
    [0, 143, 50],
    [0, 124, 141],
    [0, 0, 0],
    [0, 0, 0],
    [0, 0, 0],
    [255, 254, 255],
    [100, 176, 255],
    [146, 144, 255],
    [198, 118, 255],
    [243, 106, 255],
    [254, 110, 204],
    [254, 129, 112],
    [234, 158, 34],
    [188, 190, 0],
    [136, 216, 0],
    [92, 228, 48],
    [69, 224, 130],
    [72, 205, 222],
    [79, 79, 79],
    [0, 0, 0],
    [0, 0, 0],
    [255, 254, 255],
    [192, 223, 255],
    [211, 210, 255],
    [232, 200, 255],
    [251, 194, 255],
    [254, 196, 234],
    [254, 204, 197],
    [247, 216, 165],
    [228, 229, 148],
    [207, 239, 150],
    [189, 244, 171],
    [179, 243, 204],
    [181, 235, 242],
    [184, 184, 184],
    [0, 0, 0],
    [0, 0, 0],
];

pub fn write_emulator_png(emu: &mut Emulator, path: impl AsRef<Path>) -> Result<()> {
    let rgb = emulator_screen_rgb(emu);
    write_rgb_png(path.as_ref(), NES_WIDTH as u32, NES_HEIGHT as u32, &rgb)?;
    Ok(())
}

pub fn emulator_screen_rgb(emu: &mut Emulator) -> Vec<u8> {
    screen_rgb_from_ppu(&emu.bus.ppu)
}

pub fn screen_rgb_from_ppu(ppu: &PpuState) -> Vec<u8> {
    let mut rgb = vec![0; NES_WIDTH * NES_HEIGHT * 3];

    draw_frame_buffer(ppu, &mut rgb);

    rgb
}

fn draw_frame_buffer(ppu: &PpuState, rgb: &mut [u8]) {
    for y in 0..NES_HEIGHT {
        for x in 0..NES_WIDTH {
            let color_index = ppu.frame_buffer[y * NES_WIDTH + x];
            let color = nes_rgb(ppu, color_index);
            put_rgb(rgb, x, y, color);
        }
    }
}

fn nes_rgb(ppu: &PpuState, color_index: u8) -> [u8; 3] {
    let mut index = color_index & 0x3F;

    if ppu.mask & 0x01 != 0 {
        index &= 0x30;
    }

    let mut color = NES_RGB[index as usize];
    let emphasis = (ppu.mask >> 5) & 0x07;

    if emphasis != 0 && (index & 0x0F) <= 0x0D {
        if emphasis & 0x01 != 0 {
            color[1] = ((color[1] as f32) * 0.84) as u8;
            color[2] = ((color[2] as f32) * 0.84) as u8;
        }
        if emphasis & 0x02 != 0 {
            color[0] = ((color[0] as f32) * 0.84) as u8;
            color[2] = ((color[2] as f32) * 0.84) as u8;
        }
        if emphasis & 0x04 != 0 {
            color[0] = ((color[0] as f32) * 0.84) as u8;
            color[1] = ((color[1] as f32) * 0.84) as u8;
        }
    }

    color
}

fn put_rgb(buf: &mut [u8], x: usize, y: usize, color: [u8; 3]) {
    let i = (y * NES_WIDTH + x) * 3;
    buf[i] = color[0];
    buf[i + 1] = color[1];
    buf[i + 2] = color[2];
}

fn write_rgb_png(path: &Path, width: u32, height: u32, rgb: &[u8]) -> std::io::Result<()> {
    let stride = width as usize * 3;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);

    for y in 0..height as usize {
        raw.push(0);
        raw.extend_from_slice(&rgb[y * stride..(y + 1) * stride]);
    }

    let mut zlib = Vec::new();
    zlib.extend_from_slice(&[0x78, 0x01]);

    let mut remaining = raw.as_slice();
    while !remaining.is_empty() {
        let len = remaining.len().min(65_535);
        let final_block = len == remaining.len();
        zlib.push(if final_block { 1 } else { 0 });
        zlib.extend_from_slice(&(len as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(len as u16)).to_le_bytes());
        zlib.extend_from_slice(&remaining[..len]);
        remaining = &remaining[len..];
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = Vec::new();
    png.extend_from_slice(b"\x89PNG\r\n\x1A\n");

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    png_chunk(&mut png, b"IHDR", &ihdr);
    png_chunk(&mut png, b"IDAT", &zlib);
    png_chunk(&mut png, b"IEND", &[]);

    fs::write(path, png)
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);

    let mut crc_data = Vec::with_capacity(4 + data.len());
    crc_data.extend_from_slice(kind);
    crc_data.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_data).to_be_bytes());
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;

    for byte in data {
        a = (a + *byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }

    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;

    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }

    !crc
}
