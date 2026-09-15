# KUROSAKI

[English](README.md#english) | [日本語](README.md#japanese) | **Português (Brasil)**

**Abrir o manual de KUROSAKI em português**

Emulador de observação NES/Famicom/FDS com linha de comando, rastreamento e interface Python.

Versão de prévia pública: as APIs e o comportamento podem mudar.

## Executável para Windows

A raiz do repositório inclui `kurosaki.exe`, compilado em modo Release para Windows x64. É um programa de linha de comando. Não é necessário instalar Rust, Python ou .NET para executá-lo. Baixe o ZIP do repositório para obter o executável junto dos avisos de licença. O runtime C nativo é vinculado estaticamente; o programa utiliza DLLs de sistema do Windows.

```powershell
.\kurosaki.exe --help
```

Para recompilar apenas esta ferramenta, use `.\scripts\build.ps1`, com `-Offline` se necessário. O script copia o executável para a raiz do repositório. Interfaces gráficas, módulos de extensão Python e DLLs da API C não fazem parte desta distribuição de executáveis. Consulte o [registro da compilação binária](BINARY_BUILD.json) e os [avisos das dependências do executável](BINARY_NOTICES.md).

O licenciante do código próprio do projeto é **DAISUKE OBA**, sob a licença MIT. Na redistribuição, inclua `LICENSE`, `LICENSE.ja`, `BINARY_NOTICES.md` e a pasta `licenses/`. As bibliotecas de terceiros mantêm seus próprios titulares de direitos e condições.

## Compilar e começar a usar

Para recompilar, use uma versão estável atual do Rust. No Windows, instale o Visual Studio Build Tools com a carga de trabalho de desenvolvimento para desktop com C++.

```powershell
.\scripts\build.ps1
.\kurosaki.exe --help
```

## Manuais e licenças

- Manual em português
- [Manual em inglês](https://bartaro.github.io/kitaq-docs/en/kurosaki.html) / [Manual em japonês](https://bartaro.github.io/kitaq-docs/kurosaki.html)
- [Fontes do manual para leitura offline](https://github.com/bartaro/kitaq-docs)
- [Licença](LICENSE) / [Tradução de referência em japonês](LICENSE.ja)

A licença do projeto não substitui as condições de terceiros relativas a dependências, logotipos ou marcas. Preserve os avisos incluídos ao redistribuir.

## Escopo do pacote público

Esta publicação contém o núcleo do emulador, a ferramenta de linha de comando e as APIs de integração. As interfaces gráficas e a dependência PLITA ficaram para uma publicação futura e não estão incluídas nesta cópia do repositório.

## Política de desenvolvimento das interfaces gráficas

KOKURA-GUI e KUROSAKI-GUI continuam sem publicação. O desenvolvimento futuro das interfaces usará PLITA; SDL-GUI e egui-GUI não estão previstos para esses projetos. Este repositório continua distribuindo o núcleo, a ferramenta de linha de comando e as APIs de integração.
