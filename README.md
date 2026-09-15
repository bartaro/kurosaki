# KUROSAKI

<!-- manual-language-links:start -->
| Language / 言語 | HTML |
| --- | --- |
| English | [KUROSAKI](https://bartaro.github.io/kitaq-docs/en/kurosaki.html) |
| 日本語 | [KUROSAKI](https://bartaro.github.io/kitaq-docs/kurosaki.html) |
<!-- manual-language-links:end -->


[English](#english) | [日本語](#japanese)

<a name="english"></a>

## English

NES/Famicom/FDS observation emulator with CLI, tracing and Python interface.

Public preview: APIs and behavior may change.

### Prebuilt Windows CLI

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
When redistributing, include `LICENSE`, `LICENSE.ja`, `BINARY_NOTICES.md` and the `licenses/` directory.
Third-party libraries retain their own copyright holders and terms.

### Build and first use

A current stable Rust toolchain. On Windows, install Visual Studio Build Tools with the Desktop development with C++ workload.

```powershell
.\scripts\build.ps1
.\kurosaki.exe --help
```

### Manuals and licenses

- [Japanese HTML manuals](https://bartaro.github.io/kitaq-docs/kurosaki.html) / [English manuals](https://bartaro.github.io/kitaq-docs/en/kurosaki.html)
- [Offline manual source](https://github.com/bartaro/kitaq-docs)
- [License](LICENSE) / [Japanese reference translation](LICENSE.ja)

The project license does not replace third-party dependency, logo or trademark terms. Preserve the accompanying notices when redistributing.

### Public package scope

This publication contains the emulator core, CLI and integration APIs. GUI frontends and the PLITA dependency are deferred and are not included in this repository snapshot.

### GUI development policy

KOKURA-GUI and KUROSAKI-GUI remain unpublished. Future GUI work uses PLITA;
SDL-GUI and egui-GUI are not planned for these projects. This repository
continues to distribute the core, CLI and integration APIs.

---

<a name="japanese"></a>

## 日本語

CLI、トレース、Pythonインターフェースを備えた、NES・ファミコン・FDSの動作観測用エミュレータです。

パブリックプレビュー版です。APIや動作は変更される場合があります。

### ビルド済みWindows CLI

リポジトリ直下に、Windows x64向けにReleaseモードでビルドした `kurosaki.exe` を同梱しています。コマンドラインで使うプログラムで、実行時にRust・Python・.NETは不要です。リポジトリのZIPを取得して、実行ファイルと権利表記をまとめて入手してください。ネイティブCランタイムは静的リンクされており、WindowsのシステムDLLを使用します。

```powershell
.\kurosaki.exe --help
```

このCLIだけを再ビルドするには `.\scripts\build.ps1` を実行します。必要に応じて `-Offline` を指定できます。スクリプトは実行ファイルをリポジトリ直下へコピーします。GUI、Python拡張モジュール、C APIのDLLは、この実行ファイルの配布には含めていません。

[バイナリのビルド記録](BINARY_BUILD.json)と[バイナリの依存物に関する権利表記](BINARY_NOTICES.md)を参照してください。配布時は `LICENSE`、`LICENSE.ja`、`BINARY_NOTICES.md` と `licenses/` フォルダーも同梱してください。

独自コードの著作権者・ライセンサーは **DAISUKE OBA** で、MITライセンスを適用しています。第三者のライブラリは、それぞれの著作権者と利用条件を保持します。

### ビルドと初回利用

再ビルドには現在の安定版Rustツールチェーンが必要です。Windowsでは、Visual Studio Build Toolsの「C++によるデスクトップ開発」ワークロードをインストールしてください。

```powershell
.\scripts\build.ps1
.\kurosaki.exe --help
```

### マニュアルとライセンス

- [日本語HTMLマニュアル](https://bartaro.github.io/kitaq-docs/kurosaki.html) / [英語HTMLマニュアル](https://bartaro.github.io/kitaq-docs/en/kurosaki.html)
- [オフライン用マニュアルのソース](https://github.com/bartaro/kitaq-docs)
- [ライセンス英語原文](LICENSE) / [日本語参考訳](LICENSE.ja)

プロジェクトのライセンスは、第三者の依存ライブラリ、ロゴ、商標に関する条件を置き換えるものではありません。再配布時は付属の権利表記も保持してください。

### 公開パッケージの範囲

今回の公開にはエミュレータのコア、CLI、連携APIを含めています。GUIと依存先のPLITAは公開を延期しており、このリポジトリの公開版には含めていません。

### GUIの開発方針

KOKURA-GUIとKUROSAKI-GUIは未公開です。今後のGUI開発にはPLITAを使用し、SDL-GUIとegui-GUIは使用しません。このリポジトリでは引き続きコア、CLI、連携APIを配布します。
