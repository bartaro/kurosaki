use kurosaki_core::{
    nsf::{NsfFile, NsfPlayer},
    TraceConfig,
};
#[path = "common/n163_fixture.rs"]
mod fixture;
fn player(data: &[u8], song: Option<u8>, rate: u32) -> NsfPlayer {
    NsfPlayer::new(
        &NsfFile::from_bytes(data).unwrap(),
        song,
        rate,
        TraceConfig::none(),
    )
    .unwrap()
}
#[test]
fn n163_one_to_eight_channels_have_440hz_pitch_for_both_sample_rates() {
    for rate in [44100, 48000] {
        for channels in 1..=8 {
            let mut p = player(&fixture::nsf(channels, false), None, rate);
            let samples = p.render_samples(rate as usize).unwrap();
            let crossings = samples[rate as usize / 10..]
                .windows(2)
                .filter(|w| w[0] <= 0 && w[1] > 0)
                .count();
            let hz = crossings as f64 / 0.9;
            assert!(
                (hz - 440.0).abs() < 2.0,
                "{channels} channels at {rate}: {hz} Hz"
            );
            assert_eq!(samples.len(), rate as usize);
            assert!(samples.iter().all(|v| v.unsigned_abs() < 19000));
        }
    }
}

#[test]
fn one_audible_voice_among_four_has_no_multiplexer_switching_aliases() {
    let mut data = fixture::nsf(4, false);
    let mut play = Vec::new();
    for channel in 4..7 {
        fixture::sound_ram(&mut play, 0x47 + channel * 8, 0);
    }
    play.push(0x60);
    data[128..128 + play.len()].copy_from_slice(&play);
    let pcm = player(&data, None, 44100).render_samples(44100).unwrap();
    let crossings = pcm[4410..]
        .windows(2)
        .filter(|w| w[0] <= 0 && w[1] > 0)
        .count();
    assert!((crossings as f64 / 0.9 - 440.0).abs() < 2.0);
    let peak = pcm[4410..].iter().map(|v| v.unsigned_abs()).max().unwrap();
    assert!(
        (4000..5000).contains(&peak),
        "four-channel average gain: {peak}"
    );
}
#[test]
fn banked_non_aligned_and_unbanked_nsf_produce_identical_music() {
    let plain = player(&fixture::nsf(4, false), None, 44100)
        .render_samples(12000)
        .unwrap();
    let banked = player(&fixture::nsf(4, true), None, 44100)
        .render_samples(12000)
        .unwrap();
    let mut unaligned = fixture::nsf(4, true);
    unaligned[8..14].copy_from_slice(&[0x23, 0xe1, 0x23, 0xe2, 0x23, 0xe1]);
    let nonaligned = player(&unaligned, None, 44100)
        .render_samples(12000)
        .unwrap();
    assert_eq!(plain, banked);
    assert_eq!(plain, nonaligned);
}
#[test]
fn incremental_render_matches_one_call_and_song_default_is_one_based() {
    let file = fixture::nsf(8, true);
    let whole = player(&file, Some(1), 44100).render_samples(20000).unwrap();
    let mut p = player(&file, Some(1), 44100);
    let mut pieces = Vec::new();
    for n in [1, 20, 4979, 10000, 5000] {
        pieces.extend(p.render_samples(n).unwrap());
    }
    assert_eq!(whole, pieces);
    let mut default_second = file;
    default_second[7] = 2;
    let silence = player(&default_second, None, 44100)
        .render_samples(10000)
        .unwrap();
    assert!(silence.iter().all(|v| *v == 0));
    let h = NsfFile::from_bytes(&default_second).unwrap();
    assert!(NsfPlayer::new(&h, Some(0), 44100, TraceConfig::none()).is_err());
    assert!(NsfPlayer::new(&h, Some(3), 44100, TraceConfig::none()).is_err());
    assert!(NsfPlayer::new(&h, None, 0, TraceConfig::none()).is_err());
}
#[test]
fn play_can_switch_a_four_kib_bank_and_rewrite_wave_ram() {
    let mut data = fixture::nsf(1, true);
    data.resize(128 + 12288, 0);
    data[128 + 8192] = 0x77;
    let play = [
        0xa9, 2, 0x8d, 0xff, 0x5f, 0xa9, 0, 0x8d, 0, 0xf8, 0xad, 0, 0xf0, 0x8d, 0, 0x48, 0x60,
    ];
    data[128..128 + play.len()].copy_from_slice(&play);
    let f = NsfFile::from_bytes(&data).unwrap();
    let mut p = NsfPlayer::new(
        &f,
        None,
        44100,
        TraceConfig {
            mapper: true,
            ..TraceConfig::none()
        },
    )
    .unwrap();
    p.render_samples(2000).unwrap();
    assert!(p.play_calls >= 2);
    assert!(p
        .trace
        .events
        .iter()
        .any(|e| e.kind == "mapper.expansion_audio_write" && e.value == Some(0x77)));
}
#[test]
fn parser_rejects_unsupported_or_malformed_formats() {
    assert!(NsfFile::from_bytes(&[]).is_err());
    for (offset, value) in [
        (0, b'X'),
        (5, 2),
        (6, 0),
        (7, 0),
        (7, 3),
        (122, 1),
        (123, 8),
        (123, 32),
        (124, 1),
    ] {
        let mut data = fixture::nsf(1, false);
        data[offset] = value;
        assert!(
            NsfFile::from_bytes(&data).is_err(),
            "offset {offset} value {value}"
        );
    }
    let mut overflow = fixture::nsf(1, false);
    overflow[9] = 0xff;
    assert!(NsfFile::from_bytes(&overflow).is_err());
    let mut dual = fixture::nsf(1, false);
    dual[122] = 3;
    assert!(NsfFile::from_bytes(&dual).is_ok());
}

#[test]
fn nsfe_and_nsf_metadata_preserve_banks_audio_and_fractional_track_lengths() {
    let mut original = NsfFile::from_bytes(&fixture::nsf(4, true)).unwrap();
    original.header.tracks[0].name = "Original synthetic test".into();
    original.header.tracks[0].time_ms = Some(85191);
    original.header.tracks[0].fade_ms = Some(0);
    original.header.tracks[1].time_ms = Some(1234);
    original.header.tracks[1].fade_ms = Some(56);
    let nsfe = original.to_nsfe_bytes().unwrap();
    let restored = NsfFile::from_bytes(&nsfe).unwrap();
    assert_eq!(restored.header.format, "nsfe");
    assert_eq!(restored.header.banks, original.header.banks);
    assert_eq!(restored.header.duration_ms(1), Some(85191));
    assert_eq!(restored.header.duration_ms(2), Some(1290));
    let nsf = restored.to_nsf_bytes().unwrap();
    assert_eq!(nsf[5], 1);
    assert_eq!(
        NsfFile::from_bytes(&nsf).unwrap().header.duration_ms(1),
        Some(85191)
    );
    assert_eq!(
        player(&nsfe, None, 44100).render_samples(10000).unwrap(),
        player(&nsf, None, 44100).render_samples(10000).unwrap()
    );
    for cut in [0, 4, nsfe.len() - 1, nsfe.len() - 8] {
        assert!(NsfFile::from_bytes(&nsfe[..cut]).is_err());
    }
    let mut bad = nsfe.clone();
    bad[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(NsfFile::from_bytes(&bad).is_err());
    let mut bad = nsf.clone();
    bad[125..128].copy_from_slice(&[0xff; 3]);
    assert!(NsfFile::from_bytes(&bad).is_err());
}
#[test]
fn nonreturning_play_is_an_error_instead_of_silent_success() {
    let mut data = fixture::nsf(1, false);
    data[128..131].copy_from_slice(&[0x4c, 0, 0xe0]);
    let err = player(&data, None, 44100).render_samples(100).unwrap_err();
    assert!(err.to_string().contains("did not return"));
}
#[test]
fn two_a03_dpcm_and_n163_mix_in_the_same_host() {
    let mut data = fixture::nsf(1, false);
    let mut play = Vec::new();
    for (addr, value) in [
        (0x4000, 0x3f),
        (0x4002, 0x80),
        (0x4003, 0x08),
        (0x4010, 0x0f),
        (0x4011, 0x40),
        (0x4012, 0x80),
        (0x4013, 1),
        (0x4015, 0x1f),
    ] {
        fixture::write(&mut play, addr, value);
    }
    play.push(0x60);
    data[128..128 + play.len()].copy_from_slice(&play);
    let n163 = player(&fixture::nsf(1, false), None, 44100)
        .render_samples(8000)
        .unwrap();
    let mut mixed = player(&data, None, 44100);
    let pcm = mixed.render_samples(8000).unwrap();
    assert!(mixed.dmc_sample_fetches() > 0);
    assert_ne!(pcm, n163);
    data[123] = 0;
    let apu = player(&data, None, 44100).render_samples(8000).unwrap();
    assert_ne!(pcm, apu);
}

#[test]
fn nsf_routes_existing_vrc6_vrc7_and_fds_sound_cores() {
    for chip in [1, 2, 4] {
        let mut data = fixture::nsf(1, false);
        data[123] = chip;
        let mut init = Vec::new();
        match chip {
            1 => {
                for (a, v) in [(0x9000, 0x3f), (0x9001, 0x80), (0x9002, 0x80)] {
                    fixture::write(&mut init, a, v);
                }
            }
            2 => {
                for (r, v) in [(0x30, 0x10), (0x10, 0x80), (0x20, 0x19)] {
                    fixture::write(&mut init, 0x9010, r);
                    fixture::write(&mut init, 0x9030, v);
                }
            }
            4 => {
                fixture::write(&mut init, 0x4089, 0x80);
                for n in 0..64 {
                    fixture::write(&mut init, 0x4040 + n, n as u8);
                }
                for (a, v) in [(0x4080, 0x9f), (0x4082, 0x80), (0x4083, 0), (0x4089, 0)] {
                    fixture::write(&mut init, a, v);
                }
            }
            _ => unreachable!(),
        }
        init.push(0x60);
        data[128 + 256..128 + 256 + init.len()].copy_from_slice(&init);
        let samples = player(&data, None, 44100).render_samples(5000).unwrap();
        assert!(samples.iter().any(|v| *v != 0), "chip {chip}");
        assert!(
            samples.windows(2).any(|w| w[0] != w[1]),
            "chip {chip} must oscillate"
        );
    }
}
