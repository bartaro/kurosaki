# KUROSAKI

[English](README.md#english) | [日本語](README.md#japanese) | **繁體中文**

**開啟 KUROSAKI 繁體中文手冊**

NES／Famicom／FDS 模擬器，可透過命令列工具、執行追蹤與 Python 介面觀察程式的運作。

本專案目前為公開預覽版，API 與行為仍可能調整。

## Windows 執行檔

儲存庫根目錄附有 Windows x64 Release 版 `kurosaki.exe`。這是命令列程式，執行時不必另外安裝 Rust、Python 或 .NET。建議下載整個儲存庫的 ZIP，一併取得執行檔與授權聲明。原生 C 執行階段採靜態連結，程式也會使用 Windows 系統 DLL。

```powershell
.\kurosaki.exe --help
```

若只要重新建置命令列工具，請執行 `.\scripts\build.ps1`；需要離線建置時可加上 `-Offline`。腳本會將執行檔複製到儲存庫根目錄。這次的執行檔發行不包含圖形介面、Python 擴充模組或 C API DLL。相關資訊請見[二進位檔建置紀錄](BINARY_BUILD.json)與[二進位檔相依套件授權聲明](BINARY_NOTICES.md)。

專案自行撰寫程式碼的授權人為 **DAISUKE OBA**，採用 MIT 授權條款。再散布時，請保留 `LICENSE`、`LICENSE.ja`、`BINARY_NOTICES.md` 與 `licenses/` 目錄。第三方程式庫仍須依各權利人的授權條件使用。

## 從原始碼建置與開始使用

重新建置需要近期穩定版 Rust 工具鏈。Windows 環境還須安裝 Visual Studio Build Tools，並選取「使用 C++ 的桌面開發」工作負載。

```powershell
.\scripts\build.ps1
.\kurosaki.exe --help
```

## 手冊與授權

- 繁體中文手冊
- [英文手冊](https://bartaro.github.io/kitaq-docs/en/kurosaki.html)／[日文手冊](https://bartaro.github.io/kitaq-docs/kurosaki.html)
- [可供離線閱讀的手冊原始檔](https://github.com/bartaro/kitaq-docs)
- [授權條款](LICENSE)／[日文參考譯文](LICENSE.ja)

本專案的授權不取代第三方對相依套件、標誌或商標訂定的條件。再散布時，請一併保留隨附聲明。

## 本次公開內容

本次提供模擬器核心、命令列工具及整合 API。圖形介面與 PLITA 相依項目預計日後另行公開，不包含在這份原始碼中。

## 圖形介面開發方針

KOKURA-GUI 與 KUROSAKI-GUI 目前尚未公開。未來的圖形介面開發使用 PLITA，不使用 SDL-GUI 或 egui-GUI。本儲存庫持續提供核心、命令列工具及整合 API。
