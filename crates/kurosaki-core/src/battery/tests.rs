use super::*;
use crate::{Cartridge, TraceConfig};
use std::sync::atomic::{AtomicU64, Ordering};

fn emulator(mapper: u8, battery: bool, nes20: bool) -> Emulator {
    let mut rom = vec![0; 16 + 32768];
    rom[..4].copy_from_slice(b"NES\x1a");
    rom[4] = 2;
    rom[6] = (mapper << 4) | if battery { 2 } else { 0 };
    rom[7] = if nes20 { 8 } else { 0 };
    if nes20 {
        rom[10] = 0x70;
        rom[11] = 7;
    } else {
        rom[8] = 1;
    }
    rom[16..].fill(0xea);
    rom[16..19].copy_from_slice(&[0x4c, 0x00, 0x80]);
    for offset in [0x7ffa, 0x7ffc, 0x7ffe] {
        rom[16 + offset..18 + offset].copy_from_slice(&[0x00, 0x80]);
    }
    Emulator::from_cartridge(Cartridge::from_bytes(&rom).unwrap()).unwrap()
}

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "kurosaki-battery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn save(&self) -> PathBuf {
        self.0.join("synthetic.sav")
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn battery_raw_roundtrip_preserves_non_ram_state_and_reset() {
    for mapper in [0, 1, 4] {
        for nes20 in [false, true] {
            let mut e = emulator(mapper, true, nes20);
            let before = e.snapshot();
            let data: Vec<_> = (0..8192).map(|i| (i * 17) as u8).collect();
            e.import_battery_ram(&data).unwrap();
            assert_eq!(e.battery_ram().unwrap(), Some(data.clone()));
            let after = e.snapshot();
            assert_eq!(
                serde_json::to_value(before.bus).unwrap(),
                serde_json::to_value(after.bus).unwrap()
            );
            assert_eq!(before.cpu.pc, after.cpu.pc);
            e.reset(TraceConfig::none());
            assert_eq!(e.battery_ram().unwrap(), Some(data));
        }
    }
}

#[test]
fn battery_import_is_strict_and_non_mutating_on_invalid_size() {
    let mut e = emulator(1, true, false);
    let before = e.snapshot().mapper_private;
    for n in [0, 1, 8191, 8193, 16384] {
        assert!(e.import_battery_ram(&vec![3; n]).is_err());
    }
    assert_eq!(before, e.snapshot().mapper_private);
    assert!(emulator(0, false, false).battery_ram().unwrap().is_none());
    assert!(emulator(2, true, false).battery_ram().is_err());
    let mut mixed = emulator(1, true, true);
    mixed.cartridge.raw_header[10] = 0x77;
    assert!(mixed.battery_ram().is_err());
}

#[test]
fn battery_save_reopen_and_atomic_backup() {
    let dir = Directory::new();
    let mut e = emulator(1, true, false);
    let mut session = BatterySession::open(&mut e, dir.save()).unwrap().unwrap();
    assert!(!session.flush(&e).unwrap());
    assert!(!dir.save().exists());
    e.import_battery_ram(&vec![0x51; 8192]).unwrap();
    assert!(session.flush(&e).unwrap());
    e.import_battery_ram(&vec![0xa2; 8192]).unwrap();
    assert!(session.flush(&e).unwrap());
    assert_eq!(
        fs::read(suffix(&dir.save(), ".bak")).unwrap(),
        vec![0x51; 8192]
    );
    let mut reopened = emulator(1, true, false);
    BatterySession::open(&mut reopened, dir.save()).unwrap();
    assert_eq!(reopened.battery_ram().unwrap().unwrap(), vec![0xa2; 8192]);
    assert!(!suffix(&dir.save(), ".tmp").exists());
    assert!(!suffix(&dir.save(), ".lock").exists());
}

#[test]
fn battery_external_change_and_failed_replace_keep_original() {
    let dir = Directory::new();
    let mut e = emulator(1, true, false);
    e.import_battery_ram(&vec![1; 8192]).unwrap();
    e.save_battery_file(dir.save()).unwrap();
    let mut session = BatterySession::open(&mut e, dir.save()).unwrap().unwrap();
    e.import_battery_ram(&vec![2; 8192]).unwrap();
    fs::write(dir.save(), vec![3; 8192]).unwrap();
    assert!(session.flush(&e).is_err());
    assert_eq!(fs::read(dir.save()).unwrap(), vec![3; 8192]);
    // A stale temporary file is not ours to delete; no truncate-before-write.
    fs::write(suffix(&dir.save(), ".tmp"), b"retained").unwrap();
    assert!(e.save_battery_file(dir.save()).is_err());
    assert_eq!(fs::read(dir.save()).unwrap(), vec![3; 8192]);
    assert_eq!(fs::read(suffix(&dir.save(), ".tmp")).unwrap(), b"retained");
}

#[test]
fn battery_bad_existing_sidecar_is_not_silently_discarded() {
    let dir = Directory::new();
    fs::write(dir.save(), b"truncated").unwrap();
    let mut e = emulator(1, true, false);
    let before = e.snapshot().mapper_private;
    assert!(BatterySession::open(&mut e, dir.save()).is_err());
    assert_eq!(before, e.snapshot().mapper_private);
    assert_eq!(fs::read(dir.save()).unwrap(), b"truncated");
}

#[test]
fn battery_replay_reset_is_applied_once_and_retains_ram() {
    let mut e = emulator(1, true, false);
    e.import_battery_ram(&vec![0x6d; 8192]).unwrap();
    e.reset(TraceConfig::none());
    let seed = e.snapshot();
    let mut manual = Emulator::from_snapshot(e.cartridge.clone(), &seed).unwrap();
    manual.reset(TraceConfig::none());
    for _ in 0..3 {
        manual.step_frame(TraceConfig::none(), false).unwrap();
    }
    let replay = crate::ReplayFrame {
        frame: 0,
        pad1: 0,
        pad2: 0,
        reset: true,
        duration: Some(3),
        disk_side: None,
        expected_frame_hash: None,
    };
    let result = e.run_current(crate::RunOptions {
        frames: 3,
        replay_frames: vec![replay],
        ..Default::default()
    });
    assert!(!result.stopped);
    assert_eq!(result.cpu_cycles, manual.cpu.cycles);
    assert_eq!(
        e.snapshot().mapper_private,
        manual.snapshot().mapper_private
    );
    assert_eq!(e.battery_ram().unwrap().unwrap(), vec![0x6d; 8192]);
    assert_eq!(
        e.trace
            .events
            .iter()
            .filter(|v| v.kind == "emu.reset")
            .count(),
        2
    );
}
