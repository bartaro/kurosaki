# Publication checks

Source snapshot: 2026-09-12. Windows x86_64 checks against this publication tree.

cargo test --workspace --offline passed: 102 tests. The DMA smoke test constructs an original synthetic program in memory and requires no ROM fixture.

GUI frontends and PLITA are deferred. Build products, private inputs and generated ROMs are excluded from Git. This packaging pass does not claim physical-hardware validation.

## Windows CLI executable distribution - 2026-09-13

A fresh CLI-only Release executable is included at the repository root.
The build uses the locked dependencies, x86_64-pc-windows-msvc and static CRT.
Current binary hashes and executed checks are recorded in `BINARY_BUILD.json`.
These checks are separate from the earlier workspace test totals.
The independent code is licensed by DAISUKE OBA. Original dependency notices
are bundled; GUI executables and private ROMs/data are not distributed.
