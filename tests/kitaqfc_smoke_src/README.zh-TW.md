# KITAQFC NROM 基本功能測試原始碼

[English](README.md) | [日本語](README.ja.md) | **繁體中文**

[KUROSAKI 繁體中文手冊](https://bartaro.github.io/kitaq-docs/zh-TW/kurosaki.html) · [KITAQFC 繁體中文手冊](https://bartaro.github.io/kitaq-docs/zh-TW/kitaqfc.html)

這份原始碼預定日後使用 KITAQFC Phase26 開發樹建置，再將編譯器產生的 ROM 複製到 `crates/kurosaki-core/tests/fixtures/`，作為測試基準。目前尚不能視為已完成建置驗證的範例。

現有的 `kitaqfc_nrom_smoke.nes` 測試 ROM 是以獨立產生的位元組序列建立，沒有使用商業遊戲程式碼。即使本環境沒有 KITAQFC 或實機，也能藉此檢查相同的模擬器處理路徑。

以下指令草案供日後建置時參考。執行前須依本機的 KITAQFC 版本調整 CLI 選項；這不是已驗證可直接套用至目前 CLI 的操作步驟。

```powershell
kitaqfc.exe tests\kitaqfc_smoke_src\kitaqfc_nrom_smoke.c `
  -o crates\kurosaki-core\tests\fixtures\kitaqfc_nrom_smoke_from_kitaqfc.nes `
  --mapper nrom256 `
  --debug-out crates\kurosaki-core\tests\fixtures\debug_output
```
