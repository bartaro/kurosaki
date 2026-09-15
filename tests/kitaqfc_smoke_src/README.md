# KITAQFC NROM smoke source

<!-- manual-language-links:start -->
| Language / 言語 | HTML |
| --- | --- |
| English | [KUROSAKI](https://bartaro.github.io/kitaq-docs/en/kurosaki.html) · [KITAQFC](https://bartaro.github.io/kitaq-docs/en/kitaqfc.html) |
| 日本語 | [KUROSAKI](https://bartaro.github.io/kitaq-docs/kurosaki.html) · [KITAQFC](https://bartaro.github.io/kitaq-docs/kitaqfc.html) |
<!-- manual-language-links:end -->

[English](README.md) | [日本語](README.ja.md)


This source is intended to be built later with the current KITAQFC Phase26 tree
and then copied into `crates/kurosaki-core/tests/fixtures/` as a compiler-produced
golden ROM.

The current fixture `kitaqfc_nrom_smoke.nes` is a clean-room byte-generated ROM
that exercises the same emulator paths without requiring KITAQFC or hardware in
this environment.

Suggested future command, adjusted to the local KITAQFC CLI flags as needed:

```powershell
kitaqfc.exe tests\kitaqfc_smoke_src\kitaqfc_nrom_smoke.c `
  -o crates\kurosaki-core\tests\fixtures\kitaqfc_nrom_smoke_from_kitaqfc.nes `
  --mapper nrom256 `
  --debug-out crates\kurosaki-core\tests\fixtures\debug_output
```
