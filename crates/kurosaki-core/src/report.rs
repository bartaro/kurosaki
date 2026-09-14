use crate::cart::RomInfo;
use crate::diagnostics::DiagnosticReport;
use crate::emulator::RunSummary;
use crate::mapper_db::mapper_spec;

// Render stored cartridge metadata and registry classification as English
// Markdown. This performs no emulation, source lookup or additional ROM validation;
// warning text is inserted without general Markdown escaping.
pub fn rom_info_markdown(info: &RomInfo) -> String {
    let mut out = String::new();
    out.push_str("# KUROSAKI ROM Inspect Report\n\n");
    out.push_str("| Field | Value |\n|---|---|\n");
    out.push_str(&format!("| SHA-256 | `{}` |\n", info.sha256));
    out.push_str(&format!("| Header | `{:?}` |\n", info.header_kind));
    let spec = mapper_spec(info.mapper);
    out.push_str(&format!("| Mapper | `{}` |\n", info.mapper));
    out.push_str(&format!("| Mapper name | `{}` |\n", spec.name));
    out.push_str(&format!("| Mapper family | `{:?}` |\n", spec.family));
    out.push_str(&format!("| Mapper support | `{:?}` |\n", spec.support));
    out.push_str(&format!("| Submapper | `{}` |\n", info.submapper));
    out.push_str(&format!("| PRG-ROM | `{}` bytes |\n", info.prg_rom_size));
    out.push_str(&format!("| CHR-ROM | `{}` bytes |\n", info.chr_rom_size));
    out.push_str(&format!("| Mirroring | `{:?}` |\n", info.mirroring));
    out.push_str(&format!("| Battery | `{}` |\n", info.battery));
    out.push_str(&format!("| Trainer | `{}` |\n", info.trainer));
    if !info.warnings.is_empty() {
        out.push_str("\n## Warnings\n\n");
        for w in &info.warnings {
            out.push_str(&format!("- {w}\n"));
        }
    }
    out
}

// Render stored severity totals and diagnostic rows in input order. Missing
// recommendations become empty cells; totals are not recomputed, and labels are
// not escaped for Markdown table syntax.
pub fn diagnostics_markdown(report: &DiagnosticReport) -> String {
    let mut out = String::new();
    out.push_str("# KUROSAKI Diagnostics Report\n\n");
    out.push_str(&format!(
        "- Errors: {}\n- Warnings: {}\n- Infos: {}\n\n",
        report.errors, report.warnings, report.infos
    ));
    out.push_str("| Code | Severity | Title | Recommendation |\n|---|---:|---|---|\n");
    for d in &report.items {
        out.push_str(&format!(
            "| {} | {:?} | {} | {} |\n",
            d.code,
            d.severity,
            d.title,
            d.recommendation.clone().unwrap_or_default()
        ));
    }
    out
}

// Format a previously captured run summary and its optional PC hotspots.
// Counts and hashes describe that supplied summary; this neither advances the
// emulator nor establishes that a requested gameplay path was exercised.
pub fn run_summary_markdown(summary: &RunSummary) -> String {
    let mut out = String::new();
    out.push_str("# KUROSAKI Run Summary\n\n");
    out.push_str("| Field | Value |\n|---|---|\n");
    out.push_str(&format!("| Frames | `{}` |\n", summary.frames));
    out.push_str(&format!("| CPU cycles | `{}` |\n", summary.cpu_cycles));
    out.push_str(&format!("| Instructions | `{}` |\n", summary.instructions));
    out.push_str(&format!("| Stopped | `{}` |\n", summary.stopped));
    out.push_str(&format!(
        "| Stop reason | `{}` |\n",
        summary.stop_reason.clone().unwrap_or_default()
    ));
    out.push_str(&format!("| Final PC | `${:04X}` |\n", summary.final_pc));
    out.push_str(&format!(
        "| PRG banks | `{:?}` |\n",
        summary.mapper.prg_bank_window
    ));
    out.push_str(&format!(
        "| Nametable0 SHA-256 | `{}` |\n",
        summary.ppu.nametable0_sha256
    ));
    out.push_str(&format!(
        "| Nametable0 non-zero | `{}` |\n",
        summary.ppu.nametable0_nonzero
    ));
    out.push_str(&format!(
        "| Nametable0 unique tiles | `{}` |\n",
        summary.ppu.nametable0_unique_tiles
    ));
    out.push_str(&format!(
        "| Visible sprites | `{}` |\n",
        summary.ppu.visible_sprites
    ));
    out.push_str(&format!(
        "| PPUDATA writes | `{}` |\n",
        summary.ppu.data_writes
    ));
    out.push_str(&format!(
        "| PPUDATA outside VBlank | `{}` |\n",
        summary.ppu.data_writes_outside_vblank
    ));
    out.push_str(&format!(
        "| Pixel hash | `{:016X}` |\n",
        summary.ppu.last_pixel_hash
    ));
    if !summary.pc_hotspots.is_empty() {
        let hotspots = summary
            .pc_hotspots
            .iter()
            .map(|spot| format!("${:04X}:{}", spot.pc, spot.count))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("| PC hotspots | `{}` |\n", hotspots));
    }
    out.push_str(&format!(
        "| Final state hash | `{}` |\n",
        summary.final_state_hash
    ));
    out
}
