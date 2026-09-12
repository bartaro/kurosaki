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
pub struct Emulator {
    inner: CoreEmulator,
}

#[pymethods]
impl Emulator {
    #[staticmethod]
    #[pyo3(signature = (path, battery_path=None))]
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
    pub fn from_snapshot(rom_path: String, snapshot_path: String) -> PyResult<Self> {
        let cart = Cartridge::load_file(&rom_path).map_err(to_py_err)?;
        let snapshot: kurosaki_core::Snapshot =
            serde_json::from_str(&fs::read_to_string(snapshot_path).map_err(to_py_err)?)
                .map_err(to_py_err)?;
        let inner = CoreEmulator::from_snapshot(cart, &snapshot).map_err(to_py_err)?;
        Ok(Self { inner })
    }

    pub fn load_snapshot(&mut self, path: String) -> PyResult<()> {
        let snapshot: kurosaki_core::Snapshot =
            serde_json::from_str(&fs::read_to_string(path).map_err(to_py_err)?)
                .map_err(to_py_err)?;
        self.inner.restore_snapshot(&snapshot).map_err(to_py_err)
    }

    pub fn reset(&mut self) {
        self.inner.reset(TraceConfig::none());
    }

    /// Explicit sidecar I/O; analysis sessions never auto-overwrite user saves.
    pub fn load_battery_file(&mut self, path: String) -> PyResult<()> {
        self.inner.load_battery_file(path).map_err(to_py_err)
    }

    pub fn save_battery_file(&self, path: String) -> PyResult<()> {
        self.inner.save_battery_file(path).map_err(to_py_err)
    }

    pub fn battery_ram(&self) -> PyResult<Option<Vec<u8>>> {
        self.inner.battery_ram().map_err(to_py_err)
    }

    pub fn step_frame(&mut self) -> PyResult<()> {
        self.inner
            .step_frame(TraceConfig::none(), true)
            .map_err(to_py_err)
    }

    pub fn step_instruction(&mut self) -> PyResult<()> {
        self.inner
            .step_instruction(TraceConfig::none(), true)
            .map_err(to_py_err)
    }

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

    pub fn clear_trace(&mut self) {
        self.inner.trace = Default::default();
    }

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

    pub fn run_until_diagnostic(&mut self, event_type: String, max_frames: u64) -> PyResult<bool> {
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
    pub fn set_input(&mut self, pad1: u8, pad2: Option<u8>) {
        self.inner.bus.set_controller_state(pad1, pad2.unwrap_or(0));
    }

    pub fn read_cpu(&mut self, addr: u16) -> u8 {
        self.inner.bus.read(
            addr,
            self.inner.frame,
            self.inner.cpu.cycles,
            TraceConfig::none(),
            &mut self.inner.trace,
        )
    }

    pub fn peek_cpu(&mut self, addr: u16) -> u8 {
        self.read_cpu(addr)
    }

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

    pub fn state_hash(&self) -> String {
        self.inner.state_hash()
    }

    pub fn pixel_hash(&self) -> u64 {
        self.inner.bus.ppu.last_pixel_hash
    }

    pub fn generated_audio_samples(&self) -> u64 {
        self.inner.bus.apu.generated_samples
    }

    pub fn dmc_joypad_conflicts(&self) -> u64 {
        self.inner.bus.apu.dmc_joypad_conflicts
    }

    #[getter]
    pub fn frame(&self) -> u64 {
        self.inner.frame
    }

    #[getter]
    pub fn pc(&self) -> u16 {
        self.inner.cpu.pc
    }

    #[getter]
    pub fn a(&self) -> u8 {
        self.inner.cpu.a
    }

    #[getter]
    pub fn x(&self) -> u8 {
        self.inner.cpu.x
    }

    #[getter]
    pub fn y(&self) -> u8 {
        self.inner.cpu.y
    }

    #[getter]
    pub fn sp(&self) -> u8 {
        self.inner.cpu.sp
    }

    #[getter]
    pub fn status(&self) -> u8 {
        self.inner.cpu.p
    }

    #[pyo3(signature = (a=None, x=None, y=None, sp=None, status=None, pc=None))]
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
    pub fn cpu_cycles(&self) -> u64 {
        self.inner.cpu.cycles
    }

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

    pub fn snapshot_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner.snapshot()).map_err(to_py_err)
    }

    pub fn save_snapshot(&self, path: String) -> PyResult<()> {
        fs::write(
            path,
            serde_json::to_string_pretty(&self.inner.snapshot()).map_err(to_py_err)?,
        )
        .map_err(to_py_err)
    }

    pub fn save_snapshot_file(&self, path: String) -> PyResult<()> {
        self.save_snapshot(path)
    }

    pub fn save_png(&mut self, path: String) -> PyResult<()> {
        screen::write_emulator_png(&mut self.inner, path).map_err(to_py_err)
    }

    pub fn trace_jsonl(&self) -> PyResult<String> {
        self.inner.trace.to_jsonl().map_err(to_py_err)
    }

    pub fn save_trace_jsonl(&self, path: String) -> PyResult<()> {
        fs::write(path, self.inner.trace.to_jsonl().map_err(to_py_err)?).map_err(to_py_err)
    }

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
pub fn inspect_rom_json(path: String) -> PyResult<String> {
    let cart = Cartridge::load_file(&path).map_err(to_py_err)?;
    serde_json::to_string_pretty(&cart.info).map_err(to_py_err)
}

#[pyfunction]
pub fn mapper_spec_json(mapper: u16) -> PyResult<String> {
    serde_json::to_string_pretty(&mapper_spec(mapper)).map_err(to_py_err)
}

#[pyfunction]
pub fn mapper_list_json(all: bool) -> PyResult<String> {
    let specs = if all {
        all_mapper_specs()
    } else {
        implemented_mapper_specs()
    };
    serde_json::to_string_pretty(&specs).map_err(to_py_err)
}

#[pymodule]
fn kurosaki(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Emulator>()?;
    m.add_function(wrap_pyfunction!(inspect_rom_json, m)?)?;
    m.add_function(wrap_pyfunction!(mapper_spec_json, m)?)?;
    m.add_function(wrap_pyfunction!(mapper_list_json, m)?)?;
    Ok(())
}

fn to_py_err<E: std::fmt::Display>(err: E) -> PyErr {
    PyRuntimeError::new_err(err.to_string())
}

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

fn diagnostic_matches(events: &[DiagnosticEvent], selector: &str) -> bool {
    selector == "ALL"
        || selector == "*"
        || events
            .iter()
            .any(|event| event.event_type.to_ascii_uppercase().contains(selector))
}

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

    fn synthetic_emulator(program: &[u8]) -> Emulator {
        let cart = Cartridge::from_bytes(&synthetic_nrom(program)).expect("synthetic ROM parses");
        let mut inner = CoreEmulator::from_cartridge(cart).expect("synthetic NROM initializes");
        inner.reset(TraceConfig::none());
        Emulator { inner }
    }

    #[test]
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
