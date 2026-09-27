# KITAQFC NROM smoke source

<!-- manual-language-links:start -->
| Language / 言語 | HTML |
| --- | --- |
| English | [KUROSAKI](https://bartaro.github.io/kitaq-docs/en/kurosaki.html) |
| 日本語 | [KUROSAKI](https://bartaro.github.io/kitaq-docs/kurosaki.html) |
| 한국어 | [KUROSAKI](https://bartaro.github.io/kitaq-docs/ko/kurosaki.html) |
| 简体中文 | [KUROSAKI](https://bartaro.github.io/kitaq-docs/zh-CN/kurosaki.html) |
| 繁體中文 | [KUROSAKI](https://bartaro.github.io/kitaq-docs/zh-TW/kurosaki.html) |
| Français | [KUROSAKI](https://bartaro.github.io/kitaq-docs/fr/kurosaki.html) |
| Español | [KUROSAKI](https://bartaro.github.io/kitaq-docs/es/kurosaki.html) |
| Deutsch | [KUROSAKI](https://bartaro.github.io/kitaq-docs/de/kurosaki.html) |
<!-- manual-language-links:end -->

<!-- readme-language-links:start -->
**English** | [日本語](README.ja.md) | [한국어](README.ko.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | [Français](README.fr.md) | [Español](README.es.md) | [Deutsch](README.de.md)
<!-- readme-language-links:end -->


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
