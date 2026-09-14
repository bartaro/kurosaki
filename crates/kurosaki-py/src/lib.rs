// PyO3's generated wrappers convert `PyErr` through a uniform result path;
// clippy 1.93 reports those macro-expanded conversions at the return type.
#![allow(clippy::useless_conversion)]

use kurosaki_core::{
    all_mapper_specs, diagnostic_events_from_trace_and_report, implemented_mapper_specs,
    mapper_spec, screen, Cartridge, DiagnosticEvent, Emulator as CoreEmulator, RunOptions,
    TraceConfig,
};
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use std::fs;
use std::io::Write;
use zip::{write::FileOptions, ZipWriter};

#[pyclass]
// Python-owned core instance. Methods operate synchronously on this state;
// execution loops do not explicitly release the Python interpreter lock.
pub struct Emulator {
    inner: CoreEmulator,
}

#[pymethods]
impl Emulator {
    #[staticmethod]
    #[pyo3(signature = (path, battery_path=None))]
    // Load a cartridge, optionally import its raw battery sidecar, then reset
    // the CPU before exposing the instance. Battery import is explicit; there
    // is no automatic save session attached to this Python object.
    pub fn from_rom(path: String, battery_path: Option<String>) -> PyResult<Self> {
        let cart = Cartridge::load_file(&path).map_err(to_py_err)?;
        let mut inner = CoreEmulator::from_cartridge(cart).map_err(to_py_err)?;
        if let Some(path) = battery_path {
            inner.load_battery_file(path).map_err(to_py_err)?;
        }
        inner.reset(TraceConfig::none());
        Ok(Self { inner })
    }

    #[staticmethod]
    // Load a cartridge and UTF-8 snapshot JSON, then use the normal strict
    // ROM-fingerprint restore path without resetting the restored CPU.
    pub fn from_snapshot(rom_path: String, snapshot_path: String) -> PyResult<Self> {
        let cart = Cartridge::load_file(&rom_path).map_err(to_py_err)?;
        let snapshot: kurosaki_core::Snapshot =
            serde_json::from_str(&fs::read_to_string(snapshot_path).map_err(to_py_err)?)
                .map_err(to_py_err)?;
        let inner = CoreEmulator::from_snapshot(cart, &snapshot).map_err(to_py_err)?;
        Ok(Self { inner })
    }

    // Restore supplied JSON into this instance using the core checks. Mapper
    // restoration precedes bus validation, so a later failure need not leave
    // every component unchanged; transient trace is cleared after success.
    pub fn load_snapshot(&mut self, path: String) -> PyResult<()> {
        let snapshot: kurosaki_core::Snapshot =
            serde_json::from_str(&fs::read_to_string(path).map_err(to_py_err)?)
                .map_err(to_py_err)?;
        self.inner.restore_snapshot(&snapshot).map_err(to_py_err)
    }

    // Reset SP/status/PC and the CPU cycle counter, then clear PC hotspots.
    // A/X/Y, RAM, software frame/instruction counters and prior trace events
    // are retained; this does not construct a fresh machine.
    pub fn reset(&mut self) {
        self.inner.reset(TraceConfig::none());
    }

    /// Explicit sidecar I/O; analysis sessions never auto-overwrite user saves.
    // Import an explicitly named raw sidecar through the core layout/length
    // checks; no CPU reset or snapshot restoration is performed.
    pub fn load_battery_file(&mut self, path: String) -> PyResult<()> {
        self.inner.load_battery_file(path).map_err(to_py_err)
    }

    // Export the supported physical battery RAM through the core backup and
    // replacement path, including its protection against overwriting the loaded ROM.
    pub fn save_battery_file(&self, path: String) -> PyResult<()> {
        self.inner.save_battery_file(path).map_err(to_py_err)
    }

    // Return an owned RAM byte vector, None for a cartridge without a battery,
    // or an error for an unsupported persistence layout.
    pub fn battery_ram(&self) -> PyResult<Option<Vec<u8>>> {
        self.inner.battery_ram().map_err(to_py_err)
    }

    // Advance one core frame step with unsupported opcodes converted to a
    // stopped CPU. Successful return alone does not prove a whole PPU frame
    // was rendered; the software frame counter can advance after an early stop.
    pub fn step_frame(&mut self) -> PyResult<()> {
        self.inner
            .step_frame(TraceConfig::none(), true)
            .map_err(to_py_err)
    }

    // Request one core instruction step with unsupported opcodes converted to
    // a stopped CPU instead of a propagated opcode error.
    pub fn step_instruction(&mut self) -> PyResult<()> {
        self.inner
            .step_instruction(TraceConfig::none(), true)
            .map_err(to_py_err)
    }

    // Execute at most count steps, stopping before a step if the CPU is stopped.
    // Return the cumulative core instruction counter, not the number of steps
    // performed by this call.
    pub fn step_instructions(&mut self, count: u64) -> PyResult<u64> {
        for _ in 0..count {
            if self.inner.cpu.stopped {
                break;
            }
            self.inner
                .step_instruction(TraceConfig::none(), true)
                .map_err(to_py_err)?;
        }
        Ok(self.inner.instructions)
    }

    // Continue with the current controller masks and return the full run summary
    // as JSON. Runtime stops are represented by summary fields; frame/cycle/
    // instruction totals describe the cumulative state.
    pub fn step_frames(&mut self, frames: u64) -> PyResult<String> {
        let summary = self.inner.run_current(RunOptions {
            frames,
            pad1: self.inner.bus.controller_state[0],
            pad2: self.inner.bus.controller_state[1],
            allow_unimplemented_opcode: true,
            ..RunOptions::default()
        });
        serde_json::to_string_pretty(&summary).map_err(to_py_err)
    }

    // Check the CPU address before each bounded instruction step and once after
    // the budget. Stop on a stopped CPU unless the target PC already matches;
    // matching the address does not identify a particular physical ROM bank.
    pub fn run_until_pc(&mut self, pc: u16, max_instructions: u64) -> PyResult<bool> {
        for _ in 0..max_instructions {
            if self.inner.cpu.pc == pc {
                return Ok(true);
            }
            if self.inner.cpu.stopped {
                return Ok(false);
            }
            self.inner
                .step_instruction(TraceConfig::none(), true)
                .map_err(to_py_err)?;
        }
        Ok(self.inner.cpu.pc == pc)
    }

    #[pyo3(signature = (pc, max_instructions, clear_trace=true))]
    // Optionally clear accumulated trace, then perform the bounded PC search
    // with debugger selectors. Retained traces can include earlier execution;
    // a matching PC is reported before its instruction executes.
    pub fn run_until_pc_traced(
        &mut self,
        pc: u16,
        max_instructions: u64,
        clear_trace: bool,
    ) -> PyResult<bool> {
        if clear_trace {
            self.inner.trace = Default::default();
        }
        let trace = debugger_trace_config();
        for _ in 0..max_instructions {
            if self.inner.cpu.pc == pc {
                return Ok(true);
            }
            if self.inner.cpu.stopped {
                return Ok(false);
            }
            self.inner
                .step_instruction(trace, true)
                .map_err(to_py_err)?;
        }
        Ok(self.inner.cpu.pc == pc)
    }

    // Discard accumulated events without changing CPU/device state or counters.
    pub fn clear_trace(&mut self) {
        self.inner.trace = Default::default();
    }

    // Take at most max_frames core frame steps until the software frame count
    // reaches the requested value. There is no separate stopped-CPU check here,
    // so reaching that counter does not guarantee newly rendered frames.
    pub fn run_until_frame(&mut self, frame: u64, max_frames: u64) -> PyResult<bool> {
        for _ in 0..max_frames {
            if self.inner.frame >= frame {
                return Ok(true);
            }
            self.inner
                .step_frame(TraceConfig::none(), true)
                .map_err(to_py_err)?;
        }
        Ok(self.inner.frame >= frame)
    }

    // Advance one frame per iteration, then search diagnostics built from all
    // retained trace and cumulative counters. Earlier events can satisfy the
    // selector. ALL or * succeeds after the first iteration even with no events;
    // zero max_frames always returns false.
    pub fn run_until_diagnostic(&mut self, event_type: String, max_frames: u64) -> PyResult<bool> {
        // Only ASCII case conversion is applied to this diagnostic selector.
        let selector = event_type.to_ascii_uppercase();
        for _ in 0..max_frames {
            let summary = self.inner.run_current(RunOptions {
                frames: 1,
                pad1: self.inner.bus.controller_state[0],
                pad2: self.inner.bus.controller_state[1],
                trace: diagnostic_trace_config(true),
                allow_unimplemented_opcode: true,
                ..RunOptions::default()
            });
            let events =
                diagnostic_events_from_trace_and_report(&self.inner.trace, &summary.diagnostics);
            if diagnostic_matches(&events, &selector) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[pyo3(signature = (pad1, pad2=None))]
    // Replace both live controller masks. Omitting pad2 sets it to zero rather
    // than retaining the second controller's preceding value.
    pub fn set_input(&mut self, pad1: u8, pad2: Option<u8>) {
        self.inner.bus.set_controller_state(pad1, pad2.unwrap_or(0));
    }

    // Perform a side-effecting CPU-bus read at the current timestamp without
    // advancing clocks or requesting memory trace events.
    pub fn read_cpu(&mut self, addr: u16) -> u8 {
        self.inner.bus.read(
            addr,
            self.inner.frame,
            self.inner.cpu.cycles,
            TraceConfig::none(),
            &mut self.inner.trace,
        )
    }

    // Compatibility alias for read_cpu. This is not a side-effect-free debugger
    // peek: reads of status or controller registers can alter emulated state.
    pub fn peek_cpu(&mut self, addr: u16) -> u8 {
        self.read_cpu(addr)
    }

    // Perform a normal CPU-bus write at the current timestamp. Device side
    // effects still apply, including the bus's synchronous OAM-DMA path.
    pub fn write_cpu(&mut self, addr: u16, value: u8) {
        self.inner.bus.write(
            addr,
            value,
            self.inner.frame,
            self.inner.cpu.cycles,
            TraceConfig::none(),
            &mut self.inner.trace,
        );
    }

    // Return the core observation fingerprint over selected memory, pixels,
    // audio and mapper state. It excludes CPU registers and is not a complete
    // snapshot identity.
    pub fn state_hash(&self) -> String {
        self.inner.state_hash()
    }

    // Read the PPU's stored last pixel hash without rendering or refreshing it.
    pub fn pixel_hash(&self) -> u64 {
        self.inner.bus.ppu.last_pixel_hash
    }

    // Read the cumulative generated-sample counter, not the current buffer length.
    pub fn generated_audio_samples(&self) -> u64 {
        self.inner.bus.apu.generated_samples
    }

    // Read the core's accumulated DMC/controller conflict-candidate counter.
    pub fn dmc_joypad_conflicts(&self) -> u64 {
        self.inner.bus.apu.dmc_joypad_conflicts
    }

    #[getter]
    // Expose the software frame counter, which may differ from completed PPU frames.
    pub fn frame(&self) -> u64 {
        self.inner.frame
    }

    #[getter]
    // Expose the current 16-bit CPU program counter without reading the bus.
    pub fn pc(&self) -> u16 {
        self.inner.cpu.pc
    }

    #[getter]
    // Expose the CPU accumulator byte.
    pub fn a(&self) -> u8 {
        self.inner.cpu.a
    }

    #[getter]
    // Expose the CPU X index register byte.
    pub fn x(&self) -> u8 {
        self.inner.cpu.x
    }

    #[getter]
    // Expose the CPU Y index register byte.
    pub fn y(&self) -> u8 {
        self.inner.cpu.y
    }

    #[getter]
    // Expose the stack-pointer offset within CPU page one.
    pub fn sp(&self) -> u8 {
        self.inner.cpu.sp
    }

    #[getter]
    // Expose the stored CPU status byte without normalizing flag bits.
    pub fn status(&self) -> u8 {
        self.inner.cpu.p
    }

    #[pyo3(signature = (a=None, x=None, y=None, sp=None, status=None, pc=None))]
    // Assign only supplied register values and preserve omitted registers.
    // Assignments do not reset a stopped CPU, normalize status bits, execute
    // code or update timing counters.
    pub fn set_cpu_registers(
        &mut self,
        a: Option<u8>,
        x: Option<u8>,
        y: Option<u8>,
        sp: Option<u8>,
        status: Option<u8>,
        pc: Option<u16>,
    ) {
        if let Some(value) = a {
            self.inner.cpu.a = value;
        }
        if let Some(value) = x {
            self.inner.cpu.x = value;
        }
        if let Some(value) = y {
            self.inner.cpu.y = value;
        }
        if let Some(value) = sp {
            self.inner.cpu.sp = value;
        }
        if let Some(value) = status {
            self.inner.cpu.p = value;
        }
        if let Some(value) = pc {
            self.inner.cpu.pc = value;
        }
    }

    #[getter]
    // Expose the cumulative CPU cycle counter.
    pub fn cpu_cycles(&self) -> u64 {
        self.inner.cpu.cycles
    }

    // Run the requested frames with current input before serializing the report.
    // This changes emulator state and may stop execution; it is not a read-only
    // inspection of the previous diagnostics.
    pub fn diagnostics_json(&mut self, frames: u64) -> PyResult<String> {
        let summary = self.inner.run_current(RunOptions {
            frames,
            pad1: self.inner.bus.controller_state[0],
            pad2: self.inner.bus.controller_state[1],
            allow_unimplemented_opcode: true,
            ..RunOptions::default()
        });
        serde_json::to_string_pretty(&summary.diagnostics).map_err(to_py_err)
    }

    // Serialize the current v2 snapshot without running additional instructions.
    pub fn snapshot_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner.snapshot()).map_err(to_py_err)
    }

    // Serialize and directly replace the requested snapshot file. Parent
    // directories are not created and this export has no atomic backup wrapper.
    pub fn save_snapshot(&self, path: String) -> PyResult<()> {
        fs::write(
            path,
            serde_json::to_string_pretty(&self.inner.snapshot()).map_err(to_py_err)?,
        )
        .map_err(to_py_err)
    }

    // Compatibility alias for save_snapshot with identical file replacement behavior.
    pub fn save_snapshot_file(&self, path: String) -> PyResult<()> {
        self.save_snapshot(path)
    }

    // Write the current PPU frame buffer through the core PNG encoder without
    // running a frame or creating the parent directory.
    pub fn save_png(&mut self, path: String) -> PyResult<()> {
        screen::write_emulator_png(&mut self.inner, path).map_err(to_py_err)
    }

    // Serialize every retained trace event in order, one JSON value per line.
    pub fn trace_jsonl(&self) -> PyResult<String> {
        self.inner.trace.to_jsonl().map_err(to_py_err)
    }

    // Directly replace a file with the complete accumulated JSONL trace.
    pub fn save_trace_jsonl(&self, path: String) -> PyResult<()> {
        fs::write(path, self.inner.trace.to_jsonl().map_err(to_py_err)?).map_err(to_py_err)
    }

    // Run with diagnostic tracing and current input, then export aggregated
    // events from retained trace and the resulting cumulative report. Existing
    // trace is not cleared before collection.
    pub fn emit_diagnostics_jsonl(&mut self, path: String, frames: u64) -> PyResult<()> {
        let summary = self.inner.run_current(RunOptions {
            frames,
            pad1: self.inner.bus.controller_state[0],
            pad2: self.inner.bus.controller_state[1],
            trace: diagnostic_trace_config(true),
            allow_unimplemented_opcode: true,
            ..RunOptions::default()
        });
        let events =
            diagnostic_events_from_trace_and_report(&self.inner.trace, &summary.diagnostics);
        write_events_jsonl(&path, &events).map_err(to_py_err)
    }

    // Advance the requested frames and return aggregated diagnostic events as
    // JSON, retaining earlier trace. Polling changes state and can return an
    // event observed before this call.
    pub fn poll_diagnostics(&mut self, frames: u64) -> PyResult<String> {
        let summary = self.inner.run_current(RunOptions {
            frames,
            pad1: self.inner.bus.controller_state[0],
            pad2: self.inner.bus.controller_state[1],
            trace: diagnostic_trace_config(true),
            allow_unimplemented_opcode: true,
            ..RunOptions::default()
        });
        let events =
            diagnostic_events_from_trace_and_report(&self.inner.trace, &summary.diagnostics);
        serde_json::to_string_pretty(&events).map_err(to_py_err)
    }

    // Advance with all declared trace selectors and current input, then export
    // the resulting snapshot, summary, plans and retained events. The archive
    // contains the end state; no starting checkpoint or replay inputs are captured.
    pub fn save_repro_bundle(&mut self, path: String, frames: u64) -> PyResult<()> {
        let summary = self.inner.run_current(RunOptions {
            frames,
            pad1: self.inner.bus.controller_state[0],
            pad2: self.inner.bus.controller_state[1],
            trace: TraceConfig::full(),
            allow_unimplemented_opcode: true,
            ..RunOptions::default()
        });
        let events =
            diagnostic_events_from_trace_and_report(&self.inner.trace, &summary.diagnostics);
        write_repro_bundle(&path, &summary, &self.inner, &events).map_err(to_py_err)
    }
}

#[pyfunction]
// Load cartridge metadata and serialize it without executing the ROM.
// FDS inspection still follows cartridge loading and requires an external BIOS.
pub fn inspect_rom_json(path: String) -> PyResult<String> {
    let cart = Cartridge::load_file(&path).map_err(to_py_err)?;
    serde_json::to_string_pretty(&cart.info).map_err(to_py_err)
}

#[pyfunction]
// Serialize the registry descriptor for one mapper number without loading a ROM.
pub fn mapper_spec_json(mapper: u16) -> PyResult<String> {
    serde_json::to_string_pretty(&mapper_spec(mapper)).map_err(to_py_err)
}

#[pyfunction]
// Return all registry entries when requested; otherwise include both
// Implemented and Scaffold entries, not only accuracy-complete implementations.
pub fn mapper_list_json(all: bool) -> PyResult<String> {
    let specs = if all {
        all_mapper_specs()
    } else {
        implemented_mapper_specs()
    };
    serde_json::to_string_pretty(&specs).map_err(to_py_err)
}

#[pymodule]
// Register the emulator class and three module-level JSON inspection helpers.
fn kurosaki(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Emulator>()?;
    m.add_function(wrap_pyfunction!(inspect_rom_json, m)?)?;
    m.add_function(wrap_pyfunction!(mapper_spec_json, m)?)?;
    m.add_function(wrap_pyfunction!(mapper_list_json, m)?)?;
    Ok(())
}

// Convert any displayable core, serialization or I/O error into Python
// RuntimeError while retaining its formatted message.
fn to_py_err<E: std::fmt::Display>(err: E) -> PyErr {
    PyRuntimeError::new_err(err.to_string())
}

// Select PPU, APU, mapper, NMI and DMA events when enabled. CPU and generic
// memory events remain disabled; actual emission depends on each component.
fn diagnostic_trace_config(enabled: bool) -> TraceConfig {
    if enabled {
        TraceConfig {
            ppu: true,
            apu: true,
            mapper: true,
            nmi: true,
            dma: true,
            ..TraceConfig::none()
        }
    } else {
        TraceConfig::none()
    }
}

// Select CPU, memory writes, mapper, NMI and source annotations for a
// breakpoint run. Generic memory reads are not selected.
fn debugger_trace_config() -> TraceConfig {
    TraceConfig {
        cpu: true,
        mem_write: true,
        mapper: true,
        nmi: true,
        source: true,
        ..TraceConfig::none()
    }
}

// Treat ALL and * as unconditional matches; otherwise use an uppercase
// substring search on event types. The caller supplies the folded selector;
// an empty selector matches any event but not an empty event list.
fn diagnostic_matches(events: &[DiagnosticEvent], selector: &str) -> bool {
    selector == "ALL"
        || selector == "*"
        || events
            .iter()
            .any(|event| event.event_type.to_ascii_uppercase().contains(selector))
}

// Build the complete JSONL string with one newline per event and replace
// the target file. Empty input produces an empty file; errors propagate.
fn write_events_jsonl(
    path: &str,
    events: &[DiagnosticEvent],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut out = String::new();
    for event in events {
        out.push_str(&serde_json::to_string(event)?);
        out.push('\n');
    }
    fs::write(path, out)?;
    Ok(())
}

// Create a deflated ZIP of the supplied end-state artifacts and schema
// placeholder plans. The archive is written directly, so an error can leave
// a partial file. Plan records describe data only and execute no repair steps.
fn write_repro_bundle(
    path: &str,
    summary: &kurosaki_core::RunSummary,
    emu: &CoreEmulator,
    events: &[DiagnosticEvent],
) -> Result<(), Box<dyn std::error::Error>> {
    let file = fs::File::create(path)?;
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip_json(&mut zip, options, "run_summary.json", summary)?;
    zip_json(&mut zip, options, "snapshot.json", &emu.snapshot())?;
    zip_json(
        &mut zip,
        options,
        "ai_diagnostics.json",
        &summary.diagnostics,
    )?;
    zip_json(
        &mut zip,
        options,
        "diagnostic_summary.json",
        &serde_json::json!({
            "schema": "kurosaki-diagnostic-summary",
            "schema_version": 1,
            "rom_sha256": &summary.rom_sha256,
            "diagnostic_events": events.len(),
            "diagnostics": summary.diagnostics.items.len()
        }),
    )?;
    zip_json(
        &mut zip,
        options,
        "retest_plan.json",
        &serde_json::json!({ "schema": "kurosaki-retest-plan", "schema_version": 1 }),
    )?;
    zip_json(
        &mut zip,
        options,
        "repair_plan.json",
        &serde_json::json!({ "schema": "kurosaki-repair-plan", "schema_version": 1, "diagnostics": &summary.diagnostics.items }),
    )?;
    zip_json(
        &mut zip,
        options,
        "automation_plan.json",
        &serde_json::json!({ "schema": "kurosaki-automation-plan", "schema_version": 1 }),
    )?;
    let mut event_text = String::new();
    for event in events {
        event_text.push_str(&serde_json::to_string(event)?);
        event_text.push('\n');
    }
    zip_text(&mut zip, options, "diagnostics.jsonl", &event_text)?;
    zip_text(&mut zip, options, "trace.jsonl", &emu.trace.to_jsonl()?)?;
    zip_json(
        &mut zip,
        options,
        "manifest.json",
        &serde_json::json!({
            "schema": "sarakura-repro-bundle-manifest",
            "schema_version": 1,
            "producer": "kurosaki-py",
            "platform": "fc",
            "rom": { "hash": &summary.rom_sha256, "target": "nes" },
            "build_id": &summary.rom_sha256,
            "diagnostics_total": events.len(),
            "diagnostics": "ai_diagnostics.json",
            "diagnostic_summary": "diagnostic_summary.json",
            "retest_plan": "retest_plan.json",
            "repair_plan": "repair_plan.json",
            "automation_plan": "automation_plan.json"
        }),
    )?;
    zip.finish()?;
    Ok(())
}

// Start an archive member and write indented JSON without adding a newline.
fn zip_json<T: serde::Serialize>(
    zip: &mut ZipWriter<fs::File>,
    options: FileOptions,
    name: &str,
    value: &T,
) -> Result<(), Box<dyn std::error::Error>> {
    zip.start_file(name, options)?;
    zip.write_all(serde_json::to_string_pretty(value)?.as_bytes())?;
    Ok(())
}

// Start an archive member and write the supplied UTF-8 text bytes unchanged.
fn zip_text(
    zip: &mut ZipWriter<fs::File>,
    options: FileOptions,
    name: &str,
    text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    zip.start_file(name, options)?;
    zip.write_all(text.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build an original 16 KiB NROM fixture with NOP padding and all vectors
    // pointing to $8000. The supplied program must fit before the vector area.
    fn synthetic_nrom(program: &[u8]) -> Vec<u8> {
        let mut prg = vec![0xEA; 16 * 1024];
        prg[..program.len()].copy_from_slice(program);
        let end = prg.len();
        for offset in [end - 6, end - 4, end - 2] {
            prg[offset] = 0x00;
            prg[offset + 1] = 0x80;
        }
        let mut rom = vec![0x4E, 0x45, 0x53, 0x1A, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        rom.extend_from_slice(&prg);
        rom
    }

    // Initialize and reset the core around the synthetic NROM without file I/O.
    fn synthetic_emulator(program: &[u8]) -> Emulator {
        let cart = Cartridge::from_bytes(&synthetic_nrom(program)).expect("synthetic ROM parses");
        let mut inner = CoreEmulator::from_cartridge(cart).expect("synthetic NROM initializes");
        inner.reset(TraceConfig::none());
        Emulator { inner }
    }

    #[test]
    // Load an on-disk synthetic ROM, check its reset entry, then execute two
    // instructions and verify the expected RAM write before deleting the fixture.
    fn from_rom_starts_at_reset_vector() {
        let path =
            std::env::temp_dir().join(format!("kurosaki_py_reset_{}.nes", std::process::id()));
        fs::write(
            &path,
            synthetic_nrom(&[0xA9, 0x42, 0x85, 0x10, 0x4C, 0x04, 0x80]),
        )
        .unwrap();
        let mut emu = Emulator::from_rom(path.to_string_lossy().into_owned(), None).unwrap();
        assert_eq!(emu.pc(), 0x8000);
        emu.step_instructions(2).unwrap();
        assert_eq!(emu.peek_cpu(0x10), 0x42);
        fs::remove_file(path).unwrap();
    }

    #[test]
    // Check that stepping and both diagnostic helpers preserve both selected
    // controller masks across their frame runs.
    fn step_frames_keeps_the_input_selected_by_set_input() {
        let mut emu = synthetic_emulator(&[0x4C, 0x00, 0x80]);
        emu.set_input(0x89, Some(0x46));
        emu.step_frames(2).unwrap();
        assert_eq!(emu.inner.bus.controller_state, [0x89, 0x46]);
        emu.diagnostics_json(1).unwrap();
        assert_eq!(emu.inner.bus.controller_state, [0x89, 0x46]);
        emu.poll_diagnostics(1).unwrap();
        assert_eq!(emu.inner.bus.controller_state, [0x89, 0x46]);
    }

    #[test]
    // Set A/X/Y/PC through the wrapper and verify their getters while ensuring
    // an omitted SP retains its original value.
    fn cpu_registers_can_be_inspected_and_set_selectively() {
        let mut emulator = synthetic_emulator(&[0xEA]);
        let original_sp = emulator.sp();

        emulator.set_cpu_registers(Some(0x12), Some(0x34), Some(0x56), None, None, Some(0x8123));

        assert_eq!(emulator.a(), 0x12);
        assert_eq!(emulator.x(), 0x34);
        assert_eq!(emulator.y(), 0x56);
        assert_eq!(emulator.sp(), original_sp);
        assert_eq!(emulator.pc(), 0x8123);
    }

    #[test]
    // Execute a synthetic load/store sequence up to its loop PC, verify CPU/RAM
    // results and selected trace kinds, then check explicit trace clearing.
    fn traced_breakpoint_run_records_cpu_and_memory_write_events() {
        let mut emulator = synthetic_emulator(&[
            0xA9, 0x5A, // LDA #$5A
            0x8D, 0x02, 0x00, // STA $0002
            0x4C, 0x05, 0x80, // JMP $8005
        ]);

        assert!(emulator
            .run_until_pc_traced(0x8005, 8, true)
            .expect("synthetic program executes"));
        assert_eq!(emulator.a(), 0x5A);
        assert_eq!(emulator.read_cpu(0x0002), 0x5A);

        let trace = emulator.trace_jsonl().expect("trace serializes");
        assert!(trace.contains("\"kind\":\"cpu.instruction\""));
        assert!(trace.contains("\"kind\":\"mem.write\""));

        emulator.clear_trace();
        assert_eq!(emulator.trace_jsonl().unwrap(), "");
    }
}
