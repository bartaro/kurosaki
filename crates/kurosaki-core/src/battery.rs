//! Raw battery RAM sidecars. Never serialize mapper registers, CPU state or CHR ROM.
use crate::{Emulator, HeaderKind, KurosakiError, Result};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

// Construct a malformed/conflicting-save error without changing RAM or disk.
fn invalid(message: impl Into<String>) -> KurosakiError {
    KurosakiError::BatterySave(message.into())
}

// Distinguish an unsupported cartridge persistence layout from invalid save bytes.
fn unsupported(message: impl Into<String>) -> KurosakiError {
    KurosakiError::BatterySaveUnsupported(message.into())
}

impl Emulator {
    // Accept only an unambiguous supported 8 KiB PRG-NVRAM layout. Return None
    // for a cartridge without a battery; reject unsupported mapper/header combinations
    // rather than inferring a save layout from the size of an arbitrary RAM window.
    fn battery_size(&self) -> Result<Option<usize>> {
        let info = &self.cartridge.info;
        if !info.battery {
            return Ok(None);
        }
        // Do not guess banked, mixed volatile/nonvolatile, CHR-NVRAM or disk layouts.
        if !matches!(info.mapper, 0 | 1 | 4) || info.submapper != 0 {
            return Err(unsupported(format!(
                "mapper {}/{} battery layout is not implemented",
                info.mapper, info.submapper
            )));
        }
        let size = match info.header_kind {
            HeaderKind::INes => info.prg_ram_size.unwrap_or(8192),
            HeaderKind::Nes20 => {
                let h = &self.cartridge.raw_header;
                if h[10] & 15 != 0 || h[11] >> 4 != 0 || h[10] >> 4 == 0 {
                    return Err(unsupported(
                        "mixed PRG RAM or CHR NVRAM layout is not implemented",
                    ));
                }
                64usize << (h[10] >> 4)
            }
            HeaderKind::Fds => {
                return Err(unsupported(
                    "disk persistence is not a raw battery RAM sidecar",
                ))
            }
        };
        if size != 8192 || self.bus.mapper.battery_prg_ram().map(<[u8]>::len) != Some(size) {
            return Err(unsupported(
                "only an unambiguous 8 KiB physical PRG NVRAM layout is supported",
            ));
        }
        Ok(Some(size))
    }

    // Validate the persistence layout and return an owned copy of its physical NVRAM.
    // The copy contains no CPU state, mapper registers or cartridge ROM bytes.
    pub fn battery_ram(&self) -> Result<Option<Vec<u8>>> {
        Ok(self
            .battery_size()?
            .map(|_| self.bus.mapper.battery_prg_ram().unwrap().to_vec()))
    }

    // Validate the exact byte count before replacing NVRAM. A size mismatch leaves
    // RAM unchanged; volatile emulator state is not restored by this operation.
    pub fn import_battery_ram(&mut self, bytes: &[u8]) -> Result<()> {
        let size = self
            .battery_size()?
            .ok_or_else(|| invalid("cartridge has no battery RAM"))?;
        if bytes.len() != size {
            return Err(invalid(format!(
                "expected {size} bytes, received {}; RAM was not changed",
                bytes.len()
            )));
        }
        self.bus
            .mapper
            .battery_prg_ram_mut()
            .ok_or_else(|| invalid("mapper import is unavailable"))?
            .copy_from_slice(bytes);
        Ok(())
    }

    // Read the raw sidecar bytes, then apply the same layout/length checks as an in-memory import.
    pub fn load_battery_file(&mut self, path: impl AsRef<Path>) -> Result<()> {
        self.import_battery_ram(&fs::read(path)?)
    }

    /// Explicit export, with a backup of the previous file and atomic replacement.
    pub fn save_battery_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let data = self
            .battery_ram()?
            .ok_or_else(|| invalid("cartridge has no battery RAM"))?;
        let path = path.as_ref();
        if let Some(rom) = &self.cartridge.info.path {
            if let (Ok(source), Ok(target)) = (rom.canonicalize(), path.canonicalize()) {
                if source == target {
                    return Err(invalid("refusing to overwrite the loaded ROM"));
                }
            }
        }
        let old = read_optional(path)?;
        replace_save(path, &data, old.as_deref())
    }
}

/// GUI persistence context. Snapshot imports must suspend this session until an
/// explicit export; they must not silently roll back the on-disk adventure.
// The caller must drop or suspend this context after importing a snapshot.
// The session itself tracks ROM identity and RAM/disk baselines but does not
// observe snapshot imports or automatically enforce that suspension.
pub struct BatterySession {
    rom_sha256: String,
    path: PathBuf,
    disk: Option<Vec<u8>>,
    last_ram: Vec<u8>,
}

impl BatterySession {
    // Bind persistence to this ROM fingerprint and remember the observed disk contents.
    // If a sidecar exists, validate/import it before establishing the last-RAM baseline.
    pub fn open(emulator: &mut Emulator, path: PathBuf) -> Result<Option<Self>> {
        let Some(mut last_ram) = emulator.battery_ram()? else {
            return Ok(None);
        };
        let disk = read_optional(&path)?;
        if let Some(bytes) = &disk {
            emulator.import_battery_ram(bytes)?;
            last_ram.clone_from(bytes);
        }
        Ok(Some(Self {
            rom_sha256: emulator.cartridge.info.sha256.clone(),
            path,
            disk,
            last_ram,
        }))
    }

    // Borrow the sidecar path owned by this persistence session.
    pub fn path(&self) -> &Path {
        &self.path
    }

    // Save only changed RAM for the original ROM. Compare against the disk baseline
    // inside the locked replacement transaction; update session baselines only after success.
    pub fn flush(&mut self, emulator: &Emulator) -> Result<bool> {
        if emulator.cartridge.info.sha256 != self.rom_sha256 {
            return Err(invalid("save session belongs to a different ROM"));
        }
        let bytes = emulator
            .battery_ram()?
            .ok_or_else(|| invalid("cartridge has no battery RAM"))?;
        // Unchanged RAM avoids disk I/O, so an external edit is detected only
        // when a later flush actually attempts replacement.
        if bytes == self.last_ram {
            return Ok(false);
        }
        replace_save(&self.path, &bytes, self.disk.as_deref())?;
        self.last_ram.clone_from(&bytes);
        self.disk = Some(bytes);
        Ok(true)
    }
}

// Treat a missing file as an absent save, while propagating permission and other I/O errors.
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

// Append a suffix to the complete path without requiring a UTF-8 filename.
fn suffix(path: &Path, value: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(value);
    name.into()
}

struct OwnedFile(PathBuf);
impl Drop for OwnedFile {
    fn drop(&mut self) {
        // Best-effort cleanup of the temporary file owned by this guard, including error paths.
        let _ = fs::remove_file(&self.0);
    }
}

struct SaveLock {
    file: Option<fs::File>,
    path: PathBuf,
}
impl Drop for SaveLock {
    fn drop(&mut self) {
        // Close the lock handle before unlinking its path, as required by Windows file sharing.
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

// Create an exclusive sibling temporary file, write/sync all bytes, close its
// handle and rename it into place. Existing temporary files cause an error
// rather than being overwritten; the guard removes this operation's temporary file.
// Sync the temporary file contents before rename; parent-directory metadata
// is not separately synced. Locks protect only cooperating users of this API.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = suffix(path, ".tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let _cleanup = OwnedFile(temporary.clone());
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)?;
    Ok(())
}

// Acquire an exclusive cooperating-process lock, verify the expected disk bytes,
// back up the old save when present, and replace it. An external change aborts
// the transaction instead of silently overwriting another session's progress.
fn replace_save(path: &Path, bytes: &[u8], expected: Option<&[u8]>) -> Result<()> {
    // A cooperating second GUI cannot race a read/check/replace transaction.
    let lock_path = suffix(path, ".lock");
    let lock = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)?;
    let _lock = SaveLock {
        file: Some(lock),
        path: lock_path,
    };
    let current = read_optional(path)?;
    if current.as_deref() != expected {
        return Err(invalid(
            "save changed on disk; refusing to overwrite another session",
        ));
    }
    if current.as_deref() == Some(bytes) {
        return Ok(());
    }
    if let Some(old) = current {
        atomic_write(&suffix(path, ".bak"), &old)?;
    }
    atomic_write(path, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests;
