# KUROSAKI

[English](README.md#english) | [日本語](README.md#japanese) | **简体中文**

**[打开KUROSAKI简体中文手册](https://bartaro.github.io/kitaq-docs/zh-CN/kurosaki.html)**

用于观察NES/Famicom/FDS程序运行状态的模拟器，提供命令行工具、执行跟踪和Python接口。

本项目处于公开预览阶段，API和行为可能发生变化。

## Windows可执行文件

仓库根目录提供Windows x64的Release版本 `kurosaki.exe`。这是命令行程序；运行时不需要另行安装Rust、Python或.NET。请下载仓库ZIP，以便同时获取可执行文件及许可证声明。原生C运行库采用静态链接，程序会使用Windows系统DLL。

```powershell
.\kurosaki.exe --help
```

如需只重新构建这个命令行工具，请运行 `.\scripts\build.ps1`，必要时添加 `-Offline`。脚本会将可执行文件复制到仓库根目录。本次可执行文件发布不包含图形界面、Python扩展模块或C API DLL。详情请参阅[二进制构建记录](BINARY_BUILD.json)和[二进制依赖许可声明](BINARY_NOTICES.md)。

项目自有代码的许可方为 **DAISUKE OBA**，采用MIT许可证。再分发时请一并提供 `LICENSE`、`LICENSE.ja`、`BINARY_NOTICES.md` 和 `licenses/` 目录。第三方库仍适用各自权利人的许可条件。

## 从源码构建并开始使用

重新构建需要近期稳定版Rust工具链。Windows还需要安装Visual Studio Build Tools及“使用C++的桌面开发”工作负载。

```powershell
.\scripts\build.ps1
.\kurosaki.exe --help
```

## 手册与许可证

- [简体中文手册](https://bartaro.github.io/kitaq-docs/zh-CN/kurosaki.html)
- [英文手册](https://bartaro.github.io/kitaq-docs/en/kurosaki.html) / [日文手册](https://bartaro.github.io/kitaq-docs/kurosaki.html)
- [供离线阅读的手册源码](https://github.com/bartaro/kitaq-docs)
- [许可证](LICENSE) / [日文参考译文](LICENSE.ja)

项目许可证不能替代第三方对依赖库、标志或商标规定的条件。再分发时请保留随附声明。

## 公开包的范围

本次公开内容包括模拟器核心、命令行工具和集成API。图形界面及PLITA依赖留待以后发布，因此不包含在这份源码中。

## 图形界面开发方针

KOKURA-GUI和KUROSAKI-GUI目前尚未公开。后续图形界面开发使用PLITA，不使用SDL-GUI或egui-GUI。本仓库继续提供核心、命令行工具和集成API。
