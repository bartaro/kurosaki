# KUROSAKI

NES/Famicom/FDS observation emulator with CLI, tracing and Python interface.

Public preview: APIs and behavior may change.

## Prebuilt Windows CLI

The repository root includes `kurosaki.exe`, built for Windows x64 in Release mode.
It is a command-line program. Rust, Python and .NET are not required to run it.
Download the repository ZIP to keep the executable and license notices together.
The native C runtime is linked statically; the program uses Windows system DLLs.

```powershell
.\kurosaki.exe --help
```

Rebuild only this CLI with `.\scripts\build.ps1` (optionally `-Offline`).
The script copies the executable to the repository root. GUI frontends, Python
extension modules and C API DLLs are not part of this executable distribution.
See [binary build record](BINARY_BUILD.json) and [binary dependency notices](BINARY_NOTICES.md).

The licensor of the independent project code is **DAISUKE OBA**, under MIT.
Third-party libraries retain their own copyright holders and terms.

日本語: ビルド済みのCLI版 `kurosaki.exe` をリポジトリ直下に同梱しています。
Windows x64向けです。実行時にRust・Python・.NETは不要です。
独自コードの著作権者・ライセンサーは **DAISUKE OBA** です。
配布時はLICENSE・LICENSE.ja・BINARY_NOTICES.mdとlicensesフォルダーも同梱してください。
GUI版は今回の配布に含みません。

## Build and first use

A current stable Rust toolchain. On Windows, install Visual Studio Build Tools with the Desktop development with C++ workload.

```powershell
.\scripts\build.ps1
.\kurosaki.exe --help
```

## Manuals and licenses

- [Japanese HTML manuals](https://bartaro.github.io/kitaq-docs/) / [English manuals](https://bartaro.github.io/kitaq-docs/en/)
- [Offline manual source](https://github.com/bartaro/kitaq-docs)
- [License](LICENSE) / [日本語参考訳](LICENSE.ja)

The project license does not replace third-party dependency, logo or trademark terms. Preserve the accompanying notices when redistributing.

## Public package scope

This publication contains the emulator core, CLI and integration APIs. GUI frontends and the PLITA dependency are deferred and are not included in this repository snapshot.

## GUI development policy

KOKURA-GUI and KUROSAKI-GUI remain unpublished. Future GUI work uses PLITA;
SDL-GUI and egui-GUI are not planned for these projects. This repository
continues to distribute the core, CLI and integration APIs.

GUIは未公開です。GUI開発にはPLITAを使用し、SDL-GUIとegui-GUIは使用しません。
