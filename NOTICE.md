# KUROSAKI Notice

KUROSAKI is intended to be released under the MIT License.

The current Windows dependency inventory is in
[`THIRD_PARTY_NOTICES.windows.md`](THIRD_PARTY_NOTICES.windows.md).
The PLITA GUI also uses third-party icon data and may load bundled OFL fonts.
Preserve PLITA's font and Lucide/Feather notices when shipping those components;
the KUROSAKI MIT declaration does not relicense them.

## Clean-room policy

This source tree is a clean-room implementation scaffold written for the KITAQFC
project. It does not copy source code from Mesen, FCEUX, Nestopia, Nintendulator,
or any other NES emulator implementation.

Implementation rules for future contributors:

- Do not paste or mechanically translate emulator source code from another project.
- Use public hardware documentation, original tests, and behavior observations only.
- Keep provenance notes for non-trivial timing, mapper, and APU/PPU behavior decisions.
- Prefer small regression ROMs and KITAQFC-generated smoke ROMs over copying existing emulator internals.
- Keep third-party dependencies limited to general-purpose Rust crates with compatible licenses.

## Current third-party Rust crates

- clap: CLI argument parsing
- serde / serde_json: machine-readable reports
- sha2: ROM and state hashing
- thiserror / anyhow: error handling
- pyo3: Python bindings skeleton

These crates are used as ordinary infrastructure dependencies, not as emulator implementation sources.
