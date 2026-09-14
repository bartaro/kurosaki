use crate::cart::{Cartridge, HeaderKind};
use crate::emulator::Emulator;
use crate::error::{KurosakiError, Result};
use crate::hash::sha256_hex;
use crate::snapshot::Snapshot;
use serde::{Deserialize, Serialize};

pub const SNAPSHOT_REBASE_CONTRACT_FORMAT: &str = "kurosaki-snapshot-rebase-contract-v1";
pub const SNAPSHOT_REBASE_REPORT_FORMAT: &str = "kurosaki-snapshot-rebase-report-v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
// The only accepted transfer policy moves mutable mapper state while the
// target mapper retains its own cartridge configuration.
pub enum MapperStateTransfer {
    MutableStateRetainTargetConfiguration,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
// An allowed range relative to the PRG payload, excluding the file header
// and trainer. It binds both original and replacement bytes by SHA-256.
pub struct PrgChangeDigest {
    pub offset: usize,
    pub length: usize,
    pub source_sha256: String,
    pub target_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
// One complete target-only PRG suffix, beginning exactly at source PRG length.
pub struct TargetPrgAppendDigest {
    pub offset: usize,
    pub length: usize,
    pub target_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
// Contract for supplied patch bytes in the CPU-visible PRG-RAM window;
// the bytes themselves are passed separately.
pub struct PrgRamPatchDigest {
    pub cpu_address: u16,
    pub length: usize,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
// Contract for supplied physical CPU-RAM bytes, with no address mirroring.
pub struct CpuRamPatchDigest {
    pub cpu_address: u16,
    pub length: usize,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
// Explicit identity and modification allowlist. Unknown JSON fields are
// rejected; absent patch/change arrays default to empty and still undergo
// semantic validation before state transfer.
pub struct SnapshotRebaseContract {
    pub format: String,
    pub source_snapshot_sha256: String,
    pub source_rom_sha256: String,
    pub target_rom_sha256: String,
    pub source_mapper: u16,
    pub target_mapper: u16,
    pub mapper_state_transfer: MapperStateTransfer,
    pub chr_rom_sha256: String,
    #[serde(default)]
    pub allowed_prg_changes: Vec<PrgChangeDigest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_prg_append: Option<TargetPrgAppendDigest>,
    #[serde(default)]
    pub prg_ram_patches: Vec<PrgRamPatchDigest>,
    #[serde(default)]
    pub cpu_ram_patches: Vec<CpuRamPatchDigest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Owned PRG-RAM replacement bytes supplied separately from their digest contract.
pub struct PrgRamPatch {
    pub cpu_address: u16,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// Owned physical CPU-RAM replacement bytes supplied for the validated rebase.
pub struct CpuRamPatch {
    pub cpu_address: u16,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
// Success report emitted only after strict target restoration succeeds.
// Its pass flag proves this transfer contract, not gameplay compatibility
// or correctness of the newly supplied program code.
pub struct SnapshotRebaseReport {
    pub format: &'static str,
    pub pass: bool,
    pub source_snapshot_sha256: String,
    pub source_rom_sha256: String,
    pub target_rom_sha256: String,
    pub mapper: u16,
    pub allowed_prg_change_count: usize,
    pub allowed_prg_change_bytes: usize,
    pub target_prg_append_bytes: usize,
    pub prg_ram_patch_count: usize,
    pub prg_ram_patch_bytes: usize,
    pub cpu_ram_patch_count: usize,
    pub cpu_ram_patch_bytes: usize,
    pub frame: u64,
    pub instructions: u64,
    pub output_mapper_private_sha256: String,
    pub strict_target_restore_verified: bool,
    pub normal_snapshot_fingerprint_check_unchanged: bool,
}

// Validate identities, ROM differences and supplied RAM patches before
// creating an isolated target emulator. Require normal strict restoration of
// both source and final target snapshots; no CPU execution or file write
// occurs here. The caller supplies the snapshot-file digest and must bind it
// to the actual parsed input bytes.
pub fn rebase_snapshot(
    source_cartridge: Cartridge,
    target_cartridge: Cartridge,
    source_snapshot: &Snapshot,
    source_snapshot_file_sha256: &str,
    contract: &SnapshotRebaseContract,
    patches: &[PrgRamPatch],
    cpu_ram_patches: &[CpuRamPatch],
) -> Result<(Snapshot, SnapshotRebaseReport)> {
    validate_digest("source snapshot", source_snapshot_file_sha256)?;
    validate_contract_identity(
        &source_cartridge,
        &target_cartridge,
        source_snapshot,
        source_snapshot_file_sha256,
        contract,
    )?;
    validate_cartridge_geometry(&source_cartridge, &target_cartridge)?;
    let (changed_bytes, appended_bytes) =
        validate_prg_changes(&source_cartridge, &target_cartridge, contract)?;
    validate_prg_ram_patches(contract, patches)?;
    validate_cpu_ram_patches(contract, cpu_ram_patches)?;

    // This is intentionally strict and proves that the input snapshot is a
    // valid snapshot of the declared source ROM before any state is moved.
    Emulator::from_snapshot(source_cartridge, source_snapshot)?;

    // Keep the target cartridge for the final strict-restore check. All
    // intermediate mutations occur on the newly constructed local emulator.
    let target_for_roundtrip = target_cartridge.clone();
    let mut target = Emulator::from_cartridge(target_cartridge)?;
    target.restore_snapshot_for_rebase(source_snapshot)?;
    for patch in patches {
        target.patch_snapshot_prg_ram(patch.cpu_address, &patch.bytes)?;
    }
    for patch in cpu_ram_patches {
        target.patch_snapshot_cpu_ram(patch.cpu_address, &patch.bytes)?;
    }
    let output = target.snapshot();

    // The emitted object must pass the ordinary, fingerprint-strict restore
    // path. Rebase never relaxes the semantics of Emulator::from_snapshot.
    Emulator::from_snapshot(target_for_roundtrip, &output)?;

    let report = SnapshotRebaseReport {
        format: SNAPSHOT_REBASE_REPORT_FORMAT,
        pass: true,
        source_snapshot_sha256: source_snapshot_file_sha256.to_string(),
        source_rom_sha256: contract.source_rom_sha256.clone(),
        target_rom_sha256: contract.target_rom_sha256.clone(),
        mapper: contract.target_mapper,
        allowed_prg_change_count: contract.allowed_prg_changes.len(),
        allowed_prg_change_bytes: changed_bytes,
        target_prg_append_bytes: appended_bytes,
        prg_ram_patch_count: contract.prg_ram_patches.len(),
        prg_ram_patch_bytes: contract.prg_ram_patches.iter().map(|p| p.length).sum(),
        cpu_ram_patch_count: contract.cpu_ram_patches.len(),
        cpu_ram_patch_bytes: contract.cpu_ram_patches.iter().map(|p| p.length).sum(),
        frame: output.frame,
        instructions: output.instructions,
        output_mapper_private_sha256: output.mapper_private_sha256.clone(),
        strict_target_restore_verified: true,
        normal_snapshot_fingerprint_check_unchanged: true,
    };
    Ok((output, report))
}

// Require the supported contract/policy and exact lowercase identity
// strings for the supplied cartridges, snapshot and file digest. Cartridge
// hashes are read from metadata; this does not reread or rehash ROM files.
fn validate_contract_identity(
    source: &Cartridge,
    target: &Cartridge,
    snapshot: &Snapshot,
    snapshot_file_sha256: &str,
    contract: &SnapshotRebaseContract,
) -> Result<()> {
    if contract.format != SNAPSHOT_REBASE_CONTRACT_FORMAT {
        return snapshot_error(format!(
            "unsupported snapshot rebase contract {}; expected {SNAPSHOT_REBASE_CONTRACT_FORMAT}",
            contract.format
        ));
    }
    for (label, digest) in [
        (
            "contract source snapshot",
            contract.source_snapshot_sha256.as_str(),
        ),
        ("contract source ROM", contract.source_rom_sha256.as_str()),
        ("contract target ROM", contract.target_rom_sha256.as_str()),
        ("contract CHR-ROM", contract.chr_rom_sha256.as_str()),
    ] {
        validate_digest(label, digest)?;
    }
    if contract.source_snapshot_sha256 != snapshot_file_sha256 {
        return snapshot_error("source snapshot file hash does not match rebase contract");
    }
    if contract.source_rom_sha256 != source.info.sha256 || snapshot.rom_sha256 != source.info.sha256
    {
        return snapshot_error("source ROM identity does not match snapshot rebase contract");
    }
    if contract.target_rom_sha256 != target.info.sha256 {
        return snapshot_error("target ROM identity does not match snapshot rebase contract");
    }
    if contract.source_mapper != source.info.mapper
        || contract.target_mapper != target.info.mapper
        || snapshot.mapper.mapper != source.info.mapper
    {
        return snapshot_error("mapper identity does not match snapshot rebase contract");
    }
    if source.info.mapper != target.info.mapper {
        return snapshot_error("snapshot rebase cannot change mapper number");
    }
    if contract.mapper_state_transfer != MapperStateTransfer::MutableStateRetainTargetConfiguration
    {
        return snapshot_error("unsupported mapper-state transfer policy");
    }
    Ok(())
}

// Exclude disk images, require matching submapper, RAM totals, mirroring,
// battery and console type, allow only PRG growth, and compare all CHR bytes.
// This is the listed compatibility predicate, not equality of every header
// field: trainer bytes, region hints and inferred profile labels are not checked.
fn validate_cartridge_geometry(source: &Cartridge, target: &Cartridge) -> Result<()> {
    if matches!(source.info.header_kind, HeaderKind::Fds)
        || matches!(target.info.header_kind, HeaderKind::Fds)
    {
        return snapshot_error("FDS snapshots are not eligible for ROM rebase");
    }
    if source.info.submapper != target.info.submapper
        || source.info.prg_ram_size != target.info.prg_ram_size
        || source.info.chr_ram_size != target.info.chr_ram_size
        || source.info.mirroring != target.info.mirroring
        || source.info.battery != target.info.battery
        || source.info.console_type != target.info.console_type
    {
        return snapshot_error("source and target cartridge geometry is not rebase-compatible");
    }
    if source.prg_rom.len() > target.prg_rom.len() {
        return snapshot_error("target PRG-ROM cannot be smaller than source PRG-ROM");
    }
    if source.chr_rom != target.chr_rom {
        return snapshot_error("source and target CHR-ROM must be byte-identical");
    }
    Ok(())
}

// Verify ordered disjoint source-PRG ranges with both byte digests and at
// least one difference per range. Reject changes outside those ranges and
// require one exact digest for the entire appended suffix. Return actual
// changed-byte count plus suffix size, not the sum of allowed range lengths.
fn validate_prg_changes(
    source: &Cartridge,
    target: &Cartridge,
    contract: &SnapshotRebaseContract,
) -> Result<(usize, usize)> {
    if contract.chr_rom_sha256 != sha256_hex(&source.chr_rom) {
        return snapshot_error("CHR-ROM hash does not match snapshot rebase contract");
    }

    let overlap = source.prg_rom.len();
    let mut previous_end = 0usize;
    // Track allowlist coverage over the full source PRG payload so an
    // unlisted byte difference cannot be hidden between permitted ranges.
    let mut covered = vec![false; overlap];
    let mut changed_bytes = 0usize;
    for change in &contract.allowed_prg_changes {
        validate_digest("allowed PRG source range", &change.source_sha256)?;
        validate_digest("allowed PRG target range", &change.target_sha256)?;
        if change.length == 0 {
            return snapshot_error("allowed PRG change ranges must be non-empty");
        }
        let end = change.offset.checked_add(change.length).ok_or_else(|| {
            KurosakiError::SnapshotFormat("allowed PRG change range overflow".to_string())
        })?;
        if change.offset < previous_end || end > overlap {
            return snapshot_error(
                "allowed PRG change ranges must be ordered, disjoint, and inside source PRG-ROM",
            );
        }
        let source_range = &source.prg_rom[change.offset..end];
        let target_range = &target.prg_rom[change.offset..end];
        if change.source_sha256 != sha256_hex(source_range)
            || change.target_sha256 != sha256_hex(target_range)
        {
            return snapshot_error("allowed PRG change range hash mismatch");
        }
        let range_changed = source_range
            .iter()
            .zip(target_range)
            .filter(|(a, b)| a != b)
            .count();
        if range_changed == 0 {
            return snapshot_error("allowed PRG change range contains no changed bytes");
        }
        covered[change.offset..end].fill(true);
        changed_bytes = changed_bytes.saturating_add(range_changed);
        previous_end = end;
    }

    if source
        .prg_rom
        .iter()
        .zip(&target.prg_rom[..overlap])
        .enumerate()
        .any(|(index, (a, b))| a != b && !covered[index])
    {
        return snapshot_error("target PRG-ROM has a change outside the contract allowlist");
    }

    let appended = target.prg_rom.len() - overlap;
    match (&contract.target_prg_append, appended) {
        (None, 0) => {}
        (Some(region), length) => {
            if length == 0 {
                return snapshot_error(
                    "target PRG append is declared but the target has no append",
                );
            }
            validate_digest("target PRG append", &region.target_sha256)?;
            if region.offset != overlap || region.length != length {
                return snapshot_error(
                    "target PRG append must describe the complete appended PRG-ROM suffix",
                );
            }
            if region.target_sha256 != sha256_hex(&target.prg_rom[overlap..]) {
                return snapshot_error("target PRG append hash mismatch");
            }
        }
        (None, _) => {
            return snapshot_error("target PRG-ROM append is not declared by the contract")
        }
    }
    Ok((changed_bytes, appended))
}

// Match patches positionally to their contracts, requiring exact address,
// length and byte digest. Accept only nonempty ordered disjoint ranges in
// $6000-$7FFF; the mapper later decides whether it can apply those writes.
fn validate_prg_ram_patches(
    contract: &SnapshotRebaseContract,
    patches: &[PrgRamPatch],
) -> Result<()> {
    if contract.prg_ram_patches.len() != patches.len() {
        return snapshot_error("PRG-RAM patch count does not match snapshot rebase contract");
    }
    let mut previous_end = 0x6000usize;
    for (expected, patch) in contract.prg_ram_patches.iter().zip(patches) {
        validate_digest("PRG-RAM patch", &expected.sha256)?;
        let start = usize::from(expected.cpu_address);
        let end = start.checked_add(expected.length).ok_or_else(|| {
            KurosakiError::SnapshotFormat("PRG-RAM patch range overflow".to_string())
        })?;
        if expected.length == 0 || start < previous_end || start < 0x6000 || end > 0x8000 {
            return snapshot_error(
                "PRG-RAM patches must be non-empty, ordered, disjoint, and inside $6000-$7FFF",
            );
        }
        if patch.cpu_address != expected.cpu_address
            || patch.bytes.len() != expected.length
            || sha256_hex(&patch.bytes) != expected.sha256
        {
            return snapshot_error("PRG-RAM patch bytes do not match snapshot rebase contract");
        }
        previous_end = end;
    }
    Ok(())
}

// Match patches positionally and verify their digests before accepting
// nonempty ordered disjoint ranges in physical $0000-$07FF. Mirrored CPU
// RAM addresses are deliberately outside this contract.
fn validate_cpu_ram_patches(
    contract: &SnapshotRebaseContract,
    patches: &[CpuRamPatch],
) -> Result<()> {
    if contract.cpu_ram_patches.len() != patches.len() {
        return snapshot_error("CPU-RAM patch count does not match snapshot rebase contract");
    }
    let mut previous_end = 0usize;
    for (expected, patch) in contract.cpu_ram_patches.iter().zip(patches) {
        validate_digest("CPU-RAM patch", &expected.sha256)?;
        let start = usize::from(expected.cpu_address);
        let end = start.checked_add(expected.length).ok_or_else(|| {
            KurosakiError::SnapshotFormat("CPU-RAM patch range overflow".to_string())
        })?;
        if expected.length == 0 || start < previous_end || end > 0x0800 {
            return snapshot_error(
                "CPU-RAM patches must be non-empty, ordered, disjoint, and inside $0000-$07FF",
            );
        }
        if patch.cpu_address != expected.cpu_address
            || patch.bytes.len() != expected.length
            || sha256_hex(&patch.bytes) != expected.sha256
        {
            return snapshot_error("CPU-RAM patch bytes do not match snapshot rebase contract");
        }
        previous_end = end;
    }
    Ok(())
}

// Require exactly 64 lowercase hexadecimal characters. This validates the
// representation only; callers separately compare digests with their data.
fn validate_digest(label: &str, digest: &str) -> Result<()> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return snapshot_error(format!(
            "{label} SHA-256 must be 64 lowercase hex characters"
        ));
    }
    Ok(())
}

// Return a snapshot-format error with the requested message for any result type.
fn snapshot_error<T>(message: impl Into<String>) -> Result<T> {
    Err(KurosakiError::SnapshotFormat(message.into()))
}
