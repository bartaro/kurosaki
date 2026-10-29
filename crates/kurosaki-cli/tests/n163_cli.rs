use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
#[path = "../../kurosaki-core/tests/common/n163_fixture.rs"]
mod fixture;
fn run(args: &[&str], paths: &[(&str, &PathBuf)]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kurosaki"));
    command.args(args);
    for (flag, path) in paths {
        if !flag.is_empty() {
            command.arg(flag);
        }
        command.arg(path);
    }
    command.output().unwrap()
}
fn success(output: std::process::Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn pcm(path: &PathBuf) -> Vec<i16> {
    let data = fs::read(path).unwrap();
    assert_eq!(&data[..4], b"RIFF");
    assert_eq!(&data[8..12], b"WAVE");
    data[44..]
        .chunks_exact(2)
        .map(|v| i16::from_le_bytes([v[0], v[1]]))
        .collect()
}
#[test]
fn cli_renders_n163_nes_and_banked_unbanked_nsf_and_rejects_bad_requests() {
    let evidence = std::env::var_os("KUROSAKI_N163_EVIDENCE");
    let dir = evidence.as_ref().map(PathBuf::from).unwrap_or_else(|| {
        std::env::temp_dir().join(format!(
            "kurosaki-n163-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    });
    fs::create_dir_all(&dir).unwrap();
    let rom = dir.join("original-eight-channel.nes");
    fs::write(&rom, fixture::nes(8)).unwrap();
    let rom_wav = dir.join("rom.wav");
    let rom_json = dir.join("rom.json");
    success(run(
        &["audio-export", "--frames", "30"],
        &[("", &rom), ("--wav", &rom_wav), ("--json", &rom_json)],
    ));
    assert!(pcm(&rom_wav).iter().any(|v| *v != 0));
    let report: serde_json::Value = serde_json::from_slice(&fs::read(&rom_json).unwrap()).unwrap();
    assert_eq!(report["mapper"]["mapper"], 19);
    let mut reference = Vec::new();
    for banked in [false, true] {
        let name = if banked { "banked" } else { "unbanked" };
        let nsf = dir.join(format!("original-{name}.nsf"));
        fs::write(&nsf, fixture::nsf(8, banked)).unwrap();
        let wav = dir.join(format!("{name}.wav"));
        let json = dir.join(format!("{name}.json"));
        let trace = dir.join(format!("{name}.jsonl"));
        success(run(&["nsf-info"], &[("", &nsf)]));
        let nsfe = dir.join(format!("{name}.nsfe"));
        success(run(
            &["nsfe-export", "--seconds", "0.123"],
            &[("", &nsf), ("--out", &nsfe)],
        ));
        let converted = kurosaki_core::nsf::NsfFile::from_bytes(&fs::read(&nsfe).unwrap()).unwrap();
        assert_eq!(converted.header.duration_ms(1), Some(123));
        let exact = dir.join(format!("{name}-exact.wav"));
        success(run(&["nsf-export"], &[("", &nsfe), ("--wav", &exact)]));
        assert_eq!(pcm(&exact).len(), 5424);
        success(run(
            &[
                "nsf-export",
                "--song",
                "1",
                "--seconds",
                "1",
                "--sample-rate",
                "48000",
            ],
            &[
                ("", &nsf),
                ("--wav", &wav),
                ("--json", &json),
                ("--trace-out", &trace),
            ],
        ));
        let samples = pcm(&wav);
        assert_eq!(samples.len(), 48000);
        let crossings = samples[4800..]
            .windows(2)
            .filter(|w| w[0] <= 0 && w[1] > 0)
            .count();
        assert!((crossings as f64 / 0.9 - 440.0).abs() < 2.0);
        if banked {
            assert_eq!(reference, samples);
        } else {
            reference = samples;
        }
        assert!(fs::read_to_string(&trace)
            .unwrap()
            .contains("mapper.expansion_audio_write"));
        let silent = dir.join(format!("{name}-song2.wav"));
        success(run(
            &["nsf-export", "--song", "2", "--seconds", "0.1"],
            &[("", &nsf), ("--wav", &silent)],
        ));
        assert!(pcm(&silent).iter().all(|v| *v == 0));
        for args in [
            vec!["nsf-export", "--song", "0"],
            vec!["nsf-export", "--seconds", "NaN"],
            vec!["nsf-export", "--sample-rate", "0"],
        ] {
            let bad = dir.join("must-not-exist.wav");
            assert!(!run(&args, &[("", &nsf), ("--wav", &bad)]).status.success());
            assert!(!bad.exists());
        }
    }
    if evidence.is_none() {
        fs::remove_dir_all(dir).unwrap();
    }
}
