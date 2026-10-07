//! Exercise real JSON file commands with a clean-room generated noise program.
use kurosaki_core::{Cartridge, Emulator, RunOptions, Snapshot, TraceConfig};
use std::{
    fs,
    path::Path,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn run(args: &[&Path], flags: &[&str]) {
    let result = Command::new(env!("CARGO_BIN_EXE_kurosaki"))
        .args(args)
        .args(flags)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn assert_snapshot(path: &Path, native: &Emulator) {
    let restored: Snapshot = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_vec(&restored).unwrap(),
        serde_json::to_vec(&native.snapshot()).unwrap()
    );
}

#[test]
fn snapshot_inspect_and_resume_keep_exact_audio_fractions() {
    let dir = std::env::temp_dir().join(format!(
        "kurosaki-fractions-cli-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    let rom_path = dir.join("synthetic.nes");
    let mut rom = vec![0; 16 + 16384];
    rom[..4].copy_from_slice(b"NES\x1a");
    rom[4] = 1;
    let code = [
        0xa9, 0x08, 0x8d, 0x15, 0x40, 0xa9, 0x1f, 0x8d, 0x0c, 0x40, 0xa9, 0x07, 0x8d, 0x0e, 0x40,
        0xa9, 0x08, 0x8d, 0x0f, 0x40, 0x4c, 0x14, 0x80,
    ];
    rom[16..16 + code.len()].copy_from_slice(&code);
    for vector in [0x3ffa, 0x3ffc, 0x3ffe] {
        rom[16 + vector..18 + vector].copy_from_slice(&0x8000u16.to_le_bytes());
    }
    fs::write(&rom_path, &rom).unwrap();
    let cartridge = Cartridge::from_bytes(&rom).unwrap();
    let mut native = Emulator::from_cartridge(cartridge).unwrap();
    native.reset(TraceConfig::none());
    native.trace = Default::default();
    let mut bits = 0x4d59_5df4_d0f3_3173u64;
    for index in 0..32 {
        bits = bits.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        native.bus.apu.sample_clock_accum =
            f64::from_bits((1020u64 << 52) | (bits & ((1u64 << 52) - 1)));
        let start = dir.join(format!("start-{index}.json"));
        let inspected = dir.join(format!("inspect-{index}.json"));
        let reloaded = dir.join(format!("reload-{index}.json"));
        let continued = dir.join(format!("continue-{index}.json"));
        fs::write(
            &start,
            serde_json::to_vec_pretty(&native.snapshot()).unwrap(),
        )
        .unwrap();
        run(
            &[
                Path::new("snapshot-load"),
                &start,
                Path::new("--json"),
                &inspected,
            ],
            &[],
        );
        assert_snapshot(&inspected, &native);
        run(
            &[
                Path::new("snapshot-resume"),
                &rom_path,
                &start,
                Path::new("--out"),
                &reloaded,
            ],
            &["--frames", "0"],
        );
        assert_snapshot(&reloaded, &native);
        run(
            &[
                Path::new("snapshot-resume"),
                &rom_path,
                &start,
                Path::new("--out"),
                &continued,
            ],
            &["--frames", "3"],
        );
        let summary = native.run_current(RunOptions {
            frames: 3,
            ..RunOptions::default()
        });
        assert!(!summary.stopped);
        assert_snapshot(&continued, &native);
    }
    fs::remove_dir_all(dir).unwrap();
}
