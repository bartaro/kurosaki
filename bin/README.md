# KUROSAKI native CLI


<!-- native-platform-binaries-20261004-en -->
### Prebuilt Linux and macOS CLI

Native CLI files verified in GitHub Actions are available in the folders below. Rust, Python and .NET are not required to run the CLI. The Linux build targets x86_64/glibc; choose the matching CPU for macOS.

| OS / CPU | CLI |
| --- | --- |
| Linux x86_64 (glibc) | [bin/linux-x86_64/kurosaki](../bin/linux-x86_64/kurosaki) |
| macOS ARM64 | [bin/macos-arm64/kurosaki](../bin/macos-arm64/kurosaki) |
| macOS Intel | [bin/macos-x86_64/kurosaki](../bin/macos-x86_64/kurosaki) |

```sh
chmod +x bin/linux-x86_64/kurosaki
./bin/linux-x86_64/kurosaki --help

chmod +x bin/macos-arm64/kurosaki
./bin/macos-arm64/kurosaki --help

chmod +x bin/macos-x86_64/kurosaki
./bin/macos-x86_64/kurosaki --help
```

Run the commands from the repository root. For redistribution, retain LICENSE, LICENSE.ja, BINARY_NOTICES.md and licenses/. NATIVE_BINARIES.json records hashes, dependencies, source revisions and native execution checks.


<!-- native-platform-binaries-20261004-ja -->
### ビルド済みLinux・macOS CLI

GitHub Actionsで実行検証したCLIを下記フォルダに配置しています。CLIの実行にRust・Python・.NETは不要です。Linux版はx86_64/glibc向けです。macOSではCPUに合う版を選んでください。

| OS / CPU | CLI |
| --- | --- |
| Linux x86_64 (glibc) | [bin/linux-x86_64/kurosaki](../bin/linux-x86_64/kurosaki) |
| macOS ARM64 | [bin/macos-arm64/kurosaki](../bin/macos-arm64/kurosaki) |
| macOS Intel | [bin/macos-x86_64/kurosaki](../bin/macos-x86_64/kurosaki) |

```sh
chmod +x bin/linux-x86_64/kurosaki
./bin/linux-x86_64/kurosaki --help

chmod +x bin/macos-arm64/kurosaki
./bin/macos-arm64/kurosaki --help

chmod +x bin/macos-x86_64/kurosaki
./bin/macos-x86_64/kurosaki --help
```

コマンドはリポジトリ直下で実行します。再配布時はLICENSE、LICENSE.ja、BINARY_NOTICES.md、licenses/も保持してください。NATIVE_BINARIES.jsonにハッシュ、依存物、ソースのリビジョン、ネイティブ実行検証の記録があります。

Linux: x86_64/glibc, GLIBC_2.34 symbol requirements. Native execution was verified on the platform recorded in NATIVE_BINARIES.json.

macOS deployment targets: ARM64 11.0, Intel 10.12; execution was verified on the newer native CI hosts recorded in NATIVE_BINARIES.json.

Rebuild the CLI only with `cargo build --locked --release -p kurosaki-cli`; Python extension build configuration is separate.
