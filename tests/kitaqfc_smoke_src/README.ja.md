# KITAQFC NROMの基本動作確認用ソース

[English](README.md) | [日本語](README.ja.md) | [Français](README.fr.md) | [Deutsch](README.de.md)

[日本語のKUROSAKI説明書](https://bartaro.github.io/kitaq-docs/kurosaki.html) · [日本語のKITAQFC説明書](https://bartaro.github.io/kitaq-docs/kitaqfc.html)

このソースは、KITAQFCのPhase26開発ツリーで後日ビルドし、コンパイラが生成した基準ROMとして `crates/kurosaki-core/tests/fixtures/` にコピーすることを想定したものです。そのビルドが完了していることを示すものではありません。

現在の `kitaqfc_nrom_smoke.nes` は、市販ゲームのコードを使わずにバイト列を独自に生成したテストROMです。この環境にKITAQFCや実機がなくても、同じエミュレーター処理を確認するために用意しています。

以下は今後のビルドを想定したコマンド案です。実行前に、使用するKITAQFCのCLIオプションに合わせて調整してください。現在のCLIでそのまま実行できると確認済みの手順ではありません。

```powershell
kitaqfc.exe tests\kitaqfc_smoke_src\kitaqfc_nrom_smoke.c `
  -o crates\kurosaki-core\tests\fixtures\kitaqfc_nrom_smoke_from_kitaqfc.nes `
  --mapper nrom256 `
  --debug-out crates\kurosaki-core\tests\fixtures\debug_output
```
