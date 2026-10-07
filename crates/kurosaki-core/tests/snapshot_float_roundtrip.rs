use kurosaki_core::{Cartridge, Emulator, Snapshot, TraceConfig};

fn synthetic_noise_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 16 + 16 * 1024];
    rom[..4].copy_from_slice(b"NES\x1a");
    rom[4] = 1;
    // Enable noise, then vary its period in an entirely generated CPU loop.
    let code = [
        0xa9, 0x08, 0x8d, 0x15, 0x40, 0xa9, 0x1f, 0x8d, 0x0c, 0x40, 0xa2, 0x00, 0x8a, 0x29, 0x0f,
        0x8d, 0x0e, 0x40, 0xa9, 0x08, 0x8d, 0x0f, 0x40, 0xe8, 0x4c, 0x0c, 0x80,
    ];
    rom[16..16 + code.len()].copy_from_slice(&code);
    for offset in [0x3ffa, 0x3ffc, 0x3ffe] {
        rom[16 + offset..16 + offset + 2].copy_from_slice(&0x8000u16.to_le_bytes());
    }
    rom
}

fn state_bytes(emulator: &Emulator) -> Vec<u8> {
    serde_json::to_vec(&emulator.snapshot()).unwrap()
}

fn assert_exact(left: &Emulator, right: &Emulator) {
    let left = state_bytes(left);
    let right = state_bytes(right);
    let first_difference = left.iter().zip(&right).position(|(a, b)| a != b);
    assert!(
        left == right,
        "Complete snapshot differs at {first_difference:?}; lengths {} and {}",
        left.len(),
        right.len()
    );
}

#[test]
fn complete_snapshot_json_reload_and_continuation_are_bit_exact() {
    let cartridge = Cartridge::from_bytes(&synthetic_noise_rom()).unwrap();
    let mut native = Emulator::from_cartridge(cartridge.clone()).unwrap();
    native.reset(TraceConfig::none());
    // Trace history is intentionally not resumable machine state. Start both
    // paths with empty history rather than comparing reset-only diagnostics.
    native.trace = Default::default();
    for _ in 0..32 {
        native.step_frame(TraceConfig::none(), false).unwrap();
        let bytes = serde_json::to_vec_pretty(&native.snapshot()).unwrap();
        let snapshot: Snapshot = serde_json::from_slice(&bytes).unwrap();
        let mut restored = Emulator::from_snapshot(cartridge.clone(), &snapshot).unwrap();
        assert_exact(&restored, &native);
        for _ in 0..3 {
            native.step_frame(TraceConfig::none(), false).unwrap();
            restored.step_frame(TraceConfig::none(), false).unwrap();
            assert_exact(&restored, &native);
        }
    }
}
