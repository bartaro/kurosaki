# KUROSAKI

<!-- readme-language-links:start -->
[English](README.md#english) | [日本語](README.md#japanese) | [한국어](README.ko.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | [Français](README.fr.md) | [Español](README.es.md) | **Deutsch**
<!-- readme-language-links:end -->

**[KUROSAKI · HTML-Handbuch](https://bartaro.github.io/kitaq-docs/de/kurosaki.html)**

NES-/Famicom-/FDS-Emulator zur Untersuchung von Programmläufen, mit Kommandozeile, Ablaufprotokollen und Python-Schnittstelle.

Öffentliche Vorabversion: APIs und Verhalten können sich noch ändern.

## Fertige Windows-Version

Im Stammverzeichnis des Repositorys liegt `kurosaki.exe`, als Release für Windows x64 kompiliert. Das Programm wird über die Kommandozeile bedient. Zum Ausführen benötigen Sie weder Rust noch Python oder .NET. Laden Sie das Repository als ZIP herunter, damit die ausführbare Datei und die Lizenzhinweise zusammenbleiben. Die native C-Laufzeitbibliothek ist statisch eingebunden; das Programm verwendet System-DLLs von Windows.

```powershell
.\kurosaki.exe --help
```

Mit `.\scripts\build.ps1` bauen Sie ausschließlich dieses Kommandozeilenprogramm neu; bei Bedarf ergänzen Sie `-Offline`. Das Skript kopiert die ausführbare Datei ins Stammverzeichnis. Grafische Oberflächen, Python-Erweiterungsmodule und DLLs der C-API gehören nicht zu diesem Paket ausführbarer Programme. Einzelheiten finden Sie im [Build-Nachweis](BINARY_BUILD.json) und in den [Lizenzhinweisen zu den Binärabhängigkeiten](BINARY_NOTICES.md).

Lizenzgeber des eigenständigen Projektcodes ist **DAISUKE OBA**. Dieser Code steht unter der MIT-Lizenz. Geben Sie bei einer Weiterverteilung `LICENSE`, `LICENSE.ja`, `BINARY_NOTICES.md` und das Verzeichnis `licenses/` mit. Für Bibliotheken Dritter gelten deren jeweilige Rechteinhaber und Lizenzbedingungen.

## Selbst kompilieren und starten

Verwenden Sie eine aktuelle stabile Rust-Toolchain. Installieren Sie unter Windows außerdem Visual Studio Build Tools mit dem Workload für die Desktopentwicklung mit C++.

```powershell
.\scripts\build.ps1
.\kurosaki.exe --help
```

<!-- current-nametable-mapping -->
## Nametable-Zuordnung

AxROM unterstützt die Auswahl der unteren oder oberen Seite im Einbildschirmmodus. FME7 unterstützt vertikale und horizontale Spiegelung sowie beide Einbildschirmmodi. MMC5 unterstützt alle sechzehn Anordnungen mit den beiden CIRAM-Seiten; ExRAM- und Fülldarstellung werden nicht unterstützt. [Modustabelle und ausgeführte Beispiele](https://bartaro.github.io/kitaq-docs/de/kitaqfc.html#api-__mirroring_set).

## Handbücher und Lizenzen

- [KUROSAKI · HTML-Handbuch](https://bartaro.github.io/kitaq-docs/de/kurosaki.html)
- [Englisches Handbuch](https://bartaro.github.io/kitaq-docs/en/kurosaki.html) / [Japanisches Handbuch](https://bartaro.github.io/kitaq-docs/kurosaki.html)
- [Handbuchquellen zum Offline-Lesen](https://github.com/bartaro/kitaq-docs)
- [Lizenz](LICENSE) / [Japanische Übersetzung zur Orientierung](LICENSE.ja)

Die Projektlizenz ersetzt keine Bedingungen Dritter für Abhängigkeiten, Logos oder Marken. Behalten Sie bei einer Weitergabe die beigefügten Hinweise bei.


<!-- native-platform-binaries-20261004-de -->
### Vorgefertigte CLI für Linux und macOS

Die in GitHub Actions durch Ausführung geprüften CLI-Dateien befinden sich in den folgenden Ordnern. Für ihre Ausführung sind weder Rust noch Python noch .NET erforderlich. Die Linux-Version ist für x86_64/glibc vorgesehen; wählen Sie unter macOS die passende CPU-Version.

| OS / CPU | CLI |
| --- | --- |
| Linux x86_64 (glibc) | [bin/linux-x86_64/kurosaki](bin/linux-x86_64/kurosaki) |
| macOS ARM64 | [bin/macos-arm64/kurosaki](bin/macos-arm64/kurosaki) |
| macOS Intel | [bin/macos-x86_64/kurosaki](bin/macos-x86_64/kurosaki) |

```sh
chmod +x bin/linux-x86_64/kurosaki
./bin/linux-x86_64/kurosaki --help

chmod +x bin/macos-arm64/kurosaki
./bin/macos-arm64/kurosaki --help

chmod +x bin/macos-x86_64/kurosaki
./bin/macos-x86_64/kurosaki --help
```

Führen Sie die Befehle im Stammverzeichnis des Repositorys aus. Bewahren Sie bei der Weitergabe LICENSE, LICENSE.ja, BINARY_NOTICES.md und licenses/ auf. NATIVE_BINARIES.json enthält Prüfsummen, Abhängigkeiten, Quellcoderevisionen und die Ergebnisse der nativen Ausführungsprüfungen.
