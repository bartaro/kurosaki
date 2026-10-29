//! Bounded NSFe container and optional NSF1 metadata I/O, shared by both apps.
use super::{invalid, NsfFile, NsfHeader};
use crate::Result;

fn chunks(data: &[u8]) -> Result<Vec<(&[u8], &[u8])>> {
    let mut result = Vec::new();
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let size = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let name = &data[pos + 4..pos + 8];
        pos += 8;
        let end = pos
            .checked_add(size)
            .filter(|end| *end <= data.len())
            .ok_or_else(|| invalid("NSFe chunk exceeds the end of the file"))?;
        result.push((name, &data[pos..end]));
        pos = end;
        if name == b"NEND" {
            return Ok(result);
        }
    }
    Err(invalid("NSFe metadata is truncated or missing NEND"))
}

pub(super) fn read_metadata(data: &[u8], h: &mut NsfHeader) -> Result<()> {
    for (name, payload) in chunks(data)? {
        match name {
            b"time" | b"fade" => {
                if payload.len() % 4 != 0 {
                    return Err(invalid("invalid NSFe time/fade chunk"));
                }
                for (track, bytes) in h.tracks.iter_mut().zip(payload.chunks_exact(4)) {
                    let value = u32::try_from(i32::from_le_bytes(bytes.try_into().unwrap())).ok();
                    if name == b"time" {
                        track.time_ms = value;
                    } else {
                        track.fade_ms = value;
                    }
                }
            }
            b"tlbl" => {
                for (track, value) in h.tracks.iter_mut().zip(payload.split(|b| *b == 0)) {
                    track.name = String::from_utf8_lossy(value).into_owned();
                }
            }
            b"auth" => {
                let mut fields = payload.split(|b| *b == 0);
                for value in [&mut h.title, &mut h.artist, &mut h.copyright] {
                    if let Some(bytes) = fields.next() {
                        *value = String::from_utf8_lossy(bytes).into_owned();
                    }
                }
            }
            b"NEND" => break,
            _ if name[0].is_ascii_uppercase() => {
                return Err(invalid("unsupported required NSFe metadata chunk"))
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn from_nsfe(data: &[u8]) -> Result<NsfFile> {
    let mut header = vec![0; 128];
    header[..8].copy_from_slice(b"NESM\x1a\x01\x01\x01");
    header[110..112].copy_from_slice(&16639u16.to_le_bytes());
    header[120..122].copy_from_slice(&19997u16.to_le_bytes());
    let mut info = false;
    let mut program = None;
    let mut metadata = Vec::new();
    for (name, payload) in chunks(&data[4..])? {
        match name {
            b"INFO" => {
                if info || program.is_some() || payload.len() < 9 {
                    return Err(invalid("invalid or duplicate NSFe INFO"));
                }
                header[8..14].copy_from_slice(&payload[..6]);
                header[122] = payload[6];
                header[123] = payload[7];
                header[6] = payload[8];
                header[7] = payload
                    .get(9)
                    .copied()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| invalid("invalid NSFe starting song"))?;
                info = true;
            }
            b"DATA" => {
                if !info || program.is_some() || payload.is_empty() || payload.len() > 1024 * 1024 {
                    return Err(invalid("invalid NSFe DATA or missing INFO"));
                }
                program = Some(payload);
            }
            b"BANK" => {
                let count = payload.len().min(8);
                header[112..112 + count].copy_from_slice(&payload[..count]);
            }
            b"RATE" => {
                if payload.len() < 2 {
                    return Err(invalid("invalid NSFe RATE"));
                }
                header[110..112].copy_from_slice(&payload[..2]);
                if payload.len() >= 4 {
                    header[120..122].copy_from_slice(&payload[2..4]);
                }
            }
            b"NSF2" => return Err(invalid("NSF2 execution features are not supported")),
            _ => append(&mut metadata, name, payload),
        }
    }
    let program = program.ok_or_else(|| invalid("NSFe is missing DATA"))?;
    header[125..128].copy_from_slice(&(program.len() as u32).to_le_bytes()[..3]);
    header.extend_from_slice(program);
    header.extend(metadata);
    let mut result = NsfFile::from_bytes(&header)?;
    result.header.format = "nsfe";
    Ok(result)
}

fn append(out: &mut Vec<u8>, name: &[u8], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(payload);
}

fn metadata(h: &NsfHeader) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for (name, fade) in [(b"time", false), (b"fade", true)] {
        let mut values = Vec::new();
        for track in &h.tracks {
            let value = if fade { track.fade_ms } else { track.time_ms };
            let value = match value {
                Some(ms) => i32::try_from(ms)
                    .map_err(|_| invalid("NSFe duration exceeds signed milliseconds"))?,
                None => -1,
            };
            values.extend_from_slice(&value.to_le_bytes());
        }
        append(&mut out, name, &values);
    }
    let mut labels = Vec::new();
    for track in &h.tracks {
        labels.extend_from_slice(track.name.as_bytes());
        labels.push(0);
    }
    append(&mut out, b"tlbl", &labels);
    let auth = format!(
        "{}\0{}\0{}\0KUROSAKI / FCSEQ\0",
        h.title, h.artist, h.copyright
    );
    append(&mut out, b"auth", auth.as_bytes());
    append(&mut out, b"NEND", &[]);
    Ok(out)
}

pub(super) fn write_nsfe(file: &NsfFile) -> Result<Vec<u8>> {
    let h = &file.header;
    let mut out = b"NSFE".to_vec();
    let mut info = Vec::new();
    for value in [h.load_address, h.init_address, h.play_address] {
        info.extend_from_slice(&value.to_le_bytes());
    }
    info.extend_from_slice(&[h.region, h.expansion_bits, h.songs, h.starting_song - 1]);
    append(&mut out, b"INFO", &info);
    if h.banked {
        append(&mut out, b"BANK", &h.banks);
    }
    // Omit redundant RATE: older Mesen NSFe readers use 16639 us by default
    // and reject this required chunk even when its value is the default.
    if h.ntsc_period_us != 16639 {
        let rate = [h.ntsc_period_us.to_le_bytes(), 19997u16.to_le_bytes()].concat();
        append(&mut out, b"RATE", &rate);
    }
    append(&mut out, b"DATA", &file.data);
    out.extend(metadata(h)?);
    Ok(out)
}

pub(super) fn write_nsf(file: &NsfFile) -> Result<Vec<u8>> {
    let h = &file.header;
    let mut out = vec![0; 128];
    out[..8].copy_from_slice(&[b'N', b'E', b'S', b'M', 0x1a, 1, h.songs, h.starting_song]);
    for (at, value) in [
        (8, h.load_address),
        (10, h.init_address),
        (12, h.play_address),
        (110, h.ntsc_period_us),
        (120, 19997),
    ] {
        out[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
    for (at, text) in [(14, &h.title), (46, &h.artist), (78, &h.copyright)] {
        let mut end = text.len().min(31);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        out[at..at + end].copy_from_slice(&text.as_bytes()[..end]);
    }
    out[112..120].copy_from_slice(&h.banks);
    out[122] = h.region;
    out[123] = h.expansion_bits;
    out[125..128].copy_from_slice(&(file.data.len() as u32).to_le_bytes()[..3]);
    out.extend_from_slice(&file.data);
    out.extend(metadata(h)?);
    Ok(out)
}
