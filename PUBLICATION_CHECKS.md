# Publication checks

Workspace test record: September 12, 2026, on Windows x86_64.
The executable build record is identified separately in `BINARY_BUILD.json`.

cargo test --workspace --offline passed: 102 tests. The DMA smoke test constructs an original synthetic program in memory and requires no ROM fixture.

Build caches, private inputs and generated ROMs are excluded from Git. These checks do not cover physical hardware.

## Windows CLI executable distribution - 2026-09-13

A CLI-only Release executable is included at the repository root.
The build uses the locked dependencies, x86_64-pc-windows-msvc and static CRT.
Current binary hashes and executed checks are recorded in `BINARY_BUILD.json`.
Each test record applies to its recorded source and executable hashes.
The independent code is licensed by DAISUKE OBA. Original dependency notices
are bundled; GUI executables and private ROMs/data are not distributed.
