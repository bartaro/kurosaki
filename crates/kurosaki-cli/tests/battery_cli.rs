//! Real subprocess cold boots using only a generated looping NROM program.
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
// Run real CLI subprocesses against a generated NROM that increments one
// battery byte per cold boot. Check two successive saves, preserved input,
// snapshot-to-save export and rejection of a short input without an output.
fn battery_cli_roundtrip_and_invalid_input() {
    let dir = std::env::temp_dir().join(format!(
        "kurosaki-battery-cli-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    let rom = dir.join("synthetic.nes");
    let mut bytes = vec![0; 16 + 16384];
    bytes[..4].copy_from_slice(b"NES\x1a");
    bytes[4] = 1;
    bytes[6] = 2;
    // INC $6000; JMP $8003: one battery write per cold boot.
    bytes[16..22].copy_from_slice(&[0xee, 0x00, 0x60, 0x4c, 0x03, 0x80]);
    for vector in [0x3ffa, 0x3ffc, 0x3ffe] {
        bytes[16 + vector..18 + vector].copy_from_slice(&[0, 0x80]);
    }
    fs::write(&rom, bytes).unwrap();
    let initial = dir.join("input.sav");
    fs::write(&initial, vec![0x40; 8192]).unwrap();
    let exe = env!("CARGO_BIN_EXE_kurosaki");
    let path = |s: &str| -> PathBuf { dir.join(s) };
    // Each invocation starts a fresh process and imports only the requested
    // raw battery sidecar before running two frames and saving both artifacts.
    let run = |input: &str, output: &str, snapshot: &str| {
        let result = Command::new(exe)
            .arg("battery-run")
            .arg(&rom)
            .arg("--sav")
            .arg(path(input))
            .args(["--frames", "2"])
            .arg("--save-out")
            .arg(path(output))
            .arg("--snapshot")
            .arg(path(snapshot))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    run("input.sav", "first.sav", "first.kss.json");
    assert_eq!(fs::read(path("first.sav")).unwrap()[0], 0x41);
    run("first.sav", "second.sav", "second.kss.json");
    assert_eq!(fs::read(path("second.sav")).unwrap()[0], 0x42);
    assert_eq!(fs::read(initial).unwrap(), vec![0x40; 8192]);
    // Export battery RAM from the second snapshot, then compare the complete
    // sidecar rather than only the incremented byte.
    let export = Command::new(exe)
        .arg("battery-export")
        .arg(&rom)
        .arg(path("second.kss.json"))
        .arg("--out")
        .arg(path("export.sav"))
        .output()
        .unwrap();
    assert!(
        export.status.success(),
        "{}",
        String::from_utf8_lossy(&export.stderr)
    );
    assert_eq!(
        fs::read(path("export.sav")).unwrap(),
        fs::read(path("second.sav")).unwrap()
    );
    fs::write(path("bad.sav"), b"short").unwrap();
    let bad = Command::new(exe)
        .arg("battery-run")
        .arg(&rom)
        .arg("--sav")
        .arg(path("bad.sav"))
        .arg("--save-out")
        .arg(path("must-not-exist.sav"))
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(!path("must-not-exist.sav").exists());
    fs::remove_dir_all(dir).unwrap();
}
