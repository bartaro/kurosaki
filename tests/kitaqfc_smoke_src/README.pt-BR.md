# Fonte do teste básico NROM para KITAQFC

[English](README.md) | [日本語](README.ja.md) | **Português (Brasil)**

Manual do KUROSAKI em português · Manual da KITAQFC em português

Este fonte foi preparado para uma compilação futura com a árvore de desenvolvimento Phase26 da KITAQFC. A ROM gerada pelo compilador deverá então ser copiada para `crates/kurosaki-core/tests/fixtures/` e usada como referência nos testes. Isso ainda não constitui uma compilação verificada deste fonte.

A ROM de teste atual, `kitaqfc_nrom_smoke.nes`, foi gerada diretamente como uma sequência original de bytes, sem reutilizar código de jogos comerciais. Ela exercita os mesmos caminhos do emulador sem exigir KITAQFC ou hardware físico neste ambiente.

O comando abaixo é uma proposta para essa etapa futura. Antes de executá-lo, adapte as opções à versão local da CLI da KITAQFC; ele não foi validado como uma sequência pronta para a CLI atual.

```powershell
kitaqfc.exe tests\kitaqfc_smoke_src\kitaqfc_nrom_smoke.c `
  -o crates\kurosaki-core\tests\fixtures\kitaqfc_nrom_smoke_from_kitaqfc.nes `
  --mapper nrom256 `
  --debug-out crates\kurosaki-core\tests\fixtures\debug_output
```
