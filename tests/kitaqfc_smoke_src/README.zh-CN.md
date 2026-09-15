# KITAQFC NROM基本功能测试源码

[English](README.md) | [日本語](README.ja.md) | **简体中文**

KUROSAKI简体中文手册 · KITAQFC简体中文手册

这份源码计划在后续使用KITAQFC Phase26开发树构建，再将编译器生成的ROM复制到 `crates/kurosaki-core/tests/fixtures/`，作为测试基准。这并不表示该源码已经完成构建验证。

当前测试文件 `kitaqfc_nrom_smoke.nes` 是独立生成字节序列得到的ROM，没有使用商业游戏代码。它用于在本环境不具备KITAQFC或实机的情况下，检查相同的模拟器处理路径。

以下是供后续构建参考的命令草案。执行前请根据本地KITAQFC版本调整CLI选项；它不是已验证可直接用于当前CLI的操作步骤。

```powershell
kitaqfc.exe tests\kitaqfc_smoke_src\kitaqfc_nrom_smoke.c `
  -o crates\kurosaki-core\tests\fixtures\kitaqfc_nrom_smoke_from_kitaqfc.nes `
  --mapper nrom256 `
  --debug-out crates\kurosaki-core\tests\fixtures\debug_output
```
