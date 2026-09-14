# KITAQFC-NROM-Quellcode für einen Basistest

[English](README.md) | **Deutsch**

**[KUROSAKI-Handbuch](https://bartaro.github.io/kitaq-docs/de/kurosaki.html)** · **[KITAQFC-Handbuch](https://bartaro.github.io/kitaq-docs/de/kitaqfc.html)**

Dieser Quellcode ist dafür vorgesehen, später mit dem KITAQFC-Quellenstand Phase26 kompiliert und anschließend als vom Compiler erzeugte Referenz-ROM nach `crates/kurosaki-core/tests/fixtures/` kopiert zu werden.

Die derzeitige Testdatei `kitaqfc_nrom_smoke.nes` wurde unabhängig als Bytefolge erzeugt. Sie prüft dieselben Emulatorpfade, ohne in dieser Umgebung KITAQFC oder reale Hardware vorauszusetzen.

Der folgende Befehl ist ein Vorschlag für diesen späteren Schritt. Passen Sie die Optionen vor der Verwendung an die lokale KITAQFC-Version an; er ist kein Nachweis eines bereits ausgeführten Builds:

```powershell
kitaqfc.exe tests\kitaqfc_smoke_src\kitaqfc_nrom_smoke.c `
  -o crates\kurosaki-core\tests\fixtures\kitaqfc_nrom_smoke_from_kitaqfc.nes `
  --mapper nrom256 `
  --debug-out crates\kurosaki-core\tests\fixtures\debug_output
```
