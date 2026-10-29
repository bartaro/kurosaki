use kurosaki_core::{Cartridge, Emulator, TraceConfig};

fn synthetic_loop() -> Emulator {
    let mut rom = vec![0; 16 + 16 * 1024];
    rom[..4].copy_from_slice(b"NES\x1a");
    rom[4] = 1;
    rom[16..20].copy_from_slice(&[0xea, 0x4c, 0x00, 0x80]);
    for offset in [rom.len() - 6, rom.len() - 4, rom.len() - 2] {
        rom[offset..offset + 2].copy_from_slice(&[0x00, 0x80]);
    }
    let cart = Cartridge::from_bytes(&rom).expect("synthetic cartridge");
    let mut engine = Emulator::from_cartridge(cart).expect("synthetic mapper");
    engine.reset(TraceConfig::none());
    engine
}

#[test]
fn instruction_steps_count_completed_ppu_frames() {
    let mut engine = synthetic_loop();
    for _ in 0..50_000 {
        engine.step_instruction(TraceConfig::none(), false).unwrap();
        assert_eq!(engine.frame, engine.bus.ppu.rendered_frames);
        if engine.frame == 3 {
            return;
        }
    }
    panic!("instruction execution must cross three complete frames");
}

#[test]
fn frame_and_instruction_steps_have_identical_complete_snapshots() {
    let mut frames = synthetic_loop();
    let mut instructions = synthetic_loop();
    for _ in 0..3 {
        frames.step_frame(TraceConfig::none(), false).unwrap();
    }
    for _ in 0..50_000 {
        if instructions.bus.ppu.rendered_frames == 3 {
            break;
        }
        instructions
            .step_instruction(TraceConfig::none(), false)
            .unwrap();
    }
    assert_eq!(instructions.bus.ppu.rendered_frames, 3);
    assert!(
        serde_json::to_vec(&frames.snapshot()).unwrap()
            == serde_json::to_vec(&instructions.snapshot()).unwrap(),
        "complete serialized snapshots must match across stepping modes"
    );
}

#[test]
fn halted_frame_step_does_not_invent_completed_frames() {
    let mut engine = synthetic_loop();
    engine.cpu.stopped = true;
    engine.step_frame(TraceConfig::none(), false).unwrap();
    assert_eq!(engine.frame, 0);
    assert_eq!(engine.bus.ppu.rendered_frames, 0);
}

#[test]
fn instruction_trace_frame_labels_advance_with_the_ppu() {
    let mut engine = synthetic_loop();
    let trace = TraceConfig {
        cpu: true,
        ..TraceConfig::none()
    };
    for _ in 0..30_000 {
        engine.step_instruction(trace, false).unwrap();
        if engine.bus.ppu.rendered_frames == 2 {
            break;
        }
    }
    let frames: Vec<_> = engine
        .trace
        .events
        .iter()
        .filter(|event| event.kind == "cpu.instruction")
        .map(|event| event.frame)
        .collect();
    assert!(frames.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(
        frames.contains(&1),
        "later-frame instructions need later frame labels"
    );
}
