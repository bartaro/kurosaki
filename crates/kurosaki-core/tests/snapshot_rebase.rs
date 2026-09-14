use kurosaki_core::hash::sha256_hex;
use kurosaki_core::{
    rebase_snapshot, Cartridge, CpuRamPatch, CpuRamPatchDigest, Emulator, MapperStateTransfer,
    PrgChangeDigest, PrgRamPatch, PrgRamPatchDigest, Snapshot, SnapshotRebaseContract,
    TargetPrgAppendDigest, SNAPSHOT_REBASE_CONTRACT_FORMAT,
};

const BANK_SIZE: usize = 16 * 1024;
const SOURCE_BANKS: usize = 16;
const TARGET_BANKS: usize = 32;
const CHANGE_OFFSET: usize = 0x2345;

// Generate mapper-1 battery-ROM bytes filled by physical bank number.
// These are state-transfer fixtures, not executable gameplay programs.
fn make_rom(bank_count: usize) -> Vec<u8> {
    let mut rom = vec![0u8; 16 + bank_count * BANK_SIZE];
    rom[0..4].copy_from_slice(b"NES\x1A");
    rom[4] = bank_count as u8;
    rom[5] = 0;
    rom[6] = 0x12;
    rom[8] = 1;
    for bank in 0..bank_count {
        rom[16 + bank * BANK_SIZE..16 + (bank + 1) * BANK_SIZE].fill(bank as u8);
    }
    rom
}

// Grow synthetic PRG from 256 to 512 KiB, alter one three-byte range and
// copy bank 15 into bank 31 as the target common replica. Preserve every
// other byte of the overlapping source payload.
fn source_and_target() -> (Vec<u8>, Vec<u8>) {
    let source = make_rom(SOURCE_BANKS);
    let mut target = make_rom(TARGET_BANKS);
    target[16..16 + SOURCE_BANKS * BANK_SIZE]
        .copy_from_slice(&source[16..16 + SOURCE_BANKS * BANK_SIZE]);
    target[16 + CHANGE_OFFSET..16 + CHANGE_OFFSET + 3].copy_from_slice(&[0x11, 0x22, 0x33]);
    let common = target[16 + 15 * BANK_SIZE..16 + 16 * BANK_SIZE].to_vec();
    target[16 + 31 * BANK_SIZE..16 + 32 * BANK_SIZE].copy_from_slice(&common);
    (source, target)
}

// Seed frame/instruction counters, selected CPU/RAM values and mapper RAM
// directly, then return both the snapshot object and the exact serialized
// bytes used for its contract digest. No frames are executed.
fn source_snapshot(source_rom: &[u8]) -> (Snapshot, Vec<u8>) {
    let cart = Cartridge::from_bytes(source_rom).expect("source fixture parses");
    let mut emulator = Emulator::from_cartridge(cart).expect("source emulator initializes");
    emulator.frame = 77;
    emulator.instructions = 12_345;
    emulator.cpu.pc = 0xC234;
    emulator.cpu.a = 0x5A;
    emulator.bus.ram[0x321] = 0xA5;
    emulator
        .bus
        .mapper
        .patch_prg_ram_bytes(0x7010, &[0x3C, 0x4D])
        .expect("source PRG-RAM can be initialized");
    let snapshot = emulator.snapshot();
    let bytes = serde_json::to_vec_pretty(&snapshot).expect("snapshot serializes");
    (snapshot, bytes)
}

// Bind the fixture identities, one three-byte PRG range, the complete
// appended suffix and one $7080 PRG-RAM patch. Start with no CPU-RAM patches.
fn contract(
    source: &Cartridge,
    target: &Cartridge,
    snapshot_bytes: &[u8],
    patch: &[u8],
) -> SnapshotRebaseContract {
    SnapshotRebaseContract {
        format: SNAPSHOT_REBASE_CONTRACT_FORMAT.to_string(),
        source_snapshot_sha256: sha256_hex(snapshot_bytes),
        source_rom_sha256: source.info.sha256.clone(),
        target_rom_sha256: target.info.sha256.clone(),
        source_mapper: source.info.mapper,
        target_mapper: target.info.mapper,
        mapper_state_transfer: MapperStateTransfer::MutableStateRetainTargetConfiguration,
        chr_rom_sha256: sha256_hex(&source.chr_rom),
        allowed_prg_changes: vec![PrgChangeDigest {
            offset: CHANGE_OFFSET,
            length: 3,
            source_sha256: sha256_hex(&source.prg_rom[CHANGE_OFFSET..CHANGE_OFFSET + 3]),
            target_sha256: sha256_hex(&target.prg_rom[CHANGE_OFFSET..CHANGE_OFFSET + 3]),
        }],
        target_prg_append: Some(TargetPrgAppendDigest {
            offset: source.prg_rom.len(),
            length: target.prg_rom.len() - source.prg_rom.len(),
            target_sha256: sha256_hex(&target.prg_rom[source.prg_rom.len()..]),
        }),
        prg_ram_patches: vec![PrgRamPatchDigest {
            cpu_address: 0x7080,
            length: patch.len(),
            sha256: sha256_hex(patch),
        }],
        cpu_ram_patches: Vec::new(),
    }
}

#[test]
// First reject ordinary restoration against the changed ROM, then rebase
// and verify selected counters/registers/RAM, target SUROM configuration,
// patch bytes and strict target restoration. This does not run the modified ROM.
fn explicit_rebase_moves_mutable_mmc1_state_and_retains_target_board() {
    let (source_rom, target_rom) = source_and_target();
    let source_cart = Cartridge::from_bytes(&source_rom).expect("source fixture parses");
    let target_cart = Cartridge::from_bytes(&target_rom).expect("target fixture parses");
    assert_ne!(source_cart.info.board_profile.as_deref(), Some("surom512"));
    assert_eq!(target_cart.info.board_profile.as_deref(), Some("surom512"));

    let (snapshot, snapshot_bytes) = source_snapshot(&source_rom);
    assert!(Emulator::from_snapshot(target_cart.clone(), &snapshot).is_err());

    let patch = vec![0xEA; 84];
    let contract = contract(&source_cart, &target_cart, &snapshot_bytes, &patch);
    let (rebased, report) = rebase_snapshot(
        source_cart,
        target_cart.clone(),
        &snapshot,
        &sha256_hex(&snapshot_bytes),
        &contract,
        &[PrgRamPatch {
            cpu_address: 0x7080,
            bytes: patch.clone(),
        }],
        &[],
    )
    .expect("reviewed compatible snapshot rebases");

    assert!(report.pass);
    assert!(report.strict_target_restore_verified);
    assert!(report.normal_snapshot_fingerprint_check_unchanged);
    assert_eq!(report.allowed_prg_change_bytes, 3);
    assert_eq!(report.target_prg_append_bytes, 16 * BANK_SIZE);
    assert_eq!(report.prg_ram_patch_bytes, 84);
    assert_eq!(report.cpu_ram_patch_bytes, 0);
    assert_eq!(rebased.rom_sha256, target_cart.info.sha256);
    assert_eq!(rebased.frame, 77);
    assert_eq!(rebased.instructions, 12_345);
    assert_eq!(rebased.cpu.pc, 0xC234);
    assert_eq!(rebased.cpu.a, 0x5A);
    assert_eq!(rebased.bus.ram[0x321], 0xA5);
    assert_eq!(rebased.mapper_private[7], 1, "target SUROM configuration");
    // The fixture assertion uses the current MMC1 private-state layout: a
    // 21-byte prefix precedes PRG RAM. Update this offset if that format changes.
    let patch_start = 21 + usize::from(0x7080u16 - 0x6000);
    assert_eq!(
        &rebased.mapper_private[patch_start..patch_start + patch.len()],
        patch
    );

    Emulator::from_snapshot(target_cart, &rebased)
        .expect("rebased snapshot passes ordinary strict target restore");
}

#[test]
// Bind the updated target fingerprint but leave one extra PRG byte outside
// the allowed range; require the specific allowlist rejection.
fn rebase_rejects_unallowlisted_prg_difference() {
    let (source_rom, mut target_rom) = source_and_target();
    target_rom[16 + 0x3456] ^= 0x80;
    let source_cart = Cartridge::from_bytes(&source_rom).expect("source fixture parses");
    let target_cart = Cartridge::from_bytes(&target_rom).expect("target fixture parses");
    let (snapshot, snapshot_bytes) = source_snapshot(&source_rom);
    let patch = vec![0xEA; 8];
    let contract = contract(&source_cart, &target_cart, &snapshot_bytes, &patch);

    let error = rebase_snapshot(
        source_cart,
        target_cart,
        &snapshot,
        &sha256_hex(&snapshot_bytes),
        &contract,
        &[PrgRamPatch {
            cpu_address: 0x7080,
            bytes: patch,
        }],
        &[],
    )
    .expect_err("an unallowlisted PRG change must fail");
    assert!(error.to_string().contains("outside the contract allowlist"));
}

#[test]
// Independently corrupt supplied patch bytes and the snapshot-file digest
// while retaining the original contract; require both calls to fail.
fn rebase_rejects_snapshot_or_patch_hash_drift() {
    let (source_rom, target_rom) = source_and_target();
    let source_cart = Cartridge::from_bytes(&source_rom).expect("source fixture parses");
    let target_cart = Cartridge::from_bytes(&target_rom).expect("target fixture parses");
    let (snapshot, snapshot_bytes) = source_snapshot(&source_rom);
    let patch = vec![0xEA; 8];
    let contract = contract(&source_cart, &target_cart, &snapshot_bytes, &patch);

    let mut wrong_patch = patch.clone();
    wrong_patch[0] ^= 1;
    assert!(rebase_snapshot(
        source_cart.clone(),
        target_cart.clone(),
        &snapshot,
        &sha256_hex(&snapshot_bytes),
        &contract,
        &[PrgRamPatch {
            cpu_address: 0x7080,
            bytes: wrong_patch,
        }],
        &[],
    )
    .is_err());

    assert!(rebase_snapshot(
        source_cart,
        target_cart,
        &snapshot,
        &"0".repeat(64),
        &contract,
        &[PrgRamPatch {
            cpu_address: 0x7080,
            bytes: patch,
        }],
        &[],
    )
    .is_err());
}

#[test]
// Apply a valid two-byte physical CPU-RAM patch and check output bytes,
// report counts and strict restoration. This case covers the accepted path;
// it does not exercise mirrored, overlapping or out-of-range patch rejection.
fn rebase_applies_only_contract_bound_physical_cpu_ram_patches() {
    let (source_rom, target_rom) = source_and_target();
    let source_cart = Cartridge::from_bytes(&source_rom).expect("source fixture parses");
    let target_cart = Cartridge::from_bytes(&target_rom).expect("target fixture parses");
    let (snapshot, snapshot_bytes) = source_snapshot(&source_rom);
    let prg_patch = vec![0xEA; 8];
    let mut contract = contract(&source_cart, &target_cart, &snapshot_bytes, &prg_patch);
    let cpu_patch = vec![0x12, 0x34];
    contract.cpu_ram_patches = vec![CpuRamPatchDigest {
        cpu_address: 0x009A,
        length: cpu_patch.len(),
        sha256: sha256_hex(&cpu_patch),
    }];

    let (rebased, report) = rebase_snapshot(
        source_cart,
        target_cart.clone(),
        &snapshot,
        &sha256_hex(&snapshot_bytes),
        &contract,
        &[PrgRamPatch {
            cpu_address: 0x7080,
            bytes: prg_patch,
        }],
        &[CpuRamPatch {
            cpu_address: 0x009A,
            bytes: cpu_patch.clone(),
        }],
    )
    .expect("contract-bound CPU-RAM patch rebases");

    assert_eq!(&rebased.bus.ram[0x009A..0x009C], cpu_patch);
    assert_eq!(report.cpu_ram_patch_count, 1);
    assert_eq!(report.cpu_ram_patch_bytes, 2);
    Emulator::from_snapshot(target_cart, &rebased)
        .expect("CPU-RAM patched snapshot passes ordinary strict restore");
}
